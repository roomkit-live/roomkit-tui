//! RMK-1161: a message sent while the agent's turn runs goes to that turn.

use super::*;
use roomkit::model::{HookResult, RoomEvent};
use roomkit_tui_engine::{
    CommandExecution, CommandHandler, CommandResult, LifecycleFuture, SessionLifecycle, TurnOutcome,
};
use std::sync::Mutex;
use tokio::sync::{Notify, watch};

mod cut;
mod lifecycle;

/// The console and the agent of a test's room.
#[derive(Clone, Copy)]
struct Setup {
    /// The console shows only committed rows.
    buffered: bool,
    /// The agent steers its room's turns (`steer_room_turns`).
    steers: bool,
}

/// An agent that steers its room's turns, answering `rounds`, beside a
/// streaming console.
async fn steering_room(
    kit: &RoomKit,
    rounds: Vec<ScriptedRound>,
) -> (ConversationChannel, ScriptedProvider) {
    let setup = Setup {
        buffered: false,
        steers: true,
    };
    room_with(kit, rounds, setup).await
}

/// The room `setup` describes, its agent answering `rounds`.
async fn room_with(
    kit: &RoomKit,
    rounds: Vec<ScriptedRound>,
    setup: Setup,
) -> (ConversationChannel, ScriptedProvider) {
    let provider = ScriptedProvider::new(rounds);
    let lookup = AiTool {
        name: "lookup_record".to_owned(),
        description: "Read a fictional record".to_owned(),
        parameters: json!({"type":"object","properties":{},"additionalProperties":false}),
        tags: Vec::new(),
        changes_state: false,
        defer_loading: false,
    };
    let options = AiChannelOptions {
        tools: vec![ToolEntry::Definition(lookup)],
        tool_handler: Some(ToolHandler::new(|_, _| async {
            Ok(ToolResult::Json(json!({"record":"42"})))
        })),
        steer_room_turns: setup.steers,
        ..Default::default()
    };
    let agent = AiChannel::new(config().agent, provider.clone(), options).unwrap();
    kit.register_channel(agent).unwrap();
    let id = ChannelId::new("console").unwrap();
    let channel = if setup.buffered {
        ConversationChannel::buffered(id)
    } else {
        ConversationChannel::new(id)
    };
    kit.register_channel(channel.clone()).unwrap();
    let room = config().room;
    kit.create_room(CreateRoomOptions {
        room_id: Some(room.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    for id in [channel.id().clone(), config().agent] {
        kit.attach_channel(&room, &id, Default::default())
            .await
            .unwrap();
    }
    (channel, provider)
}

/// Hold every tool call until `gate` opens, telling `entered` once one waits.
fn hold_tools(kit: &RoomKit, gate: watch::Receiver<bool>, entered: Arc<Notify>) {
    kit.hook(
        roomkit::HookTrigger::BeforeToolUse,
        move |_: roomkit::ToolCallEvent, _| {
            let (mut gate, entered) = (gate.clone(), entered.clone());
            async move {
                entered.notify_one();
                let _ = gate.wait_for(|open| *open).await;
                Ok(HookResult::allow(Default::default()))
            }
        },
        hook_options("hold-tools"),
    )
    .unwrap();
}

/// Tell `delivered` once the delivery of the message `text` is over: the turn
/// under way took it, or RoomKit has it wait a turn of its own. The host says a
/// message went to a turn as it is committed, before that (RMK-1371).
fn tell_when_delivered(kit: &RoomKit, text: &'static str, delivered: Arc<Notify>) {
    let options = roomkit::HookOptions {
        execution: roomkit::HookExecution::Async,
        ..hook_options("delivered")
    };
    let tell = move |event: RoomEvent, _| {
        if roomkit_core::rules::transcoding::text_of(&event.content) == text {
            delivered.notify_one();
        }
        async {}
    };
    kit.hook(roomkit::HookTrigger::AfterBroadcast, tell, options)
        .unwrap();
}

/// Returns once `told` says a message's delivery is over.
async fn delivery_ended(told: &Notify) {
    let ended = tokio::time::timeout(Duration::from_secs(5), told.notified());
    ended.await.expect("The message's delivery ends");
}

fn submit(id: u64, operation: &str, text: &str) -> Request {
    let command = Command::Submit {
        agent: "agent".into(),
        text: text.into(),
    };
    Request::new(RequestId::new(id), operation, "room", command)
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

/// Whether the model was asked with `text` among its messages.
fn asked_with(provider: &ScriptedProvider, round: usize, text: &str) -> bool {
    provider.requests()[round]
        .messages
        .iter()
        .any(|message| message.text().contains(text))
}

/// RMK-1256: the host declares nothing; the engine reads the agent's channel.
#[tokio::test]
async fn a_message_during_a_tool_round_goes_to_the_running_turn() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![
        ScriptedRound::calling("Checking", [("call1", "lookup_record", "{}")]),
        ScriptedRound::answering("Record 42 is in the second ledger"),
    ];
    let (channel, provider) = steering_room(&kit, rounds).await;
    let (open, gate) = watch::channel(false);
    let entered = Arc::new(Notify::new());
    hold_tools(&kit, gate, entered.clone());
    let delivered = Arc::new(Notify::new());
    tell_when_delivered(&kit, "Use the second ledger", delivered.clone());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    assert!(matches!(next(&client).await, Event::Snapshot(snapshot) if snapshot.can_steer));
    client
        .try_send(submit(1, "one", "Look up record 42"))
        .unwrap();
    entered.notified().await;
    let mut turn = None;
    client
        .try_send(submit(2, "two", "Use the second ledger"))
        .unwrap();
    let (message, steered_into) = loop {
        match next(&client).await {
            Event::RunStarted { run, .. } => turn = Some(run),
            Event::Reply { request, error } if request == RequestId::new(2) => {
                assert_eq!(error, None, "The message is accepted during the turn");
            }
            Event::MessageSteered { message, run, .. } => break (message, run),
            _ => {}
        }
    };
    assert_eq!(Some(steered_into), turn, "Handed to the running turn");
    delivery_ended(&delivered).await;
    open.send_replace(true);
    let events = collect_operation(&client).await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::OperationStarted { operation, .. } if operation.as_str() == "two")),
        "No operation of its own: {events:#?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::RunStarted { answers: Some(m), .. } if *m == message)),
        "No turn of its own: {events:#?}"
    );
    assert_eq!(provider.requests().len(), 2);
    assert!(asked_with(&provider, 1, "Use the second ledger"));
    drop(client);
    task.await.unwrap().unwrap();
    channel_idle(&kit).await;
}

