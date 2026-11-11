//! RMK-1646: a host that compacts a conversation between turns tells the
//! view its estimate of the context, which the terminal's gauge shows until
//! the next turn measures it, live and after a relaunch.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "Invalid fixtures and missing terminal events fail these tests"
)]

use std::{num::NonZeroUsize, sync::Arc, time::Duration};

use roomkit::{
    CreateRoomOptions, RoomKit,
    model::{ChannelId, Metadata, RoomId},
};
use roomkit_ai::testing::{ScriptedProvider, ScriptedRound};
use roomkit_ai::{AiChannel, AiChannelOptions};
use roomkit_tui::{Client, Session, connection, render};
use roomkit_tui_engine::{
    AdapterError, CommandExecution, CommandHandler, CommandResult, ConversationChannel,
    LifecycleFuture, LocalSession, SessionConfig, SessionLifecycle, TurnOutcome,
};
use roomkit_tui_protocol::*;
use serde_json::json;
use tokio::{sync::watch, task::JoinHandle};

const CAPACITY: u64 = 1_000_000;

/// A host that keeps a compaction's estimate in the live state its
/// subscription sends, until a turn of the agent measured the context. Its
/// `compact` compacts beside the view, as work it reports later; its
/// `compact-now` compacts as host work the view follows as a run.
#[derive(Clone, Debug)]
struct Host {
    live: Arc<watch::Sender<Vec<Event>>>,
}

impl Host {
    fn new(events: Vec<Event>) -> Self {
        Self {
            live: Arc::new(watch::channel(events).0),
        }
    }

    fn compacted(&self) {
        self.live.send_replace(vec![estimate("scripted", 92_000)]);
    }
}

impl CommandHandler for Host {
    fn subscribe(&self, _: SessionConfig) -> Option<watch::Receiver<Vec<Event>>> {
        Some(self.live.subscribe())
    }

    async fn handle(&self, request: Request, _: SessionConfig) -> Result<CommandResult, Rejection> {
        let Command::Invoke { name, .. } = request.command else {
            return Err("Unsupported".into());
        };
        let host = self.clone();
        match name.as_str() {
            "compact" => {
                host.compacted();
                Ok(CommandResult::Events(vec![]))
            }
            "compact-now" => Ok(CommandResult::Execute(CommandExecution::new(
                "compaction-1".into(),
                move |_| async move {
                    host.compacted();
                    // The view reads the estimate while the run goes.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok(RunOutcome::Completed)
                },
            ))),
            _ => Err("Unsupported".into()),
        }
    }
}

impl SessionLifecycle for Host {
    /// The turn measured the context: the estimate is no longer current.
    fn finished<'a>(
        &'a self,
        _: &'a SessionConfig,
        _: &'a OperationId,
        _: TurnOutcome,
    ) -> LifecycleFuture<'a, ()> {
        self.live.send_modify(|events| {
            events.retain(|event| !matches!(event, Event::ContextEstimated { .. }));
        });
        Box::pin(async {})
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
    config.commands = vec![
        CommandSpec::new("compact", "Compact the conversation"),
        CommandSpec::new("compact-now", "Compact the conversation as host work"),
    ];
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

/// A terminal attached to the room through `host`, its snapshot applied.
struct View {
    client: Client,
    ui: Session,
    task: JoinHandle<Result<(), AdapterError>>,
}

impl View {
    async fn attach(
        kit: &RoomKit,
        channel: &ConversationChannel,
        config: &SessionConfig,
        host: &Host,
    ) -> Self {
        let (client, adapter) = connection(NonZeroUsize::new(256).unwrap());
        let session = LocalSession::new(kit.clone(), channel.clone(), config.clone())
            .with_lifecycle(Arc::new(host.clone()));
        let task = tokio::spawn(session.run_with_commands(adapter, host.clone()));
        let mut view = Self {
            client,
            ui: Session::default(),
            task,
        };
        view.until(|e| matches!(e, Event::Snapshot(_))).await;
        view
    }

    async fn close(self) {
        drop(self.client);
        self.task.await.unwrap().unwrap();
    }

