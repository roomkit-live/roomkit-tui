//! Request routing gaps found by the review: images, controls allowed while
//! busy, and oversized command arguments.

use super::*;
use roomkit::model::{EventContent, HookResult, TextContent};
use roomkit_tui_engine::{CommandHandler, CommandResult};

struct Host {
    calls: Arc<AtomicUsize>,
}

impl CommandHandler for Host {
    async fn prepare_images(
        &self,
        text: &str,
        images: &[ImageUpload],
        _: &SessionConfig,
    ) -> Result<EventContent, Rejection> {
        Ok(TextContent::new(format!("{text} ({} image)", images.len())).into())
    }

    fn allow_while_busy(&self, name: &str) -> bool {
        name == "status"
    }

    async fn handle(&self, _: Request, _: SessionConfig) -> Result<CommandResult, Rejection> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(CommandResult::Events(Vec::new()))
    }
}

fn images(count: usize) -> Command {
    let image = ImageUpload::png("clipboard.png", "AQID", 1, 1);
    Command::SubmitImages {
        agent: "agent".into(),
        message: Box::new(ImageMessage::new("Describe", vec![image; count])),
    }
}

async fn reply(client: &Client, id: u64) -> Option<Rejection> {
    loop {
        if let Event::Reply { request, error } = next(client).await
            && request == RequestId::new(id)
        {
            return error;
        }
    }
}

#[tokio::test]
async fn images_reach_the_engine_only_through_the_host_and_within_bounds() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![ScriptedRound::answering("Seen")];
    let channel = prepare(&kit, ScriptedProvider::new(rounds), Arc::default(), false).await;
    let mut images_allowed = config();
    images_allowed.can_attach_images = true;
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let view = LocalSession::new(kit.clone(), channel.clone(), images_allowed);
    let task = tokio::spawn(view.run_with_commands(host, Host { calls }));
    next(&client).await;
    client
        .try_send(Request::new(
            RequestId::new(1),
            "too-many",
            "room",
            images(5),
        ))
        .unwrap();
    assert_eq!(
        reply(&client, 1).await.map(|r| r.code),
        Some(ErrorCode::Invalid)
    );
    client
        .try_send(Request::new(
            RequestId::new(2),
            "picture",
            "room",
            images(1),
        ))
        .unwrap();
    assert_eq!(reply(&client, 2).await, None);
    let events = collect_operation(&client).await;
    assert!(events.iter().any(
        |e| matches!(e, Event::Item { item: Item::Message(m), .. } if m.text == "Describe (1 image)")
    ));
    drop(client);
    task.await.unwrap().unwrap();

    // Without a host that stores images, they are refused before the engine.
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let mut images_allowed = config();
    images_allowed.can_attach_images = true;
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, images_allowed).run(host));
    next(&client).await;
    client
        .try_send(Request::new(
            RequestId::new(3),
            "refused",
            "room",
            images(1),
        ))
        .unwrap();
    assert_eq!(
        reply(&client, 3).await.map(|r| r.code),
        Some(ErrorCode::Unsupported)
    );
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

#[tokio::test]
async fn only_opted_in_controls_run_while_busy_and_arguments_are_bounded() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![ScriptedRound::answering("Done")];
    let channel = prepare(&kit, ScriptedProvider::new(rounds), Arc::default(), false).await;
    let (open, gate) = tokio::sync::watch::channel(false);
    kit.hook(
        roomkit::HookTrigger::BeforeBroadcast,
        move |_: roomkit::model::RoomEvent, _| {
            let mut gate = gate.clone();
            async move {
                let _ = gate.wait_for(|open| *open).await;
                Ok(HookResult::allow(Default::default()))
            }
        },
        hook_options("hold"),
    )
    .unwrap();
    let mut commands = config();
    commands.commands = vec![
        CommandSpec::new("status", "Show status"),
        CommandSpec::new("other", "Change settings"),
    ];
    let calls = Arc::new(AtomicUsize::new(0));
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let view = LocalSession::new(kit.clone(), channel, commands);
    let task = tokio::spawn(view.run_with_commands(
        host,
        Host {
            calls: calls.clone(),
        },
    ));
    next(&client).await;
    client.try_send(request("held")).unwrap();
    loop {
        if let Event::OperationStarted { .. } = next(&client).await {
            break;
        }
    }
    let invoke = |id, name: &str, arguments: String| {
        let command = Command::Invoke {
            name: name.into(),
            arguments,
        };
        Request::new(RequestId::new(id), format!("invoke-{id}"), "room", command)
    };
    client.try_send(invoke(2, "status", String::new())).unwrap();
    assert_eq!(
        reply(&client, 2).await,
        None,
        "An opted-in control runs while busy"
    );
    client.try_send(invoke(3, "other", String::new())).unwrap();
    assert_eq!(
        reply(&client, 3).await.map(|r| r.code),
        Some(ErrorCode::Busy)
    );
    client
        .try_send(invoke(4, "status", "x".repeat(65_537)))
        .unwrap();
    assert_eq!(
        reply(&client, 4).await.map(|r| r.code),
        Some(ErrorCode::Invalid)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    open.send_replace(true);
    collect_operation(&client).await;
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

struct Slow {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    calls: Arc<AtomicUsize>,
}

impl CommandHandler for Slow {
    async fn handle(&self, _: Request, _: SessionConfig) -> Result<CommandResult, Rejection> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(CommandResult::Events(Vec::new()))
    }
}

/// Review: requests arriving during a slow handler were held without bound.
#[tokio::test]
async fn requests_held_during_a_slow_handler_are_bounded() {
    let kit = RoomKit::new(Default::default());
    let channel = prepare(&kit, ScriptedProvider::new([]), Arc::default(), false).await;
    let mut commands = config();
    commands.commands = vec![CommandSpec::new("status", "Show status")];
    let handler = Slow {
        entered: Arc::default(),
        release: Arc::default(),
        calls: Arc::default(),
    };
    let (entered, release) = (handler.entered.clone(), handler.release.clone());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let view = LocalSession::new(kit.clone(), channel, commands);
    let task = tokio::spawn(view.run_with_commands(host, handler));
    next(&client).await;
    let invoke = |id: u64| {
        let command = Command::Invoke {
            name: "status".into(),
            arguments: String::new(),
        };
        Request::new(RequestId::new(id), format!("status-{id}"), "room", command)
    };
    client.try_send(invoke(1)).unwrap();
    entered.notified().await;
    for id in 2..=70 {
        client.try_send(invoke(id)).unwrap();
    }
    let (mut accepted, mut overloaded) = (0, 0);
    while accepted + overloaded < 70 {
        if overloaded == 5 && accepted == 0 {
            release.notify_one();
        }
        match next(&client).await {
            Event::Reply { error: None, .. } => accepted += 1,
            Event::Reply { error: Some(r), .. } if r.code == ErrorCode::Overloaded => {
                overloaded += 1
            }
            Event::Reply { error: Some(r), .. } => panic!("unexpected refusal: {r:?}"),
            _ => {}
        }
    }
    assert_eq!((accepted, overloaded), (65, 5));
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}