/// Hold the agent's first answer as it is stored, telling `held` once it
/// waits: the turn's loop is over and the room is locked.
fn hold_first_answer(kit: &RoomKit, gate: watch::Receiver<bool>, held: Arc<Notify>) {
    let (agent, first) = (config().agent, Arc::new(AtomicUsize::new(0)));
    kit.hook(
        roomkit::HookTrigger::BeforeBroadcast,
        move |event: RoomEvent, _| {
            let (mut gate, held) = (gate.clone(), held.clone());
            let hold =
                event.source.channel_id == agent && first.fetch_add(1, Ordering::SeqCst) == 0;
            async move {
                if hold {
                    held.notify_one();
                    let _ = gate.wait_for(|open| *open).await;
                }
                Ok(HookResult::allow(Default::default()))
            }
        },
        hook_options("hold-answer"),
    )
    .unwrap();
}

/// Send a message while the first turn's answer is held: the turn still
/// runs, so the message is accepted for it, but the turn no longer reads it
/// and RoomKit gives it a turn of its own. Returns that turn's run.
async fn steer_into_its_own_turn(
    client: &Client,
    open: &watch::Sender<bool>,
    held: &Notify,
) -> RunId {
    client.try_send(submit(1, "one", "First question")).unwrap();
    held.notified().await;
    client
        .try_send(submit(2, "two", "Second question"))
        .unwrap();
    let mut steered = None;
    loop {
        match next(client).await {
            Event::Reply { request, error } if request == RequestId::new(2) => {
                assert_eq!(error, None, "Accepted for the running turn");
                open.send_replace(true);
            }
            Event::MessageSteered { message, .. } => steered = Some(message),
            Event::RunStarted {
                run,
                answers: Some(message),
                ..
            } if Some(&message) == steered.as_ref() => return run,
            _ => {}
        }
    }
}

