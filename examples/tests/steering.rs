//! RMK-1161: a person writes while the agent's turn runs. The real engine
//! and the terminal's own session, joined by the transport, hand the message
//! to that turn, which reads it at its next step, and the terminal shows
//! where it went.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "Invalid example state must fail these tests"
)]

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use roomkit::model::{ChannelId, RoomId};
use roomkit::{CreateRoomOptions, HookOptions, RoomKit};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{AiChannel, AiChannelOptions, AiTool, ToolEntry, ToolHandler, ToolResult};
use roomkit_tui::{Client, Session, Steering, connection};
use roomkit_tui_engine::{
    ConversationChannel, LifecycleFuture, LocalSession, SessionConfig, SessionLifecycle,
    TurnOutcome,
};
use roomkit_tui_protocol::*;
use serde_json::json;
use tokio::sync::{Notify, watch};

/// The agent steers its room's turns; its tool call waits until `gate` opens.
async fn room(
    kit: &RoomKit,
    provider: ScriptedProvider,
    gate: watch::Receiver<bool>,
) -> (ConversationChannel, SessionConfig) {
    let lookup = AiTool {
        name: "lookup_record".to_owned(),
        description: "Read a record".to_owned(),
        parameters: json!({"type":"object"}),
    };
    let options = AiChannelOptions {
        tools: vec![ToolEntry::Definition(lookup)],
        tool_handler: Some(ToolHandler::new(|_, _| async {
            Ok(ToolResult::Json(json!({"record": "42"})))
        })),
        steer_room_turns: true,
        ..Default::default()
    };
    let agent = ChannelId::new("agent").unwrap();
    kit.register_channel(AiChannel::new(agent.clone(), provider, options).unwrap())
        .unwrap();
    let console = ConversationChannel::new(ChannelId::new("console").unwrap());
    kit.register_channel(console.clone()).unwrap();
    let room = RoomId::new("ops").unwrap();
    kit.create_room(CreateRoomOptions {
        room_id: Some(room.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    for id in [console.id(), &agent] {
        kit.attach_channel(&room, id, Default::default())
            .await
            .unwrap();
    }
    kit.hook(
        roomkit::HookTrigger::BeforeToolUse,
        move |_: roomkit::ToolCallEvent, _| {
            let mut gate = gate.clone();
            async move {
                let _ = gate.wait_for(|open| *open).await;
                Ok(roomkit::model::HookResult::allow(Default::default()))
            }
        },
        HookOptions {
            name: "hold".to_owned(),
            timeout: Duration::from_secs(300),
            ..Default::default()
        },
    )
    .unwrap();
    (console, SessionConfig::new(room, agent, "alice"))
}

/// Apply the host's events to the terminal until `done` holds.
async fn until(client: &Client, ui: &mut Session, what: &str, done: impl Fn(&Session) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        while let Ok(event) = client.try_recv() {
            ui.apply(event);
        }
        if done(ui) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "The terminal never showed {what}"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

/// Type `text` in the composer and press Enter, as the terminal's key would.
/// Returns the request the message went as.
fn write(client: &Client, ui: &mut Session, text: &str) -> Request {
    ui.set_draft(text);
    let command = ui.input_command().expect("the terminal sends the message");
    let request = ui.request(command).unwrap();
    client.try_send(request.clone()).unwrap();
    ui.sent(&request);
    request
}

/// Apply the host's events to the terminal until the host answers `request`.
async fn replied(client: &Client, ui: &mut Session, request: &Request) -> Option<Rejection> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        while let Ok(event) = client.try_recv() {
            let reply = match &event {
                Event::Reply { request: id, error } if *id == request.id => Some(error.clone()),
                _ => None,
            };
            ui.apply(event);
            if let Some(error) = reply {
                return error;
            }
        }
        assert!(Instant::now() < deadline, "The host never answered");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn message<'a>(ui: &'a Session, text: &str) -> Option<&'a Message> {
    ui.snapshot()?.items.iter().find_map(|item| match item {
        Item::Message(message) if message.text == text => Some(message),
        _ => None,
    })
}

/// A host that records what it is told, per operation.
#[derive(Debug, Default)]
struct Host(Mutex<Vec<(String, &'static str)>>);

impl Host {
    fn record(&self, operation: &OperationId, call: &'static str) {
        let operation = operation.as_str().to_owned();
        self.0.lock().unwrap().push((operation, call));
    }

    /// What the host was told about `operation`, once told how it ended.
    async fn told(&self, operation: &str) -> Vec<&'static str> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let told: Vec<_> = self
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|(id, _)| id == operation)
                .map(|(_, call)| *call)
                .collect();
            if told.last() == Some(&"finished") {
                return told;
            }
            assert!(
                Instant::now() < deadline,
                "The host never heard {operation} end"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
}

impl SessionLifecycle for Host {
    fn admit<'a>(
        &'a self,
        _: &'a SessionConfig,
        operation: &'a OperationId,
        _: &'a roomkit::model::EventContent,
    ) -> LifecycleFuture<'a, Result<(), Rejection>> {
        self.record(operation, "admit");
        Box::pin(async { Ok(()) })
    }

    fn started<'a>(
        &'a self,
        _: &'a SessionConfig,
        operation: &'a OperationId,
    ) -> LifecycleFuture<'a, ()> {
        self.record(operation, "started");
        Box::pin(async {})
    }

    fn finished<'a>(
        &'a self,
        _: &'a SessionConfig,
        operation: &'a OperationId,
        _: TurnOutcome,
    ) -> LifecycleFuture<'a, ()> {
        self.record(operation, "finished");
        Box::pin(async {})
    }
}