    /// Apply the view's events until one satisfies `done`.
    async fn until(&mut self, done: impl Fn(&Event) -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match self.client.try_recv() {
                    Ok(event) => {
                        let last = done(&event);
                        self.ui.apply(event);
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

    fn send(&self, id: u64, command: Command) {
        let request = Request::new(RequestId::new(id), format!("op-{id}"), "room", command);
        self.client.try_send(request).unwrap();
    }

    /// Ask the agent, and wait for its turn to end.
    async fn ask(&mut self, id: u64) {
        let text = "Continue".into();
        self.send(
            id,
            Command::Submit {
                agent: "agent".into(),
                text,
            },
        );
        self.until(|e| matches!(e, Event::OperationFinished { .. }))
            .await;
    }

    /// Invoke the host's `name`, and wait for the estimate it tells.
    async fn invoke(&mut self, id: u64, name: &str) {
        let (name, arguments) = (name.into(), String::new());
        self.send(id, Command::Invoke { name, arguments });
        self.until(|e| matches!(e, Event::ContextEstimated { .. }))
            .await;
    }

    fn footer(&self) -> String {
        render::screen(&self.ui, 230, 30, &Default::default())
            .lines
            .join("\n")
    }
}

#[tokio::test]
async fn a_compaction_between_turns_shows_its_estimate_live_and_after_a_relaunch() {
    let (kit, channel, config) = room().await;
    let host = Host::new(vec![estimate("other-model", 10)]);
    let mut view = View::attach(&kit, &channel, &config, &host).await;
    view.until(|e| matches!(e, Event::ContextEstimated { .. }))
        .await;
    assert!(
        view.footer().contains("context ?"),
        "Another model's estimate is ignored"
    );
    view.ask(1).await;
    assert!(view.footer().contains("context 651571/1000000"));
    view.invoke(2, "compact").await;
    let footer = view.footer();
    assert!(footer.contains("context ~92000/1000000"), "{footer}");
    view.close().await;

    // Relaunched, the view reads the measure stored before the compaction,
    // then the estimate the host kept with it.
    let mut view = View::attach(&kit, &channel, &config, &host).await;
    assert!(view.footer().contains("context 651571/1000000"));
    view.until(|e| matches!(e, Event::ContextEstimated { .. }))
        .await;
    assert!(view.footer().contains("context ~92000/1000000"));
    view.close().await;
    kit.close().await.unwrap();
}

#[tokio::test]
async fn the_next_turn_s_measure_replaces_the_estimate_for_good() {
    let (kit, channel, config) = room().await;
    let host = Host::new(vec![]);
    let mut view = View::attach(&kit, &channel, &config, &host).await;
    view.ask(1).await;
    view.invoke(2, "compact").await;
    view.ask(3).await;
    assert!(view.footer().contains("context 147015/1000000"));
    assert!(
        host.live.borrow().is_empty(),
        "The host withdrew its estimate"
    );

    // A state the host computed before the turn ended, read after it.
    let notice = Event::Notice {
        conversation: "room".into(),
        text: "Compacted earlier".into(),
    };
    host.live
        .send_replace(vec![estimate("scripted", 92_000), notice]);
    view.until(|e| matches!(e, Event::Notice { .. })).await;
    assert!(view.footer().contains("context 147015/1000000"));
    view.close().await;

    // Relaunched, the view reads the turn's measure, and no estimate.
    host.live.send_replace(vec![]);
    let view = View::attach(&kit, &channel, &config, &host).await;
    assert!(view.footer().contains("context 147015/1000000"));
    view.close().await;
    kit.close().await.unwrap();
}

#[tokio::test]
async fn a_compaction_run_as_host_work_shows_its_estimate_once_it_ends() {
    let (kit, channel, config) = room().await;
    let host = Host::new(vec![]);
    let mut view = View::attach(&kit, &channel, &config, &host).await;
    view.ask(1).await;
    view.invoke(2, "compact-now").await;
    assert!(
        view.ui.active_run().is_some(),
        "The estimate came during the run"
    );
    view.until(|e| matches!(e, Event::OperationFinished { .. }))
        .await;
    let footer = view.footer();
    assert!(footer.contains("context ~92000/1000000"), "{footer}");
    view.close().await;
    kit.close().await.unwrap();
}