/// The review of 5374eb1: a steered message's own turn could not be stopped.
#[tokio::test]
async fn a_steered_message_s_own_turn_can_be_stopped() {
    let kit = RoomKit::new(Default::default());
    let used = json!({ "input_tokens": 500, "output_tokens": 10 });
    let rounds = vec![
        ScriptedRound::answering("First answer"),
        ScriptedRound::calling("Checking", [("call2", "lookup_record", "{}")])
            .with_usage(used.as_object().unwrap().clone()),
        ScriptedRound::answering("Never said"),
    ];
    let (channel, _) = steering_room(&kit, rounds).await;
    let (open, gate) = watch::channel(false);
    let held = Arc::new(Notify::new());
    hold_first_answer(&kit, gate, held.clone());
    let (tools_open, tools_gate) = watch::channel(false);
    let entered = Arc::new(Notify::new());
    hold_tools(&kit, tools_gate, entered.clone());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let mut config = config();
    config.model = Some(ModelInfo::new("scripted", "scripted"));
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config).run(host));
    next(&client).await;
    let own = steer_into_its_own_turn(&client, &open, &held).await;
    entered.notified().await;
    let interrupt = Command::Interrupt { run: own.clone() };
    client
        .try_send(Request::new(RequestId::new(3), "stop", "room", interrupt))
        .unwrap();
    assert_eq!(reply(&client, 3).await, None, "The person stops it");
    let outcome = loop {
        if let Event::RunFinished { run, outcome, .. } = next(&client).await
            && run == own
        {
            break outcome;
        }
    };
    assert_eq!(outcome, RunOutcome::Interrupted);
    tools_open.send_replace(true);
    // RMK-1333: its usage comes after it stopped, as a turn of its own's does.
    let usage = loop {
        if let Event::UsageUpdated { run, usage, .. } = next(&client).await
            && run == own
        {
            break usage;
        }
    };
    assert_eq!(usage.input_tokens, Some(500));
    drop(client);
    task.await.unwrap().unwrap();
    channel_idle(&kit).await;
}

/// The review of 5374eb1: decisions taken in a steered message's own turn
/// were not stored.
#[tokio::test]
async fn decisions_in_a_steered_message_s_own_turn_are_stored() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![
        ScriptedRound::answering("First answer"),
        ScriptedRound::calling("Checking", [("call2", "lookup_record", "{}")]),
        ScriptedRound::answering("Second answer"),
    ];
    let (channel, _) = steering_room(&kit, rounds).await;
    let (open, gate) = watch::channel(false);
    let held = Arc::new(Notify::new());
    hold_first_answer(&kit, gate, held.clone());
    let asking = channel.clone();
    kit.hook(
        roomkit::HookTrigger::BeforeToolUse,
        move |event: roomkit::ToolCallEvent, _| {
            let asking = asking.clone();
            async move {
                let request =
                    PermissionRequest::new("Read?", "Read a record", Duration::from_secs(5));
                let decision = asking.ask_tool_permission(&event, request).await;
                Ok(match decision {
                    Decision::Allow => HookResult::allow(Default::default()),
                    _ => HookResult::block("Denied", Default::default()),
                })
            }
        },
        hook_options("permission"),
    )
    .unwrap();
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel.clone(), config()).run(host));
    next(&client).await;
    let own = steer_into_its_own_turn(&client, &open, &held).await;
    let permission = loop {
        if let Event::PermissionRequested { permission, .. } = next(&client).await {
            break permission;
        }
    };
    assert_eq!(permission.run, own);
    let decide = Command::Decide {
        permission: permission.id.clone(),
        decision: Decision::Allow,
    };
    client
        .try_send(Request::new(RequestId::new(3), "decide", "room", decide))
        .unwrap();
    loop {
        if let Event::RunFinished { run, .. } = next(&client).await
            && run == own
        {
            break;
        }
    }
    drop(client);
    task.await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(5), channel.idle())
        .await
        .unwrap();
    let rows = kit
        .get_timeline(&config().room, GetTimelineOptions::default())
        .await
        .unwrap();
    assert!(
        rows.iter()
            .any(|row| row.metadata.contains_key("roomkit_tui_approval")),
        "The decision is stored"
    );
    channel_idle(&kit).await;
}