#[tokio::test]
async fn a_message_written_during_a_tool_round_reaches_the_running_turn() {
    let kit = RoomKit::new(Default::default());
    let provider = ScriptedProvider::new([
        ScriptedRound::calling("Checking", [("call1", "lookup_record", "{}")]),
        ScriptedRound::answering("Record 42 is in the second ledger"),
    ]);
    let (open, gate) = watch::channel(false);
    let (console, view) = room(&kit, provider.clone(), gate).await;
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let lifecycle = Arc::new(Host::default());
    let session = LocalSession::new(kit.clone(), console, view).with_lifecycle(lifecycle.clone());
    let task = tokio::spawn(session.run(host));
    let mut ui = Session::default();
    until(&client, &mut ui, "the conversation", |ui| {
        ui.snapshot().is_some()
    })
    .await;

    write(&client, &mut ui, "Look up record 42");
    let tool = |item: &Item| matches!(item, Item::Tool { .. });
    until(&client, &mut ui, "the turn's tool call", |ui| {
        ui.snapshot().is_some_and(|s| s.items.iter().any(tool))
    })
    .await;
    assert!(ui.active_run().is_some(), "The turn runs");

    let steered = write(&client, &mut ui, "Use the second ledger");
    until(&client, &mut ui, "the message handed to the turn", |ui| {
        message(ui, "Use the second ledger")
            .is_some_and(|m| ui.steering(&m.id) == Some(Steering::Handed))
    })
    .await;

    open.send(true).unwrap();
    until(&client, &mut ui, "the turn's answer", |ui| {
        message(ui, "Record 42 is in the second ledger").is_some()
    })
    .await;
    until(&client, &mut ui, "the room idle", |ui| {
        ui.active_run().is_none()
    })
    .await;
    let sent = message(&ui, "Use the second ledger").unwrap();
    assert_eq!(ui.steering(&sent.id), Some(Steering::Handed));
    let asked = &provider.requests()[1];
    assert!(
        asked
            .messages
            .iter()
            .any(|m| m.text().contains("Use the second ledger")),
        "The turn read the message at its next step"
    );
    assert_eq!(provider.requests().len(), 2, "No turn of its own");
    let told = lifecycle.told(steered.operation.as_str()).await;
    assert_eq!(told, ["admit", "finished"], "It started nothing");

    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

/// Hold the agent's first answer as it is stored, telling `held` once it
/// waits: that turn's loop is over and reads no more messages.
fn hold_first_answer(kit: &RoomKit, gate: watch::Receiver<bool>, held: Arc<Notify>) {
    let first = Arc::new(AtomicUsize::new(0));
    kit.hook(
        roomkit::HookTrigger::BeforeBroadcast,
        move |event: roomkit::model::RoomEvent, _| {
            let (mut gate, held) = (gate.clone(), held.clone());
            let agent = event.source.channel_id.as_str() == "agent";
            let hold = agent && first.fetch_add(1, Ordering::SeqCst) == 0;
            async move {
                if hold {
                    held.notify_one();
                    let _ = gate.wait_for(|open| *open).await;
                }
                Ok(roomkit::model::HookResult::allow(Default::default()))
            }
        },
        HookOptions {
            name: "hold-answer".to_owned(),
            timeout: Duration::from_secs(300),
            ..Default::default()
        },
    )
    .unwrap();
}

/// RMK-1232: written as the turn writes its last answer, the message gets a
/// turn of its own, which the host hears start as any turn.
#[tokio::test]
async fn a_message_the_turn_no_longer_reads_gets_a_turn_the_host_hears_start() {
    let kit = RoomKit::new(Default::default());
    let provider = ScriptedProvider::new([
        ScriptedRound::answering("First answer"),
        ScriptedRound::answering("Second answer"),
    ]);
    let (_tools, tools_gate) = watch::channel(true);
    let (console, view) = room(&kit, provider.clone(), tools_gate).await;
    let (open, gate) = watch::channel(false);
    let held = Arc::new(Notify::new());
    hold_first_answer(&kit, gate, held.clone());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let lifecycle = Arc::new(Host::default());
    let session = LocalSession::new(kit.clone(), console, view).with_lifecycle(lifecycle.clone());
    let task = tokio::spawn(session.run(host));
    let mut ui = Session::default();
    until(&client, &mut ui, "the conversation", |ui| {
        ui.snapshot().is_some()
    })
    .await;

    write(&client, &mut ui, "First question");
    held.notified().await;
    until(&client, &mut ui, "the turn writing its answer", |ui| {
        ui.active_run().is_some()
    })
    .await;
    let steered = write(&client, &mut ui, "Second question");
    let accepted = replied(&client, &mut ui, &steered).await;
    assert_eq!(accepted, None, "Accepted for the running turn");

    // The held answer also holds the room: the message is committed once
    // the turn's last answer is.
    open.send(true).unwrap();
    until(&client, &mut ui, "the answer of its own turn", |ui| {
        message(ui, "Second answer").is_some()
    })
    .await;
    until(&client, &mut ui, "the room idle", |ui| {
        ui.active_run().is_none()
    })
    .await;
    let sent = message(&ui, "Second question").unwrap();
    assert_eq!(ui.steering(&sent.id), Some(Steering::OwnTurn));
    let told = lifecycle.told(steered.operation.as_str()).await;
    assert_eq!(
        told,
        ["admit", "started", "finished"],
        "Started as any turn"
    );

    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

/// A host that closes one message slowly, once told which: until `gate`
/// opens, the engine holds the room with no run to show.
#[derive(Debug)]
struct SlowClose {
    host: Host,
    held: Mutex<Option<String>>,
    gate: watch::Receiver<bool>,
}

impl SessionLifecycle for SlowClose {
    fn admit<'a>(
        &'a self,
        session: &'a SessionConfig,
        operation: &'a OperationId,
        content: &'a roomkit::model::EventContent,
    ) -> LifecycleFuture<'a, Result<(), Rejection>> {
        self.host.admit(session, operation, content)
    }

    fn started<'a>(
        &'a self,
        session: &'a SessionConfig,
        operation: &'a OperationId,
    ) -> LifecycleFuture<'a, ()> {
        self.host.started(session, operation)
    }

    fn finished<'a>(
        &'a self,
        session: &'a SessionConfig,
        operation: &'a OperationId,
        outcome: TurnOutcome,
    ) -> LifecycleFuture<'a, ()> {
        let held = self.held.lock().unwrap().as_deref() == Some(operation.as_str());
        let mut gate = self.gate.clone();
        Box::pin(async move {
            if held {
                let _ = gate.wait_for(|open| *open).await;
            }
            self.host.finished(session, operation, outcome).await;
        })
    }
}

