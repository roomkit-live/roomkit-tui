//! A view moves between rooms while work runs in them: it leaves a turn
//! running, finds it again, and sees a turn it did not start.

use super::*;
use roomkit_tui_engine::{CommandHandler, CommandResult, StopRun};
use std::sync::atomic::AtomicBool;
use tokio::sync::{Notify, watch};

/// `go <room>` moves the view there, even while busy.
struct Go {
    /// The host stops a run this view did not start: set once it did.
    stopped: Option<Arc<AtomicBool>>,
}

impl CommandHandler for Go {
    fn allow_while_busy(&self, name: &str) -> bool {
        name == "go"
    }

    fn stop_run(&self, _: &SessionConfig, _: &RunId) -> Option<StopRun> {
        let stopped = self.stopped.clone()?;
        Some(Box::pin(async move {
            stopped.store(true, Ordering::SeqCst);
            Ok(())
        }))
    }

    async fn handle(
        &self,
        request: Request,
        mut view: SessionConfig,
    ) -> Result<CommandResult, Rejection> {
        let Command::Invoke { arguments, .. } = request.command else {
            return Err("Unsupported".into());
        };
        view.room = RoomId::new(&arguments).unwrap();
        // "alone" is a conversation of its own; the others are one group.
        view.group = (arguments == "other").then(|| "room".to_owned());
        Ok(CommandResult::Switch(Box::new(view)))
    }
}

/// Holds the agent's answers as they are stored, telling `held` once one
/// waits for `gate`: its text is on screen, its row is not.
fn hold_answers(kit: &RoomKit, gate: watch::Receiver<bool>, held: Arc<Notify>) {
    kit.hook(
        roomkit::HookTrigger::BeforeBroadcast,
        move |event: roomkit::model::RoomEvent, _| {
            let (mut gate, held) = (gate.clone(), held.clone());
            let answer = event.source.channel_id == config().agent;
            async move {
                if answer {
                    held.notify_one();
                    let _ = gate.wait_for(|open| *open).await;
                }
                Ok(roomkit::model::HookResult::allow(Default::default()))
            }
        },
        hook_options("hold-answers"),
    )
    .unwrap();
}

async fn with_other_room(kit: &RoomKit, channel: &ConversationChannel) {
    with_room(kit, channel, "other").await;
}