/// A host that holds `started` until `release` is told.
#[derive(Debug, Default)]
struct SlowStart {
    entered: Notify,
    release: Notify,
}

impl SessionLifecycle for SlowStart {
    fn started<'a>(&'a self, _: &'a SessionConfig, _: &'a OperationId) -> LifecycleFuture<'a, ()> {
        Box::pin(async {
            self.entered.notify_one();
            self.release.notified().await;
        })
    }
}

/// The review of 5374eb1: a message sent while the host prepared the first
/// one was answered first.
#[tokio::test]
async fn a_message_while_the_host_prepares_the_turn_is_busy() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![ScriptedRound::answering("Answer")];
    let (channel, provider) = steering_room(&kit, rounds).await;
    let lifecycle = Arc::new(SlowStart::default());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let session =
        LocalSession::new(kit.clone(), channel, config()).with_lifecycle(lifecycle.clone());
    let task = tokio::spawn(session.run(host));
    next(&client).await;
    client.try_send(submit(1, "one", "First question")).unwrap();
    lifecycle.entered.notified().await;
    client
        .try_send(submit(2, "two", "Second question"))
        .unwrap();
    let refused = reply(&client, 2).await.map(|rejection| rejection.code);
    assert_eq!(refused, Some(ErrorCode::Busy), "No turn runs yet");
    lifecycle.release.notify_one();
    collect_operation(&client).await;
    assert_eq!(provider.requests().len(), 1);
    drop(client);
    task.await.unwrap().unwrap();
    channel_idle(&kit).await;
}

/// The review of 5374eb1: a message sent once the turn was being stopped
/// went to it, which reads nothing more. The turn is interrupted while its
/// answer is held, its stream still open: the message waits.
#[tokio::test]
async fn a_message_after_an_interruption_never_goes_to_the_stopped_turn() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![
        ScriptedRound::answering("First answer"),
        ScriptedRound::answering("Second answer"),
    ];
    let (channel, _) = steering_room(&kit, rounds).await;
    let (open, gate) = watch::channel(false);
    let held = Arc::new(Notify::new());
    hold_first_answer(&kit, gate, held.clone());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    next(&client).await;
    client.try_send(submit(1, "one", "First question")).unwrap();
    let stopped = loop {
        if let Event::RunStarted { run, .. } = next(&client).await {
            break run;
        }
    };
    held.notified().await;
    let interrupt = Command::Interrupt { run: stopped };
    client
        .try_send(Request::new(RequestId::new(3), "stop", "room", interrupt))
        .unwrap();
    assert_eq!(reply(&client, 3).await, None);
    client.try_send(submit(2, "two", "Too late")).unwrap();
    let refused = reply(&client, 2).await.map(|rejection| rejection.code);
    assert_eq!(
        refused,
        Some(ErrorCode::Busy),
        "Never handed to the stopped turn"
    );
    open.send_replace(true);
    drop(client);
    task.await.unwrap().unwrap();
    channel_idle(&kit).await;
}

