//! Exercise actual RoomKit execution and storage through the adapter boundary.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "Assertions fail the fixture on invalid state"
)]

use roomkit::model::{ChannelId, RoomId};
use roomkit::{ConversationStore, CreateRoomOptions, GetTimelineOptions, RoomKit, RoomKitOptions};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{AiChannel, AiChannelOptions, AiTool, ToolEntry, ToolHandler, ToolResult};
use roomkit_tui_engine::{
    AdapterError, ConversationChannel, LocalSession, PermissionRequest, SessionConfig,
};
use roomkit_tui_protocol::*;
use roomkit_tui_transport::{Client, connection};
use serde_json::json;
use std::{
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

mod approval_tests;
mod command_tests;
mod delivery_tests;
mod identity_tests;
mod interruption_tests;
mod lifecycle_tests;
mod progress_tests;
mod request_tests;
mod review_tests;
mod steering_tests;
mod view_state_tests;

fn config() -> SessionConfig {
    SessionConfig::new(
        RoomId::new("room").unwrap(),
        ChannelId::new("agent").unwrap(),
        "alice",
    )
}

/// A hook named `name`, allowed long enough for a person to decide.
fn hook_options(name: &str) -> roomkit::HookOptions {
    roomkit::HookOptions {
        name: name.to_owned(),
        timeout: Duration::from_secs(300),
        ..Default::default()
    }
}

async fn prepare<S: ConversationStore>(
    kit: &RoomKit<S>,
    provider: impl roomkit_ai::AiProvider,
    calls: Arc<AtomicUsize>,
    buffered: bool,
) -> ConversationChannel {
    prepare_as(kit, &config().agent, provider, calls, buffered).await
}

/// `prepare`, with `agent` as the agent's channel identity.
async fn prepare_as<S: ConversationStore>(
    kit: &RoomKit<S>,
    agent: &ChannelId,
    provider: impl roomkit_ai::AiProvider,
    calls: Arc<AtomicUsize>,
    buffered: bool,
) -> ConversationChannel {
    let lookup = AiTool {
        name: "lookup_record".to_owned(),
        description: "Read a fictional record".to_owned(),
        parameters: json!({"type":"object","properties":{},"additionalProperties":false}),
        tags: Vec::new(),
    };
    let handler = ToolHandler::new(move |_, arguments| {
        assert!(
            arguments.is_empty(),
            "Only business arguments reach the handler"
        );
        calls.fetch_add(1, Ordering::SeqCst);
        async { Ok(ToolResult::Json(json!({"record":"42","status":"ready"}))) }
    });
    let options = AiChannelOptions {
        tools: vec![ToolEntry::Definition(lookup)],
        tool_handler: Some(handler),
        tool_call_intents: true,
        ..Default::default()
    };
    kit.register_channel(AiChannel::new(agent.clone(), provider, options).unwrap())
        .unwrap();
    let channel = if buffered {
        ConversationChannel::buffered(ChannelId::new("console").unwrap())
    } else {
        ConversationChannel::new(ChannelId::new("console").unwrap())
    };
    kit.register_channel(channel.clone()).unwrap();
    let room = config().room;
    kit.create_room(CreateRoomOptions {
        room_id: Some(room.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    for id in [channel.id(), agent] {
        kit.attach_channel(&room, id, Default::default())
            .await
            .unwrap();
    }
    channel
}

async fn next(client: &Client) -> Event {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match client.try_recv() {
                Ok(event) => return event,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    tokio::time::sleep(Duration::from_millis(1)).await
                }
                Err(error) => panic!("adapter disconnected: {error}"),
            }
        }
    })
    .await
    .expect("an adapter event before its deadline")
}

fn request(operation: &str) -> Request {
    Request::new(
        RequestId::new(1),
        operation,
        "room",
        Command::Submit {
            agent: "agent".into(),
            text: "Look up record 42".into(),
        },
    )
}

async fn collect_operation(client: &Client) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        let event = next(client).await;
        let ended = matches!(&event, Event::OperationFinished { .. });
        events.push(event);
        if ended {
            return events;
        }
    }
}

fn assert_operation(events: &[Event], operation: &str, outcome: RunOutcome) {
    let starts: Vec<_> = events.iter().enumerate().filter_map(|(index, event)| {
        matches!(event, Event::OperationStarted { conversation, operation: id } if conversation.as_str() == "room" && id.as_str() == operation).then_some(index)
    }).collect();
    assert_eq!(starts.len(), 1, "Exactly one operation start");
    assert!(
        matches!(events.last(), Some(Event::OperationFinished { conversation, operation: id, outcome: actual }) if conversation.as_str() == "room" && id.as_str() == operation && *actual == outcome)
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::OperationFinished { .. }))
            .count(),
        1
    );
    assert!(events.iter().enumerate().all(|(index, event)| !matches!(
        event,
        Event::RunStarted { .. } | Event::RunFinished { .. }
    ) || index > starts[0]));
}

struct NamedProvider(ScriptedProvider);
impl roomkit_ai::AiProvider for NamedProvider {
    fn name(&self) -> &str {
        "local"
    }
    fn model_name(&self) -> &str {
        "fixture"
    }
    fn supports_structured_streaming(&self) -> bool {
        self.0.supports_structured_streaming()
    }
    fn generate<'a>(
        &'a self,
        context: &'a roomkit_ai::AiContext,
    ) -> impl std::future::Future<Output = Result<roomkit_ai::AiResponse, roomkit_ai::ProviderError>>
    + Send
    + 'a {
        self.0.generate(context)
    }
    fn generate_structured_stream<'a>(
        &'a self,
        context: &'a roomkit_ai::AiContext,
    ) -> roomkit_ai::ProviderStream<'a> {
        self.0.generate_structured_stream(context)
    }
}
