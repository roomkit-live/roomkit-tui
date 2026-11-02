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
use std::time::{Duration, Instant};

use roomkit::model::{ChannelId, RoomId};
use roomkit::{CreateRoomOptions, HookOptions, RoomKit};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{AiChannel, AiChannelOptions, AiTool, ToolEntry, ToolHandler, ToolResult};
use roomkit_tui::{Client, Session, Steering, connection};
use roomkit_tui_engine::{ConversationChannel, LocalSession, SessionConfig};
use roomkit_tui_protocol::*;
use serde_json::json;
use tokio::sync::watch;

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
    let mut view = SessionConfig::new(room, agent, "alice");
    view.can_steer = true;
    (console, view)
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
fn write(client: &Client, ui: &mut Session, text: &str) {
    ui.set_draft(text);
    let command = ui.input_command().expect("the terminal sends the message");
    let request = ui.request(command).unwrap();
    client.try_send(request.clone()).unwrap();
    ui.sent(&request);
}

fn message<'a>(ui: &'a Session, text: &str) -> Option<&'a Message> {
    ui.snapshot()?.items.iter().find_map(|item| match item {
        Item::Message(message) if message.text == text => Some(message),
        _ => None,
    })
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
    let task = tokio::spawn(LocalSession::new(kit.clone(), console, view).run(host));
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

    write(&client, &mut ui, "Use the second ledger");
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

    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}