/// A host whose admission of operation `m0` waits until `release`.
#[derive(Debug, Default)]
struct StuckAdmit {
    entered: Notify,
    release: Notify,
}

impl SessionLifecycle for StuckAdmit {
    fn admit<'a>(
        &'a self,
        _: &'a SessionConfig,
        operation: &'a OperationId,
        _: &'a roomkit::model::EventContent,
    ) -> LifecycleFuture<'a, Result<(), Rejection>> {
        let stuck = operation.as_str() == "m0";
        Box::pin(async move {
            if stuck {
                self.entered.notify_one();
                self.release.notified().await;
            }
            Ok(())
        })
    }
}

/// The light review of 55ee836: stopping a steered message's own turn
/// waited behind an earlier steered message still in its host's admission.
#[tokio::test]
async fn stopping_a_steered_turn_never_waits_behind_another_message() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![
        ScriptedRound::answering("First answer"),
        ScriptedRound::calling("Checking", [("call2", "lookup_record", "{}")]),
        ScriptedRound::answering("Never said"),
        ScriptedRound::answering("Spare"),
    ];
    let (channel, _) = steering_room(&kit, rounds).await;
    let (open, gate) = watch::channel(false);
    let held = Arc::new(Notify::new());
    hold_first_answer(&kit, gate, held.clone());
    let (tools_open, tools_gate) = watch::channel(false);
    let entered = Arc::new(Notify::new());
    hold_tools(&kit, tools_gate, entered.clone());
    let lifecycle = Arc::new(StuckAdmit::default());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let session =
        LocalSession::new(kit.clone(), channel, config()).with_lifecycle(lifecycle.clone());
    let task = tokio::spawn(session.run(host));
    next(&client).await;
    client.try_send(submit(1, "one", "First question")).unwrap();
    held.notified().await;
    client.try_send(submit(2, "m0", "Zero")).unwrap();
    assert_eq!(reply(&client, 2).await, None);
    lifecycle.entered.notified().await;
    client.try_send(submit(4, "m1", "Second question")).unwrap();
    let mut steered = None;
    let own = loop {
        match next(&client).await {
            Event::Reply { request, error } if request == RequestId::new(4) => {
                assert_eq!(error, None);
                open.send_replace(true);
            }
            Event::MessageSteered { message, .. } => steered = steered.or(Some(message)),
            Event::RunStarted {
                run,
                answers: Some(message),
                ..
            } if Some(&message) == steered.as_ref() => break run,
            _ => {}
        }
    };
    entered.notified().await;
    let interrupt = Command::Interrupt { run: own };
    client
        .try_send(Request::new(RequestId::new(5), "stop", "room", interrupt))
        .unwrap();
    let answered = tokio::time::timeout(Duration::from_secs(2), reply(&client, 5)).await;
    lifecycle.release.notify_one();
    tools_open.send_replace(true);
    assert_eq!(answered.ok(), Some(None), "Stopped at once");
    drop(client);
    task.await.unwrap().unwrap();
    channel_idle(&kit).await;
}

/// The review of 5374eb1: a buffered console never shows where a message
/// went, so it does not steer.
#[tokio::test]
async fn a_buffered_console_does_not_steer() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![
        ScriptedRound::calling("Checking", [("call1", "lookup_record", "{}")]),
        ScriptedRound::answering("Done"),
    ];
    let setup = Setup {
        buffered: true,
        steers: true,
    };
    let (channel, _) = room_with(&kit, rounds, setup).await;
    let (open, gate) = watch::channel(false);
    let entered = Arc::new(Notify::new());
    hold_tools(&kit, gate, entered.clone());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    assert!(matches!(next(&client).await, Event::Snapshot(snapshot) if !snapshot.can_steer));
    client
        .try_send(submit(1, "one", "Look up record 42"))
        .unwrap();
    entered.notified().await;
    client.try_send(submit(2, "two", "Also this")).unwrap();
    let refused = reply(&client, 2).await.map(|rejection| rejection.code);
    assert_eq!(refused, Some(ErrorCode::Busy));
    open.send_replace(true);
    collect_operation(&client).await;
    drop(client);
    task.await.unwrap().unwrap();
    channel_idle(&kit).await;
}

