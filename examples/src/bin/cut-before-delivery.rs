//! A real engine for `scripts/check-steering.py` (RMK-1371). The room's delivery
//! lane keeps a message written during a turn from reaching that turn, so
//! Ctrl+S cuts the turn before it can take the message, and RoomKit gives the
//! message a turn of its own. The lane opens once the host has refused the
//! terminal's first `Answer`: the terminal has asked, and that turn must end
//! its asking. The model's third answer shows if it does not.

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::Duration;

use roomkit::model::{
    ChannelBinding, ChannelId, ChannelOutput, ChannelType, HookResult, InboundMessage, RoomContext,
    RoomEvent, RoomId,
};
use roomkit::{
    Channel, ChannelError, CreateRoomOptions, HookOptions, HookTrigger, RoomKit,
    default_inbound_event,
};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{AiChannel, AiChannelOptions, AiTool, ToolEntry, ToolHandler, ToolResult};
use roomkit_tui::protocol::{Command, Event};
use roomkit_tui::render::Renderers;
use roomkit_tui::{Client, Host, connection, run};
use roomkit_tui_engine::{ConversationChannel, LocalSession, SessionConfig};
use serde_json::json;
use tokio::sync::watch;

type Failure = Box<dyn std::error::Error + Send + Sync>;

/// A transport that holds the delivery of the agent's first row until `lane`
/// opens: a set keeps the room's lane until its targets answered, so the set of
/// a message committed since waits behind it.
struct HoldsTheLane {
    id: ChannelId,
    agent: ChannelId,
    lane: watch::Receiver<bool>,
    held: AtomicBool,
}

impl Channel for HoldsTheLane {
    fn channel_id(&self) -> ChannelId {
        self.id.clone()
    }

    fn channel_type(&self) -> ChannelType {
        ChannelType::WEBSOCKET
    }

    async fn handle_inbound(
        &self,
        message: InboundMessage,
        context: RoomContext,
    ) -> Result<RoomEvent, ChannelError> {
        Ok(default_inbound_event(self, &message, &context))
    }

    async fn deliver(
        &self,
        event: RoomEvent,
        _binding: ChannelBinding,
        _context: RoomContext,
    ) -> Result<ChannelOutput, ChannelError> {
        if event.source.channel_id == self.agent && !self.held.swap(true, Ordering::SeqCst) {
            let mut lane = self.lane.clone();
            drop(lane.wait_for(|open| *open).await);
        }
        Ok(ChannelOutput::empty())
    }
}

/// The agent that steers its room's turns, its first call waiting for good, as
/// it would wait for a person's approval.
fn agent(id: ChannelId) -> Result<AiChannel<ScriptedProvider>, Failure> {
    let provider = ScriptedProvider::new([
        ScriptedRound::calling("Checking", [("call1", "lookup_record", "{}")]),
        ScriptedRound::answering("Its own answer"),
        ScriptedRound::answering("Answered twice"),
    ]);
    let lookup = AiTool {
        name: "lookup_record".to_owned(),
        description: "Read a record".to_owned(),
        parameters: json!({"type":"object"}),
        tags: Vec::new(),
        changes_state: false,
    };
    let options = AiChannelOptions {
        tools: vec![ToolEntry::Definition(lookup)],
        tool_handler: Some(ToolHandler::new(|_, _| async {
            Ok(ToolResult::Json(json!({"record": "42"})))
        })),
        steer_room_turns: true,
        ..Default::default()
    };
    Ok(AiChannel::new(id, provider, options)?)
}

async fn serve(host: Host, lane: watch::Receiver<bool>) -> Result<(), Failure> {
    let kit = RoomKit::new(Default::default());
    let id = ChannelId::new("agent")?;
    kit.register_channel(agent(id.clone())?)?;
    let console = ConversationChannel::new(ChannelId::new("console")?);
    kit.register_channel(console.clone())?;
    let slow = HoldsTheLane {
        id: ChannelId::new("slow")?,
        agent: id.clone(),
        lane,
        held: AtomicBool::new(false),
    };
    let slow_id = slow.id.clone();
    kit.register_channel(slow)?;
    let room = RoomId::new("ops")?;
    kit.create_room(CreateRoomOptions {
        room_id: Some(room.clone()),
        ..Default::default()
    })
    .await?;
    for channel in [console.id(), &id, &slow_id] {
        kit.attach_channel(&room, channel, Default::default())
            .await?;
    }
    let hold = |_: roomkit::ToolCallEvent, _| async {
        std::future::pending::<()>().await;
        Ok(HookResult::allow(Default::default()))
    };
    let options = HookOptions {
        name: "call-waits".to_owned(),
        timeout: Duration::from_secs(300),
        ..Default::default()
    };
    kit.hook(HookTrigger::BeforeToolUse, hold, options)?;
    let config = SessionConfig::new(room, id, "alice");
    LocalSession::new(kit, console, config).run(host).await?;
    Ok(())
}

/// Pass requests and events between the terminal and the engine, and open the
/// lane once the host has refused an `Answer`.
fn relay(terminal: &Host, engine: &Client, lane: &watch::Sender<bool>) {
    let mut answers = Vec::new();
    loop {
        match terminal.recv_timeout(Duration::from_millis(1)) {
            Ok(request) => {
                if matches!(request.command, Command::Answer { .. }) {
                    answers.push(request.id);
                }
                drop(engine.try_send(request));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        while let Ok(event) = engine.try_recv() {
            if let Event::Reply {
                request,
                error: Some(_),
            } = &event
                && answers.contains(request)
            {
                lane.send_replace(true);
            }
            drop(terminal.try_send(event));
        }
    }
}

fn main() -> Result<(), Failure> {
    let capacity = NonZeroUsize::new(256).ok_or("A connection holds events")?;
    let (client, terminal) = connection(capacity);
    let (engine, host) = connection(capacity);
    let (open, lane) = watch::channel(false);
    thread::spawn(move || relay(&terminal, &engine, &open));
    let runtime = tokio::runtime::Runtime::new()?;
    let served = runtime.spawn(serve(host, lane));
    run(client, Renderers::default())?;
    if served.is_finished() {
        runtime.block_on(served)??;
    }
    runtime.shutdown_background();
    Ok(())
}