async fn with_room(kit: &RoomKit, channel: &ConversationChannel, name: &str) {
    let room = RoomId::new(name).unwrap();
    kit.create_room(CreateRoomOptions {
        room_id: Some(room.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    for id in [channel.id(), &config().agent] {
        kit.attach_channel(&room, id, Default::default())
            .await
            .unwrap();
    }
}

fn commanded() -> SessionConfig {
    let mut view = config();
    view.commands.push(CommandSpec::new("go", "Move the view"));
    view
}

fn go(from: &str, to: &str) -> Request {
    let command = Command::Invoke {
        name: "go".into(),
        arguments: to.into(),
    };
    Request::new(RequestId::new(7), format!("go-{to}"), from, command)
}

/// The events up to the snapshot of `room`, and the few that follow it at once.
async fn until_snapshot(client: &Client, room: &str) -> Vec<Event> {
    loop {
        let event = next(client).await;
        if matches!(&event, Event::Snapshot(s) if s.conversation.as_str() == room) {
            break;
        }
        assert!(
            !matches!(&event, Event::Reply { error: Some(_), .. }),
            "The switch was refused: {event:?}"
        );
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut after = Vec::new();
    while let Ok(event) = client.try_recv() {
        after.push(event);
    }
    after
}

async fn started_run(client: &Client) -> RunId {
    loop {
        if let Event::RunStarted { run, .. } = next(client).await {
            return run;
        }
    }
}

fn text_of(events: &[Event]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            Event::TextDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_view_leaves_a_running_turn_and_finds_it_again() {
    let kit = RoomKit::new(Default::default());
    let provider = ScriptedProvider::new([ScriptedRound::answering("Partial answer")]);
    let channel = prepare(&kit, provider, Arc::default(), false).await;
    with_other_room(&kit, &channel).await;
    let (open, gate) = watch::channel(false);
    let held = Arc::new(Notify::new());
    hold_answers(&kit, gate, held.clone());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let view = LocalSession::new(kit.clone(), channel, commanded());
    let task = tokio::spawn(view.run_with_commands(host, Go { stopped: None }));
    next(&client).await;
    client.try_send(request("ask")).unwrap();
    let run = started_run(&client).await;
    held.notified().await;

    // Away, the turn goes on and the view is told nothing of it.
    client.try_send(go("room", "other")).unwrap();
    let away = until_snapshot(&client, "other").await;
    assert!(text_of(&away).is_empty(), "{away:?}");
    assert!(
        !away.iter().any(|e| matches!(e, Event::RunStarted { .. })),
        "{away:?}"
    );

    // Back, the run under way and what it said so far are shown, once.
    client.try_send(go("other", "room")).unwrap();
    let back = until_snapshot(&client, "room").await;
    assert!(
        back.iter()
            .any(|e| matches!(e, Event::RunStarted { run: r, .. } if *r == run)),
        "{back:?}"
    );
    assert_eq!(text_of(&back), "Partial answer", "{back:?}");

    // It is still this view's to stop.
    let stop = Command::Interrupt { run: run.clone() };
    client
        .try_send(Request::new(RequestId::new(9), "stop", "room", stop))
        .unwrap();
    let reply = loop {
        if let Event::Reply { request, error } = next(&client).await
            && request == RequestId::new(9)
        {
            break error;
        }
    };
    assert_eq!(reply, None);
    open.send_replace(true);
    loop {
        if matches!(next(&client).await, Event::RunFinished { run: r, .. } if r == run) {
            break;
        }
    }
    drop(client);
    task.await.unwrap().unwrap();
}

/// A turn of a room this view never showed: it finds the run under way and
/// its text, then follows it to its end.
#[tokio::test]
async fn a_view_arrives_in_a_turn_it_did_not_start() {
    let kit = RoomKit::new(Default::default());
    let provider = ScriptedProvider::new([ScriptedRound::answering("Elsewhere")]);
    let channel = prepare(&kit, provider, Arc::default(), false).await;
    with_other_room(&kit, &channel).await;
    let (open, gate) = watch::channel(false);
    let held = Arc::new(Notify::new());
    hold_answers(&kit, gate, held.clone());

    let (elsewhere, other_host) = connection(NonZeroUsize::new(256).unwrap());
    let mut other = commanded();
    other.room = RoomId::new("other").unwrap();
    let other_view = LocalSession::new(kit.clone(), channel.clone(), other);
    let other_task = tokio::spawn(other_view.run_with_commands(other_host, Go { stopped: None }));
    next(&elsewhere).await;
    let mut ask = request("ask");
    ask.conversation = "other".into();
    elsewhere.try_send(ask).unwrap();
    let run = started_run(&elsewhere).await;
    held.notified().await;

    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let view = LocalSession::new(kit.clone(), channel, commanded());
    let task = tokio::spawn(view.run_with_commands(host, Go { stopped: None }));
    next(&client).await;
    client.try_send(go("room", "other")).unwrap();
    let arrived = until_snapshot(&client, "other").await;
    assert!(
        arrived
            .iter()
            .any(|e| matches!(e, Event::RunStarted { run: r, .. } if *r == run)),
        "{arrived:?}"
    );
    assert_eq!(text_of(&arrived), "Elsewhere", "{arrived:?}");

    open.send_replace(true);
    loop {
        if matches!(next(&client).await, Event::RunFinished { run: r, .. } if r == run) {
            break;
        }
    }
    drop((client, elsewhere));
    task.await.unwrap().unwrap();
    other_task.await.unwrap().unwrap();
}

/// A decision waiting in a room the view left stays open, and is shown again
/// on its return.
#[tokio::test]
async fn a_decision_waits_for_the_view_to_come_back() {
    let kit = RoomKit::new(Default::default());
    let provider = ScriptedProvider::new([ScriptedRound::answering("Done")]);
    let channel = prepare(&kit, provider, Arc::default(), false).await;
    with_other_room(&kit, &channel).await;
    let (open, gate) = watch::channel(false);
    let held = Arc::new(Notify::new());
    hold_answers(&kit, gate, held.clone());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let view = LocalSession::new(kit.clone(), channel.clone(), commanded());
    let task = tokio::spawn(view.run_with_commands(host, Go { stopped: None }));
    next(&client).await;
    client.try_send(request("ask")).unwrap();
    started_run(&client).await;
    held.notified().await;

    let (room, agent) = (config().room, config().agent);
    let asker = channel.clone();
    let asked = tokio::spawn(async move {
        let request = PermissionRequest::new("Allow it?", "detail", Duration::from_secs(30));
        asker.ask_permission(&room, &agent, request).await
    });
    loop {
        if matches!(next(&client).await, Event::PermissionRequested { .. }) {
            break;
        }
    }

    client.try_send(go("room", "other")).unwrap();
    until_snapshot(&client, "other").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !asked.is_finished(),
        "Leaving the room withdrew the decision"
    );

    client.try_send(go("other", "room")).unwrap();
    let back = until_snapshot(&client, "room").await;
    let permission = back
        .iter()
        .find_map(|event| match event {
            Event::PermissionRequested { permission, .. } => Some(permission.id.clone()),
            _ => None,
        })
        .expect("The decision is shown again");
    let decide = Command::Decide {
        permission,
        decision: Decision::Allow,
    };
    client
        .try_send(Request::new(RequestId::new(3), "decide", "room", decide))
        .unwrap();
    assert_eq!(asked.await.unwrap(), Decision::Allow);
    open.send_replace(true);
    drop(client);
    task.await.unwrap().unwrap();
}

/// A run this view did not start goes to the host to stop, when it can.
#[tokio::test]
async fn a_run_the_view_did_not_start_is_stopped_by_the_host() {
    let kit = RoomKit::new(Default::default());
    let channel = prepare(&kit, ScriptedProvider::new([]), Arc::default(), false).await;
    let (client, host) = connection(NonZeroUsize::new(64).unwrap());
    let stopped = Arc::new(AtomicBool::new(false));
    let handler = Go {
        stopped: Some(stopped.clone()),
    };
    let view = LocalSession::new(kit.clone(), channel, commanded());
    let task = tokio::spawn(view.run_with_commands(host, handler));
    next(&client).await;
    let stop = Command::Interrupt {
        run: "theirs".into(),
    };
    client
        .try_send(Request::new(RequestId::new(5), "stop", "room", stop))
        .unwrap();
    let reply = loop {
        if let Event::Reply { request, error } = next(&client).await
            && request == RequestId::new(5)
        {
            break error;
        }
    };
    assert_eq!(reply, None);
    assert!(stopped.load(Ordering::SeqCst));
    drop(client);
    task.await.unwrap().unwrap();
}

/// The person looks at a worker while the delegating agent's room is idle:
/// what that agent asks next, handing back a result, waits for them rather
/// than being refused. Past the group, nobody can answer it any more.
#[tokio::test]
async fn a_group_s_room_left_idle_keeps_its_approvals_open() {
    let kit = RoomKit::new(Default::default());
    let channel = prepare(&kit, ScriptedProvider::new([]), Arc::default(), false).await;
    with_other_room(&kit, &channel).await;
    with_room(&kit, &channel, "alone").await;
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let view = LocalSession::new(kit.clone(), channel.clone(), commanded());
    let task = tokio::spawn(view.run_with_commands(host, Go { stopped: None }));
    next(&client).await;
    let ask = |wait: u64| {
        let (room, agent, asker) = (config().room, config().agent, channel.clone());
        async move {
            let request = PermissionRequest::new("Allow it?", "d", Duration::from_millis(wait));
            let started = std::time::Instant::now();
            let decision = asker.ask_permission(&room, &agent, request).await;
            (decision, started.elapsed())
        }
    };

    client.try_send(go("room", "other")).unwrap();
    until_snapshot(&client, "other").await;
    let (_, waited) = ask(150).await;
    assert!(
        waited >= Duration::from_millis(120),
        "Refused at once: {waited:?}"
    );

    client.try_send(go("other", "alone")).unwrap();
    until_snapshot(&client, "alone").await;
    let (decision, waited) = ask(150).await;
    assert_eq!(decision, Decision::Deny);
    assert!(
        waited < Duration::from_millis(100),
        "Still kept: {waited:?}"
    );
    drop(client);
    task.await.unwrap().unwrap();
}

/// An approval asked in a room left, while the view is in another room of
/// its group, is told there: nothing else would say the agent is waiting.
#[tokio::test]
async fn an_approval_in_a_room_left_is_told_in_the_room_shown() {
    let kit = RoomKit::new(Default::default());
    let provider = ScriptedProvider::new([ScriptedRound::answering("Done")]);
    let channel = prepare(&kit, provider, Arc::default(), false).await;
    with_other_room(&kit, &channel).await;
    let (open, gate) = watch::channel(false);
    let held = Arc::new(Notify::new());
    hold_answers(&kit, gate, held.clone());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let view = LocalSession::new(kit.clone(), channel.clone(), commanded());
    let task = tokio::spawn(view.run_with_commands(host, Go { stopped: None }));
    next(&client).await;
    client.try_send(request("ask")).unwrap();
    started_run(&client).await;
    held.notified().await;
    client.try_send(go("room", "other")).unwrap();
    until_snapshot(&client, "other").await;

    let (room, agent) = (config().room, config().agent);
    let asker = channel.clone();
    let asked = tokio::spawn(async move {
        let request = PermissionRequest::new("Allow it?", "d", Duration::from_secs(30));
        asker.ask_permission(&room, &agent, request).await
    });
    let notice = loop {
        if let Event::Notice { conversation, text } = next(&client).await {
            assert_eq!(conversation.as_str(), "other");
            break text;
        }
    };
    assert!(notice.contains("waiting for your approval"), "{notice}");
    asked.abort();
    open.send_replace(true);
    drop(client);
    task.await.unwrap().unwrap();
}

/// A conversation left for one of another group is not reachable from it:
/// nothing there says an agent of "this conversation" is waiting.
#[tokio::test]
async fn an_approval_outside_the_group_is_not_told() {
    let kit = RoomKit::new(Default::default());
    let provider = ScriptedProvider::new([ScriptedRound::answering("Done")]);
    let channel = prepare(&kit, provider, Arc::default(), false).await;
    with_room(&kit, &channel, "alone").await;
    let (open, gate) = watch::channel(false);
    let held = Arc::new(Notify::new());
    hold_answers(&kit, gate, held.clone());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let view = LocalSession::new(kit.clone(), channel.clone(), commanded());
    let task = tokio::spawn(view.run_with_commands(host, Go { stopped: None }));
    next(&client).await;
    client.try_send(request("ask")).unwrap();
    started_run(&client).await;
    held.notified().await;
    client.try_send(go("room", "alone")).unwrap();
    until_snapshot(&client, "alone").await;

    let (room, agent) = (config().room, config().agent);
    let asker = channel.clone();
    let asked = tokio::spawn(async move {
        let request = PermissionRequest::new("Allow it?", "d", Duration::from_secs(30));
        asker.ask_permission(&room, &agent, request).await
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    while let Ok(event) = client.try_recv() {
        assert!(!matches!(event, Event::Notice { .. }), "{event:?}");
    }
    asked.abort();
    open.send_replace(true);
    drop(client);
    task.await.unwrap().unwrap();
}