/// RMK-1303: Ctrl+S cuts everything, a message's own turn included. The host
/// then holds the room while it closes that turn, with no run to show: the
/// next message is refused as busy, before any host hook, and taken once the
/// room is free. The cut message stays in the conversation: the next turn
/// reads it with its own.
#[tokio::test]
async fn a_message_whose_own_turn_is_cut_is_read_by_the_next_turn() {
    let kit = RoomKit::new(Default::default());
    let provider = ScriptedProvider::new([
        ScriptedRound::answering("First answer"),
        ScriptedRound::calling("Checking", [("call1", "lookup_record", "{}")]),
        ScriptedRound::answering("Both questions answered"),
    ]);
    let (_tools, tools_gate) = watch::channel(false);
    let (console, view) = room(&kit, provider.clone(), tools_gate).await;
    let (open, gate) = watch::channel(false);
    let held = Arc::new(Notify::new());
    hold_first_answer(&kit, gate, held.clone());
    let (close, closing) = watch::channel(false);
    let lifecycle = Arc::new(SlowClose {
        host: Host::default(),
        held: Mutex::new(None),
        gate: closing,
    });
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let session = LocalSession::new(kit.clone(), console, view).with_lifecycle(lifecycle.clone());
    let task = tokio::spawn(session.run(host));
    let mut ui = Session::default();
    until(&client, &mut ui, "the conversation", |ui| {
        ui.snapshot().is_some()
    })
    .await;

    write(&client, &mut ui, "First question");
    held.notified().await;
    until(&client, &mut ui, "the turn writing its answer", |ui| {
        ui.active_run().is_some()
    })
    .await;
    let steered = write(&client, &mut ui, "Second question");
    *lifecycle.held.lock().unwrap() = Some(steered.operation.as_str().to_owned());
    assert_eq!(replied(&client, &mut ui, &steered).await, None);
    open.send(true).unwrap();

    // Its own turn runs, and waits at its tool call.
    let tool = |item: &Item| matches!(item, Item::Tool { .. });
    until(&client, &mut ui, "the own turn's call", |ui| {
        ui.snapshot().is_some_and(|s| s.items.iter().any(tool))
    })
    .await;
    let own = ui.active_run().cloned().expect("The own turn runs");
    let sent = message(&ui, "Second question").unwrap();
    assert_eq!(ui.steering(&sent.id), Some(Steering::OwnTurn));
    let interrupt = ui.request(Command::Interrupt { run: own }).unwrap();
    client.try_send(interrupt.clone()).unwrap();
    ui.sent(&interrupt);
    until(&client, &mut ui, "the own turn stopped", |ui| {
        ui.active_run().is_none()
    })
    .await;

    // The view shows nothing running, and the host still holds the room.
    let refused = write(&client, &mut ui, "Third question");
    let refusal = replied(&client, &mut ui, &refused).await.unwrap();
    assert_eq!(refusal.code, ErrorCode::Busy, "{refusal:?}");
    assert!(
        lifecycle
            .host
            .0
            .lock()
            .unwrap()
            .iter()
            .all(|(operation, _)| operation != refused.operation.as_str()),
        "A refused message reached the host's hooks"
    );

    // The room comes free with no event to say so: the view tries again.
    close.send(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let third = write(&client, &mut ui, "Third question");
        match replied(&client, &mut ui, &third).await {
            None => break,
            Some(refusal) => assert_eq!(refusal.code, ErrorCode::Busy, "{refusal:?}"),
        }
        assert!(Instant::now() < deadline, "The host stayed busy");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    until(&client, &mut ui, "the next turn's answer", |ui| {
        message(ui, "Both questions answered").is_some()
    })
    .await;
    let requests = provider.requests();
    assert_eq!(requests.len(), 3, "One request for each turn");
    for question in ["Second question", "Third question"] {
        assert!(
            requests[2]
                .messages
                .iter()
                .any(|m| m.text().contains(question)),
            "The next turn did not read {question:?}"
        );
    }

    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}
