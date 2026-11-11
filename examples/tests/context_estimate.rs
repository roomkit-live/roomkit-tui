//! RMK-1646: a host that compacts a conversation between turns tells the
//! view its estimate of the context, which the terminal's gauge shows until
//! the next turn measures it, live and after a relaunch.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "Invalid fixtures and missing terminal events fail these tests"
)]

use std::{num::NonZeroUsize, time::Duration};

use roomkit::{
    CreateRoomOptions, RoomKit,
    model::{ChannelId, Metadata, RoomId},
};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{AiChannel, AiChannelOptions};
use roomkit_tui::{Client, Session, connection, render};
use roomkit_tui_engine::{
    CommandHandler, CommandResult, ConversationChannel, LocalSession, SessionConfig,
};
use roomkit_tui_protocol::*;
use serde_json::json;
use tokio::sync::watch;

const CAPACITY: u64 = 1_000_000;

/// A host whose `compact` command states its estimate of the context in the
/// live state its subscription sends, as a host keeps a compaction's
/// estimate beside its record.
struct Compacting {
    live: watch::Sender<Vec<Event>>,
}

impl CommandHandler for Compacting {
    fn subscribe(&self, _: SessionConfig) -> Option<watch::Receiver<Vec<Event>>> {
        Some(self.live.subscribe())
    }

    async fn handle(&self, request: Request, _: SessionConfig) -> Result<CommandResult, Rejection> {
        match request.command {
            Command::Invoke { name, .. } if name == "compact" => {
                self.live.send_replace(vec![estimate("scripted", 92_000)]);
                Ok(CommandResult::Events(vec![]))
            }
            _ => Err("Unsupported".into()),
        }
    }
}

fn estimate(model: &str, tokens: u64) -> Event {
    Event::ContextEstimated {
        conversation: "room".into(),
        provider: "scripted".into(),
        model: model.into(),
        usage: ContextUsage::new(tokens),
    }
}

fn used(input: u64, output: u64) -> Metadata {
    json!({ "input_tokens": input, "output_tokens": output })
        .as_object()
        .unwrap()
        .clone()
}

async fn room() -> (RoomKit, ConversationChannel, SessionConfig) {
    let kit = RoomKit::new(Default::default());
    let mut config = SessionConfig::new(
        RoomId::new("room").unwrap(),
        ChannelId::new("agent").unwrap(),
        "alice",
    );
    config.model = Some(ModelInfo::new("scripted", "scripted"));
    config.context_capacity = Some(CAPACITY);
    config.commands = vec![CommandSpec::new("compact", "Compact the conversation")];
    let provider = ScriptedProvider::new([
        ScriptedRound::answering("First.").with_usage(used(650_250, 1_321)),
        ScriptedRound::answering("Second.").with_usage(used(145_000, 2_015)),
    ]);
    let agent = AiChannel::new(config.agent.clone(), provider, AiChannelOptions::default());
    kit.register_channel(agent.unwrap()).unwrap();
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

/// Apply the view's events to `session` until one satisfies `done`.
async fn until(client: &Client, session: &mut Session, done: impl Fn(&Event) -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match client.try_recv() {
                Ok(event) => {
                    let last = done(&event);
                    session.apply(event);
                    if last {
                        return;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                Err(error) => panic!("adapter disconnected: {error}"),
            }
        }
    })
    .await
    .expect("an event before its deadline");
}

fn send(client: &Client, id: u64, command: Command) {
    let request = Request::new(RequestId::new(id), format!("op-{id}"), "room", command);
    client.try_send(request).unwrap();
}

async fn ask(client: &Client, session: &mut Session, id: u64) {
    let text = "Continue".into();
    send(
        client,
        id,
        Command::Submit {
            agent: "agent".into(),
            text,
        },
    );
    until(client, session, |e| {
        matches!(e, Event::OperationFinished { .. })
    })
    .await;
}

async fn compact(client: &Client, session: &mut Session, id: u64) {
    let (name, arguments) = ("compact".into(), String::new());
    send(client, id, Command::Invoke { name, arguments });
    until(client, session, |e| {
        matches!(e, Event::ContextEstimated { .. })
    })
    .await;
}

fn footer(session: &Session) -> String {
    render::screen(session, 230, 30, &Default::default())
        .lines
        .join("\n")
}

#[tokio::test]
async fn a_compaction_between_turns_shows_its_estimate_until_the_next_turn_measures() {
    let (kit, channel, config) = room().await;
    let (live, _) = watch::channel(vec![estimate("other-model", 10)]);
    let host = Compacting { live: live.clone() };
    let (client, view) = connection(NonZeroUsize::new(256).unwrap());
    let session = LocalSession::new(kit.clone(), channel.clone(), config.clone());
    let task = tokio::spawn(session.run_with_commands(view, host));
    let mut session = Session::default();
    until(&client, &mut session, |e| {
        matches!(e, Event::ContextEstimated { .. })
    })
    .await;
    assert!(
        footer(&session).contains("context ?"),
        "Another model's estimate is ignored"
    );

    ask(&client, &mut session, 1).await;
    assert!(footer(&session).contains("context 651571/1000000"));
    compact(&client, &mut session, 2).await;
    let screen = footer(&session);
    assert!(screen.contains("context ~92000/1000000"), "{screen}");
    drop(client);
    task.await.unwrap().unwrap();

    // Relaunched, the view reads the measure stored before the compaction,
    // then the estimate the host kept with it.
    let (client, view) = connection(NonZeroUsize::new(256).unwrap());
    let host = Compacting { live: live.clone() };
    let session = LocalSession::new(kit.clone(), channel, config);
    let task = tokio::spawn(session.run_with_commands(view, host));
    let mut session = Session::default();
    until(&client, &mut session, |e| matches!(e, Event::Snapshot(_))).await;
    assert!(footer(&session).contains("context 651571/1000000"));
    until(&client, &mut session, |e| {
        matches!(e, Event::ContextEstimated { .. })
    })
    .await;
    assert!(footer(&session).contains("context ~92000/1000000"));

    // The next turn measures, and the estimate the host sends again with
    // another change of its state replaces nothing.
    ask(&client, &mut session, 3).await;
    assert!(footer(&session).contains("context 147015/1000000"));
    let notice = Event::Notice {
        conversation: "room".into(),
        text: "Compacted earlier".into(),
    };
    live.send_replace(vec![estimate("scripted", 92_000), notice]);
    until(&client, &mut session, |e| matches!(e, Event::Notice { .. })).await;
    let screen = footer(&session);
    assert!(screen.contains("context 147015/1000000"), "{screen}");
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}