/// RMK-1256: an agent that does not steer its room's turns would run a
/// second turn beside the first; its console keeps the message busy.
#[tokio::test]
async fn a_message_during_a_turn_is_busy_unless_the_agent_steers() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![
        ScriptedRound::calling("Checking", [("call1", "lookup_record", "{}")]),
        ScriptedRound::answering("Done"),
    ];
    let setup = Setup {
        buffered: false,
        steers: false,
    };
    let (channel, _) = room_with(&kit, rounds, setup).await;
    let (open, gate) = watch::channel(false);
    let entered = Arc::new(Notify::new());
    hold_tools(&kit, gate, entered.clone());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    assert!(matches!(next(&client).await, Event::Snapshot(snapshot) if !snapshot.can_steer));
    client
        .try_send(submit(1, "one", "Look up record 42"))
        .unwrap();
    entered.notified().await;
    client.try_send(submit(2, "two", "Also this")).unwrap();
    let refused = reply(&client, 2).await.map(|rejection| rejection.code);
    assert_eq!(refused, Some(ErrorCode::Busy));
    open.send_replace(true);
    collect_operation(&client).await;
    drop(client);
    task.await.unwrap().unwrap();
    channel_idle(&kit).await;
}

/// Host work that waits until `release` is told.
struct Holding {
    release: Arc<Notify>,
}

impl CommandHandler for Holding {
    async fn handle(&self, _: Request, _: SessionConfig) -> Result<CommandResult, Rejection> {
        let release = self.release.clone();
        let work = CommandExecution::new("deploy-1".into(), move |_| async move {
            release.notified().await;
            Ok(RunOutcome::Completed)
        });
        Ok(CommandResult::Execute(work))
    }
}

#[tokio::test]
async fn a_message_during_host_work_is_busy() {
    let kit = RoomKit::new(Default::default());
    let (channel, _) = steering_room(&kit, vec![]).await;
    let mut view = config();
    view.commands.push(CommandSpec::new("deploy", "Deploy"));
    let release = Arc::new(Notify::new());
    let handler = Holding {
        release: release.clone(),
    };
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let session = LocalSession::new(kit.clone(), channel, view);
    let task = tokio::spawn(session.run_with_commands(host, handler));
    next(&client).await;
    let invoke = Command::Invoke {
        name: "deploy".into(),
        arguments: String::new(),
    };
    client
        .try_send(Request::new(RequestId::new(1), "deploy", "room", invoke))
        .unwrap();
    while !matches!(next(&client).await, Event::RunStarted { .. }) {}
    client.try_send(submit(2, "two", "Status?")).unwrap();
    let refused = reply(&client, 2).await.map(|rejection| rejection.code);
    assert_eq!(refused, Some(ErrorCode::Busy), "Host work is no turn");
    release.notify_one();
    collect_operation(&client).await;
    drop(client);
    task.await.unwrap().unwrap();
    channel_idle(&kit).await;
}

async fn channel_idle(kit: &RoomKit) {
    kit.close().await.unwrap();
}

/// A host that admits only the first message it is asked about, and records
/// the outcomes it is told.
#[derive(Debug, Default)]
struct AdmitsOnce {
    asked: AtomicUsize,
    finished: Mutex<Vec<TurnOutcome>>,
}

