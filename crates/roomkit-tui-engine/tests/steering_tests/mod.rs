//! RMK-1161: a message sent while the agent's turn runs goes to that turn.

use super::*;
use roomkit::model::{HookResult, RoomEvent};
use roomkit_tui_engine::{
    CommandExecution, CommandHandler, CommandResult, LifecycleFuture, SessionLifecycle, TurnOutcome,
};
use std::sync::Mutex;
use tokio::sync::{Notify, watch};

/// An agent that steers its room's turns, answering `rounds`, beside a
/// streaming console.
async fn steering_room(
    kit: &RoomKit,
    rounds: Vec<ScriptedRound>,
) -> (ConversationChannel, ScriptedProvider) {
    let provider = ScriptedProvider::new(rounds);
    let lookup = AiTool {
        name: "lookup_record".to_owned(),
        description: "Read a fictional record".to_owned(),
        parameters: json!({"type":"object","properties":{},"additionalProperties":false}),
    };
    let options = AiChannelOptions {
        tools: vec![ToolEntry::Definition(lookup)],
        tool_handler: Some(ToolHandler::new(|_, _| async {
            Ok(ToolResult::Json(json!({"record":"42"})))
        })),
        steer_room_turns: true,
        ..Default::default()
    };
    let agent = AiChannel::new(config().agent, provider.clone(), options).unwrap();
    kit.register_channel(agent).unwrap();
    let channel = ConversationChannel::new(ChannelId::new("console").unwrap());
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

fn steering() -> SessionConfig {
    let mut config = config();
    config.can_steer = true;
    config
}

/// Whether the model was asked with `text` among its messages.
fn asked_with(provider: &ScriptedProvider, round: usize, text: &str) -> bool {
    provider.requests()[round]
        .messages
        .iter()
        .any(|message| message.text().contains(text))
}

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
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, steering()).run(host));
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

/// The agent read the room's last message after it answered: its turn
/// ended before the steered message could reach it.
#[tokio::test]
async fn a_message_the_turn_no_longer_reads_gets_its_own_turn() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![
        ScriptedRound::answering("First answer"),
        ScriptedRound::answering("Second answer"),
    ];
    let (channel, provider) = steering_room(&kit, rounds).await;
    // Hold the first answer as it is stored: the turn's loop is over.
    let (open, gate) = watch::channel(false);
    let entered = Arc::new(Notify::new());
    let (held, agent) = (entered.clone(), config().agent);
    kit.hook(
        roomkit::HookTrigger::BeforeBroadcast,
        move |event: RoomEvent, _| {
            let (mut gate, held) = (gate.clone(), held.clone());
            let answer = event.source.channel_id == agent;
            async move {
                if answer {
                    held.notify_one();
                    let _ = gate.wait_for(|open| *open).await;
                }
                Ok(HookResult::allow(Default::default()))
            }
        },
        hook_options("hold-answer"),
    )
    .unwrap();
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, steering()).run(host));
    next(&client).await;
    client.try_send(submit(1, "one", "First question")).unwrap();
    entered.notified().await;
    client
        .try_send(submit(2, "two", "Second question"))
        .unwrap();
    assert_eq!(reply(&client, 2).await, None);
    open.send_replace(true);
    let mut steered = None;
    let answered = loop {
        match next(&client).await {
            Event::Item {
                item: Item::Message(message),
                ..
            } if message.text == "Second question" => steered = Some(message.id),
            Event::RunStarted {
                answers: Some(message),
                ..
            } if Some(&message) == steered.as_ref() => break message,
            _ => {}
        }
    };
    assert_eq!(Some(answered), steered, "Its own turn names it");
    loop {
        if let Event::MessageFinalized { message, .. } = next(&client).await
            && message.text == "Second answer"
        {
            break;
        }
    }
    assert!(asked_with(&provider, 1, "Second question"));
    drop(client);
    task.await.unwrap().unwrap();
    channel_idle(&kit).await;
}

#[tokio::test]
async fn a_message_during_a_turn_is_busy_unless_the_agent_steers() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![
        ScriptedRound::calling("Checking", [("call1", "lookup_record", "{}")]),
        ScriptedRound::answering("Done"),
    ];
    let (channel, _) = steering_room(&kit, rounds).await;
    let (open, gate) = watch::channel(false);
    let entered = Arc::new(Notify::new());
    hold_tools(&kit, gate, entered.clone());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    // The host did not say its agent steers.
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    next(&client).await;
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
    let mut view = steering();
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
        LocalSession::new(kit.clone(), channel, steering()).with_lifecycle(lifecycle.clone());
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
