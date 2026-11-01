//! View state a host publishes without reloading the view: the title and the
//! mode it sends, by a command allowed while busy or by its subscription,
//! survive a later snapshot built from the configuration the host receives.

use super::*;
use roomkit::model::HookResult;
use roomkit_tui_engine::{CommandHandler, CommandResult};
use tokio::sync::watch;

/// Renames on `rename`, even while busy; publishes `published` on `publish`;
/// reloads the view it was given on `reload`.
#[derive(Default)]
struct Renamer {
    updates: Option<watch::Receiver<Vec<Event>>>,
    published: Vec<Event>,
}

impl CommandHandler for Renamer {
    fn subscribe(&self, _: SessionConfig) -> Option<watch::Receiver<Vec<Event>>> {
        self.updates.clone()
    }

    fn allow_while_busy(&self, name: &str) -> bool {
        name == "rename"
    }

    async fn handle(
        &self,
        request: Request,
        view: SessionConfig,
    ) -> Result<CommandResult, Rejection> {
        let Command::Invoke { name, arguments } = request.command else {
            return Err("Unsupported".into());
        };
        match name.as_str() {
            "reload" => return Ok(CommandResult::Switch(Box::new(view))),
            "publish" => return Ok(CommandResult::Events(self.published.clone())),
            _ => {}
        }
        Ok(CommandResult::Events(vec![
            title(&arguments),
            mode("renamed"),
        ]))
    }
}

fn title(title: &str) -> Event {
    Event::TitleChanged {
        conversation: "room".into(),
        title: title.into(),
    }
}

fn mode(selected: &str) -> Event {
    let choices = vec![Choice::new(selected, selected)];
    Event::ModeChanged {
        conversation: "room".into(),
        mode: ModeState::new("mode", selected, choices),
    }
}

fn view() -> SessionConfig {
    let mut view = config();
    view.commands = vec![
        CommandSpec::new("rename", "Rename this conversation").with_arguments("<title>"),
        CommandSpec::new("reload", "Reload this conversation"),
        CommandSpec::new("publish", "Publish the host's view state"),
        CommandSpec::new("mode", "Change the mode"),
    ];
    view
}

fn invoke(id: u64, name: &str, arguments: &str) -> Request {
    let command = Command::Invoke {
        name: name.into(),
        arguments: arguments.into(),
    };
    Request::new(RequestId::new(id), format!("{name}-{id}"), "room", command)
}

/// Reload the view through the host and return the snapshot it shows.
async fn reload(client: &Client, id: u64) -> Snapshot {
    client.try_send(invoke(id, "reload", "")).unwrap();
    loop {
        if let Event::Snapshot(snapshot) = next(client).await {
            return snapshot;
        }
    }
}

/// The title and selected mode a snapshot shows.
fn shown(snapshot: &Snapshot) -> (&str, &str) {
    let mode = snapshot
        .mode
        .as_ref()
        .map_or("", |mode| mode.selected.as_str());
    (snapshot.title.as_str(), mode)
}

#[tokio::test]
async fn a_rename_while_busy_is_published_at_once_and_kept_by_a_later_snapshot() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![ScriptedRound::answering("Done")];
    let channel = prepare(&kit, ScriptedProvider::new(rounds), Arc::default(), false).await;
    let (open, gate) = watch::channel(false);
    kit.before_broadcast("hold", move |_, _| {
        let mut gate = gate.clone();
        async move {
            let _ = gate.wait_for(|open| *open).await;
            Ok(HookResult::allow(Default::default()))
        }
    })
    .unwrap();
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let session = LocalSession::new(kit.clone(), channel, view());
    let task = tokio::spawn(session.run_with_commands(host, Renamer::default()));
    assert!(matches!(next(&client).await, Event::Snapshot(s) if s.title == "room"));
    client.try_send(request("held")).unwrap();
    while !matches!(next(&client).await, Event::OperationStarted { .. }) {}

    client
        .try_send(invoke(2, "rename", "Quarterly review"))
        .unwrap();
    assert!(matches!(
        next(&client).await,
        Event::Reply { error: None, .. }
    ));
    assert_eq!(next(&client).await, title("Quarterly review"));
    assert_eq!(next(&client).await, mode("renamed"));
    // A title the terminal would ignore is not kept for later snapshots.
    let oversized = "x".repeat(257);
    client.try_send(invoke(3, "rename", &oversized)).unwrap();
    while next(&client).await != title(&oversized) {}
    open.send_replace(true);
    collect_operation(&client).await;

    let restored = reload(&client, 4).await;
    assert_eq!(shown(&restored), ("Quarterly review", "renamed"));
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

#[tokio::test]
async fn a_title_and_mode_published_by_the_host_subscription_are_kept_by_a_later_snapshot() {
    let kit = RoomKit::new(Default::default());
    let channel = prepare(&kit, ScriptedProvider::new([]), Arc::default(), false).await;
    let (publish, updates) = watch::channel(vec![title("Named on attach"), mode("attached")]);
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let session = LocalSession::new(kit.clone(), channel, view());
    let handler = Renamer {
        updates: Some(updates),
        ..Renamer::default()
    };
    let task = tokio::spawn(session.run_with_commands(host, handler));
    assert!(matches!(next(&client).await, Event::Snapshot(s) if s.title == "room"));
    assert_eq!(next(&client).await, title("Named on attach"));

    let restored = reload(&client, 1).await;
    assert_eq!(shown(&restored), ("Named on attach", "attached"));

    publish.send_replace(vec![title("Named later"), mode("later")]);
    while next(&client).await != title("Named later") {}
    let restored = reload(&client, 2).await;
    assert_eq!(shown(&restored), ("Named later", "later"));
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

/// Modes a terminal refuses, after one it accepts: an unadvertised command, a
/// selection outside its choices and more choices than a terminal lists.
fn refused_modes() -> Vec<Event> {
    let modes = [
        ModeState::new("unadvertised", "ask", vec![Choice::new("ask", "Ask")]),
        ModeState::new("mode", "missing", vec![Choice::new("ask", "Ask")]),
        ModeState::new(
            "mode",
            "m0",
            (0..33)
                .map(|n| Choice::new(format!("m{n}"), "Mode"))
                .collect(),
        ),
    ];
    let refused = modes.into_iter().map(|mode| Event::ModeChanged {
        conversation: "room".into(),
        mode,
    });
    std::iter::once(mode("plan")).chain(refused).collect()
}

#[tokio::test]
async fn a_mode_the_terminal_would_refuse_is_not_kept_on_any_path() {
    for subscribed in [false, true] {
        let kit = RoomKit::new(Default::default());
        let channel = prepare(&kit, ScriptedProvider::new([]), Arc::default(), false).await;
        let (publish, updates) = watch::channel(Vec::new());
        let handler = Renamer {
            updates: Some(updates),
            published: refused_modes(),
        };
        let (client, host) = connection(NonZeroUsize::new(256).unwrap());
        let session = LocalSession::new(kit.clone(), channel, view());
        let task = tokio::spawn(session.run_with_commands(host, handler));
        assert!(matches!(next(&client).await, Event::Snapshot(_)));
        if subscribed {
            publish.send_replace(refused_modes());
        } else {
            client.try_send(invoke(1, "publish", "")).unwrap();
        }
        let last = refused_modes().pop().unwrap();
        while next(&client).await != last {}

        let restored = reload(&client, 2).await;
        assert_eq!(
            shown(&restored),
            ("room", "plan"),
            "subscribed: {subscribed}"
        );
        drop(client);
        task.await.unwrap().unwrap();
        kit.close().await.unwrap();
    }
}