impl SessionLifecycle for AdmitsOnce {
    fn admit<'a>(
        &'a self,
        _: &'a SessionConfig,
        _: &'a OperationId,
        _: &'a roomkit::model::EventContent,
    ) -> LifecycleFuture<'a, Result<(), Rejection>> {
        let first = self.asked.fetch_add(1, Ordering::SeqCst) == 0;
        Box::pin(async move {
            if first {
                Ok(())
            } else {
                Err(Rejection::refused("Messages are paused for this room"))
            }
        })
    }

    fn finished<'a>(
        &'a self,
        _: &'a SessionConfig,
        _: &'a OperationId,
        outcome: TurnOutcome,
    ) -> LifecycleFuture<'a, ()> {
        self.finished.lock().unwrap().push(outcome);
        Box::pin(async {})
    }
}

/// The security review of 5374eb1: a message for the running turn skipped
/// the host's admission, which every message passes before entering the
/// room's history.
#[tokio::test]
async fn a_message_for_the_running_turn_passes_the_host_s_admission() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![
        ScriptedRound::calling("Checking", [("call1", "lookup_record", "{}")]),
        ScriptedRound::answering("Done"),
    ];
    let (channel, provider) = steering_room(&kit, rounds).await;
    let (open, gate) = watch::channel(false);
    let entered = Arc::new(Notify::new());
    hold_tools(&kit, gate, entered.clone());
    let lifecycle = Arc::new(AdmitsOnce::default());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let session =
        LocalSession::new(kit.clone(), channel, config()).with_lifecycle(lifecycle.clone());
    let task = tokio::spawn(session.run(host));
    next(&client).await;
    client
        .try_send(submit(1, "one", "Look up record 42"))
        .unwrap();
    entered.notified().await;
    client.try_send(submit(2, "two", "Paused words")).unwrap();
    let refusal = loop {
        match next(&client).await {
            Event::Notice { text, .. } => break text,
            Event::MessageSteered { .. } => panic!("A refused message was handed over"),
            _ => {}
        }
    };
    assert!(refusal.contains("Messages are paused"), "{refusal}");
    open.send_replace(true);
    collect_operation(&client).await;
    assert!(!asked_with(&provider, 1, "Paused words"));
    let rows = kit
        .get_timeline(&config().room, GetTimelineOptions::default())
        .await
        .unwrap();
    assert!(
        !rows
            .iter()
            .any(|row| roomkit_core::rules::transcoding::text_of(&row.content) == "Paused words"),
        "A refused message never enters the history"
    );
    assert_eq!(lifecycle.asked.load(Ordering::SeqCst), 2);
    drop(client);
    task.await.unwrap().unwrap();
    channel_idle(&kit).await;
}

fn answer(id: u64, operation: &str, agent: &str) -> Request {
    Request::new(
        RequestId::new(id),
        operation,
        "room",
        Command::Answer {
            agent: agent.into(),
        },
    )
}

/// RMK-1311: an answer is no message: another agent's is refused, and one
/// during a turn is busy, never handed to that turn.
#[tokio::test]
async fn an_answer_is_refused_for_another_agent_and_busy_during_a_turn() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![
        ScriptedRound::calling("Checking", [("call1", "lookup_record", "{}")]),
        ScriptedRound::answering("Done"),
    ];
    let (channel, provider) = steering_room(&kit, rounds).await;
    let (open, gate) = watch::channel(false);
    let entered = Arc::new(Notify::new());
    hold_tools(&kit, gate, entered.clone());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    next(&client).await;

    client.try_send(answer(1, "other", "someone")).unwrap();
    let refused = reply(&client, 1).await.map(|rejection| rejection.code);
    assert_eq!(refused, Some(ErrorCode::Invalid));

    client
        .try_send(submit(2, "one", "Look up record 42"))
        .unwrap();
    entered.notified().await;
    client
        .try_send(answer(3, "two", config().agent.as_str()))
        .unwrap();
    let refused = reply(&client, 3).await.map(|rejection| rejection.code);
    assert_eq!(refused, Some(ErrorCode::Busy));
    open.send_replace(true);
    collect_operation(&client).await;
    assert_eq!(
        provider.requests().len(),
        2,
        "No turn for the refused answer"
    );
    drop(client);
    task.await.unwrap().unwrap();
    channel_idle(&kit).await;
}
