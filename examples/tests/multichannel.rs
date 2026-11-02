//! A person's host work and an agent turn for another channel's message share
//! a room: the real engine and the terminal's own session, joined by the
//! transport, keep the work's approvals visible and answerable throughout.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "Invalid example state must fail these tests"
)]

use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use roomkit::model::{ChannelId, EventContent, InboundMessage, RoomId, TextContent};
use roomkit::{CreateRoomOptions, HookOptions, ProcessInboundOptions, RoomKit};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{AiChannel, AiChannelOptions, AiTool, ToolEntry, ToolHandler, ToolResult};
use roomkit_tui::{Client, Session, connection};
use roomkit_tui_engine::{
    CommandExecution, CommandHandler, CommandResult, ConversationChannel, LocalSession,
    PermissionRequest, SessionConfig,
};
use roomkit_tui_protocol::*;
use serde_json::json;
use tokio::sync::watch;

/// `/deploy` asks three times through its context, then shows the answers.
struct Deployer;

impl CommandHandler for Deployer {
    async fn handle(&self, _: Request, _: SessionConfig) -> Result<CommandResult, Rejection> {
        let work = CommandExecution::new("deploy-1".into(), |context| async move {
            let mut answers = Vec::new();
            for step in ["Deploy", "Migrate", "Announce"] {
                let request = PermissionRequest::new(
                    format!("{step}?"),
                    format!("{step} in production"),
                    Duration::from_secs(10),
                );
                answers.push(format!("{:?}", context.ask_permission(request).await));
            }
            let text = answers.join("/");
            context.publish(Item::Message(Message::new("answer", "agent", text)));
            Ok(RunOutcome::Completed)
        });
        Ok(CommandResult::Execute(work))
    }
}

/// A hook named `name`, allowed long enough for a person to decide.
fn hook_options(name: &str) -> HookOptions {
    HookOptions {
        name: name.to_owned(),
        timeout: Duration::from_secs(300),
        ..Default::default()
    }
}

/// The agent checks a record, held at its tool call until `gate` opens.
async fn room(kit: &RoomKit, gate: watch::Receiver<bool>) -> (ConversationChannel, SessionConfig) {
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
        ..Default::default()
    };
    let provider = ScriptedProvider::new([
        ScriptedRound::calling("Checking", [("call1", "lookup_record", "{}")]),
        ScriptedRound::answering("Record 42 is ready"),
    ]);
    let agent = ChannelId::new("agent").unwrap();
    kit.register_channel(AiChannel::new(agent.clone(), provider, options).unwrap())
        .unwrap();
    let console = ConversationChannel::new(ChannelId::new("console").unwrap());
    kit.register_channel(console.clone()).unwrap();
    let sms = ConversationChannel::buffered(ChannelId::new("sms").unwrap());
    kit.register_channel(sms.clone()).unwrap();
    let room = RoomId::new("ops").unwrap();
    kit.create_room(CreateRoomOptions {
        room_id: Some(room.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    for id in [console.id(), &agent, sms.id()] {
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
        hook_options("hold"),
    )
    .unwrap();
    let mut view = SessionConfig::new(room, agent, "alice");
    view.commands.push(CommandSpec::new("deploy", "Deploy"));
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

fn send(client: &Client, ui: &mut Session, command: Command) {
    let request = ui.request(command).unwrap();
    client.try_send(request.clone()).unwrap();
    ui.sent(&request);
}

/// Allow the request on screen, as the approval card's key would.
fn approve(client: &Client, ui: &mut Session) {
    let decision = ui.decide(Decision::Allow).expect("a request on screen");
    send(client, ui, decision);
}

fn asked(ui: &Session, title: &str) -> bool {
    ui.permissions().iter().any(|p| p.title == title)
}

fn item(ui: &Session, found: impl Fn(&Item) -> bool) -> bool {
    ui.snapshot().is_some_and(|s| s.items.iter().any(found))
}

/// RMK-1135: a turn for another channel's message replaced the person's
/// `/deploy` in the terminal: the work's requests were no longer shown and
/// expired, and Esc aimed at the turn.
#[tokio::test]
async fn a_turn_for_another_channel_leaves_the_work_s_approvals_on_screen() {
    let kit = RoomKit::new(Default::default());
    let (open, gate) = watch::channel(false);
    let (console, view) = room(&kit, gate).await;
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let session = LocalSession::new(kit.clone(), console, view.clone());
    let task = tokio::spawn(session.run_with_commands(host, Deployer));
    let mut ui = Session::default();
    until(&client, &mut ui, "the conversation", |ui| {
        ui.snapshot().is_some()
    })
    .await;
    let deploy = Command::Invoke {
        name: "deploy".into(),
        arguments: String::new(),
    };
    send(&client, &mut ui, deploy);
    until(&client, &mut ui, "the first request", |ui| {
        asked(ui, "Deploy?")
    })
    .await;

    let text = EventContent::Text(TextContent::new("Status?"));
    let mut message = InboundMessage::new(ChannelId::new("sms").unwrap(), "carol", text);
    message.addressed_to = Some(vec![view.agent.clone()]);
    let options = ProcessInboundOptions {
        room_id: Some(view.room.clone()),
        organization_id: None,
        defer_delivery: false,
    };
    let inbound = {
        let kit = kit.clone();
        tokio::spawn(async move { kit.process_inbound(message, options).await.map(|_| ()) })
    };
    let turn_tool =
        |item: &Item| matches!(item, Item::Tool { run, .. } if run.as_str() != "deploy-1");
    until(&client, &mut ui, "the other channel's turn", |ui| {
        item(ui, turn_tool)
    })
    .await;
    let deploy_run = RunId::from("deploy-1");
    assert_eq!(ui.active_run(), Some(&deploy_run), "Esc follows the work");

    approve(&client, &mut ui);
    until(&client, &mut ui, "a request during the turn", |ui| {
        asked(ui, "Migrate?")
    })
    .await;
    // The first card is gone: `decide` resolves it before the work, on this
    // single-threaded runtime, can ask again.
    assert_eq!(ui.permissions().len(), 1);

    open.send(true).unwrap();
    let reply = |item: &Item| matches!(item, Item::Message(m) if m.text == "Record 42 is ready");
    until(&client, &mut ui, "the turn's answer", |ui| item(ui, reply)).await;
    inbound.await.unwrap().unwrap();
    assert_eq!(ui.active_run(), Some(&deploy_run), "The work still runs");
    approve(&client, &mut ui);
    until(&client, &mut ui, "a request after the turn", |ui| {
        asked(ui, "Announce?")
    })
    .await;
    approve(&client, &mut ui);
    let done = |item: &Item| matches!(item, Item::Message(m) if m.text == "Allow/Allow/Allow");
    until(&client, &mut ui, "the work's answers", |ui| item(ui, done)).await;

    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}
