//! A turn ending after tools must explain its stop in the actual terminal view.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "Invalid fixtures and missing terminal events fail these tests"
)]

use std::{num::NonZeroUsize, time::Duration};

use roomkit::{
    CreateRoomOptions, RoomKit,
    model::{ChannelId, RoomId},
};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{AiChannel, AiChannelOptions, AiTool, ToolEntry, ToolHandler, ToolResult};
use roomkit_tui::{Client, Session, connection, render};
use roomkit_tui_engine::{ConversationChannel, LocalSession, SessionConfig};
use roomkit_tui_protocol::*;
use serde_json::json;

async fn room(rounds: Vec<ScriptedRound>) -> (RoomKit, ConversationChannel, SessionConfig) {
    let kit = RoomKit::new(Default::default());
    let config = SessionConfig::new(
        RoomId::new("room").unwrap(),
        ChannelId::new("agent").unwrap(),
        "alice",
    );
    let tool = AiTool {
        name: "lookup_record".into(),
        description: "Read a record".into(),
        parameters: json!({"type":"object","properties":{}}),
        tags: vec![],
        changes_state: false,
        defer_loading: false,
    };
    let options = AiChannelOptions {
        tools: vec![ToolEntry::Definition(tool)],
        tool_handler: Some(ToolHandler::new(|_, _| async {
            Ok(ToolResult::Json(json!({"record": "42"})))
        })),
        max_tool_rounds: 1,
        max_empty_retries: 0,
        ..Default::default()
    };
    kit.register_channel(
        AiChannel::new(config.agent.clone(), ScriptedProvider::new(rounds), options).unwrap(),
    )
    .unwrap();
    let channel = ConversationChannel::new(ChannelId::new("console").unwrap());
    kit.register_channel(channel.clone()).unwrap();
    kit.create_room(CreateRoomOptions {
        room_id: Some(config.room.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    for id in [channel.id(), &config.agent] {
        kit.attach_channel(&config.room, id, Default::default())
            .await
            .unwrap();
    }
    (kit, channel, config)
}

async fn next(client: &Client) -> Event {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match client.try_recv() {
                Ok(event) => return event,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                Err(error) => panic!("adapter disconnected: {error}"),
            }
        }
    })
    .await
    .expect("an event before its deadline")
}

async fn submit(client: &Client, session: &mut Session, id: u64) -> Vec<Event> {
    client
        .try_send(Request::new(
            RequestId::new(id),
            format!("op-{id}"),
            "room",
            Command::Submit {
                agent: "agent".into(),
                text: "Continue".into(),
            },
        ))
        .unwrap();
    let mut events = vec![];
    loop {
        let event = next(client).await;
        let ended = matches!(event, Event::OperationFinished { .. });
        session.apply(event.clone());
        events.push(event);
        if ended {
            return events;
        }
    }
}

fn calling(text: &str) -> ScriptedRound {
    ScriptedRound::calling(text, [("call1", "lookup_record", "{}")])
}

#[tokio::test]
async fn a_tool_only_stop_shows_its_reason_and_allows_continuation() {
    for (text, final_round, expected) in [
        ("", calling(""), "Tool round limit reached"),
        (
            "Checking the record",
            calling(""),
            "Tool round limit reached",
        ),
        (
            "",
            ScriptedRound::answering(""),
            "no final answer after using tools",
        ),
        (
            "",
            ScriptedRound::answering("").with_finish_reason("max_tokens"),
            "output or context limit",
        ),
        (
            "",
            calling("Still checking the record"),
            "Tool round limit reached",
        ),
    ] {
        check_stop(text, final_round, expected).await;
    }
}

async fn check_stop(text: &str, final_round: ScriptedRound, expected: &str) {
    let (kit, channel, config) = room(vec![
        calling(text),
        final_round,
        ScriptedRound::answering("Work can continue."),
    ])
    .await;
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config).run(host));
    let mut session = Session::default();
    session.apply(next(&client).await);
    let events = submit(&client, &mut session, 1).await;
    assert_stopped(&session, &events, expected);
    let later = submit(&client, &mut session, 2).await;
    assert_continued(&later);
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

fn assert_stopped(session: &Session, events: &[Event], expected: &str) {
    assert!(session.notice().contains(expected), "{events:#?}");
    let notices: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::Notice { .. }))
        .collect();
    assert_eq!(
        notices.len(),
        1,
        "A stop is explained exactly once: {events:#?}"
    );
    assert!(events.iter().any(|e| matches!(
        e,
        Event::RunFinished {
            outcome: RunOutcome::Failed,
            ..
        }
    )));
    assert!(events.iter().any(|e| matches!(e,
        Event::Item { item: Item::Tool { call, .. }, .. } if call.state == ToolState::Succeeded)));
    let frame = render::screen(session, 160, 40, &Default::default());
    assert!(frame.lines.join("\n").contains(expected));
    assert!(frame.lines.join("\n").contains("Execution Failed"));
}

fn assert_continued(later: &[Event]) {
    assert!(later.iter().any(|e| matches!(e,
        Event::MessageFinalized { message, .. } if message.text == "Work can continue.")));
    assert!(later.iter().any(|e| matches!(
        e,
        Event::RunFinished {
            outcome: RunOutcome::Completed,
            ..
        }
    )));
    assert!(!later.iter().any(|e| matches!(e, Event::Notice { .. })));
}
