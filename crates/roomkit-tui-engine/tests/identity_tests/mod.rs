//! Identities the engine sends stay within the bound terminals apply to
//! identities: those it derives, however long the agent's id, and those the
//! application supplies.

use super::*;
use roomkit_tui_protocol::limits::MAX_LABEL_BYTES;

/// The longest host run identity: its approvals add `:approval:` and a `u64`
/// within the terminal's identity bound.
const MAX_RUN_ID_BYTES: usize = MAX_LABEL_BYTES - ":approval:".len() - 20;

/// 250 bytes. RoomKit bounds no identity, and `{agent}:{trigger}` alone
/// exceeded the terminal's bound: it ignored the run and hid its permissions.
fn long_agent() -> ChannelId {
    ChannelId::new(format!("agent-{}", "x".repeat(244))).unwrap()
}

/// The run that finished before the operation did, if one did.
async fn finished(client: &Client) -> Option<(RunId, RunOutcome)> {
    let mut finished = None;
    loop {
        match next(client).await {
            Event::RunFinished { run, outcome, .. } => finished = Some((run, outcome)),
            Event::OperationFinished { .. } => return finished,
            _ => {}
        }
    }
}

/// Ask the person before every tool call.
fn ask_before_tools<S: ConversationStore>(kit: &RoomKit<S>, gate: ConversationChannel) {
    kit.hook(
        roomkit::HookTrigger::BeforeToolUse,
        move |event: roomkit::ToolCallEvent, _| {
            let gate = gate.clone();
            async move {
                let request = PermissionRequest::new(
                    "Read record?",
                    "Read a fictional record.",
                    Duration::from_secs(60),
                );
                let decision = gate
                    .ask_permission(event.room_id.as_ref().unwrap(), &event.channel_id, request)
                    .await;
                Ok(if decision == Decision::Allow {
                    roomkit::model::HookResult::allow(Default::default())
                } else {
                    roomkit::model::HookResult::block("Denied", Default::default())
                })
            }
        },
        hook_options("permission"),
    )
    .unwrap();
}

#[tokio::test]
async fn a_long_agent_id_still_gives_runs_and_permissions_a_terminal_accepts() {
    let kit = RoomKit::new(Default::default());
    let provider = ScriptedProvider::new([
        ScriptedRound::calling(
            "Checking",
            [(
                "call1",
                "lookup_record",
                r#"{"_roomkit_intent":"Checking the record"}"#,
            )],
        ),
        ScriptedRound::answering("Done"),
    ]);
    let calls = Arc::new(AtomicUsize::new(0));
    let agent = long_agent();
    let channel = prepare_as(&kit, &agent, provider, calls.clone(), false).await;
    ask_before_tools(&kit, channel.clone());
    let mut view = config();
    view.agent = agent.clone();
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let session = LocalSession::new(kit.clone(), channel.clone(), view.clone());
    let task = tokio::spawn(session.run(host));
    assert!(matches!(next(&client).await, Event::Snapshot(_)));
    let submit = Command::Submit {
        agent: agent.as_str().into(),
        text: "Look up record 42".into(),
    };
    client
        .try_send(Request::new(RequestId::new(1), "long", "room", submit))
        .unwrap();

    let run = loop {
        if let Event::RunStarted { run, .. } = next(&client).await {
            break run;
        }
    };
    let bytes = run.as_str().len();
    assert!(
        bytes <= MAX_LABEL_BYTES,
        "A run id of {bytes} bytes is ignored"
    );
    let permission = loop {
        if let Event::PermissionRequested { permission, .. } = next(&client).await {
            break permission;
        }
    };
    assert!(permission.fits(), "A terminal hides {:?}", permission.id);
    assert_eq!(permission.run, run);
    let decide = Command::Decide {
        permission: permission.id,
        decision: Decision::Allow,
    };
    client
        .try_send(Request::new(RequestId::new(2), "decide", "room", decide))
        .unwrap();
    assert_eq!(
        finished(&client).await,
        Some((run.clone(), RunOutcome::Completed))
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1, "The approved tool ran");

    let restored = LocalSession::new(kit.clone(), channel, view)
        .snapshot()
        .await
        .unwrap();
    assert!(
        restored
            .items
            .iter()
            .any(|item| matches!(item, Item::Tool { run: tool, .. } if *tool == run)),
        "Restored history keeps the run's identity"
    );
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

/// Starts host work under the run identity given as the command's argument.
struct Executor;

impl roomkit_tui_engine::CommandHandler for Executor {
    async fn handle(
        &self,
        request: Request,
        _: SessionConfig,
    ) -> Result<roomkit_tui_engine::CommandResult, Rejection> {
        let Command::Invoke { arguments, .. } = request.command else {
            return Err("Unsupported".into());
        };
        let work = roomkit_tui_engine::CommandExecution::new(arguments.into(), |_| async {
            Ok(RunOutcome::Completed)
        });
        Ok(roomkit_tui_engine::CommandResult::Execute(work))
    }
}

/// Invoke `work`, whose execution runs under the identity `run`.
fn work(id: u64, run: String) -> Request {
    let command = Command::Invoke {
        name: "work".into(),
        arguments: run,
    };
    Request::new(RequestId::new(id), format!("work-{id}"), "room", command)
}

/// A view that offers `work`, its client and its serving task.
async fn executor_view<S: ConversationStore>(
    kit: &RoomKit<S>,
) -> (Client, tokio::task::JoinHandle<Result<(), AdapterError>>) {
    let channel = prepare(kit, ScriptedProvider::new([]), Arc::default(), false).await;
    let mut view = config();
    view.commands.push(CommandSpec::new("work", "Host work"));
    let (client, host) = connection(NonZeroUsize::new(64).unwrap());
    let session = LocalSession::new(kit.clone(), channel, view);
    let task = tokio::spawn(session.run_with_commands(host, Executor));
    assert!(matches!(next(&client).await, Event::Snapshot(_)));
    (client, task)
}

/// RMK-1104: a host run identity over the bound started work the terminal
/// could neither show nor interrupt.
#[tokio::test]
async fn host_work_under_a_run_id_a_terminal_ignores_is_rejected() {
    let kit = RoomKit::new(Default::default());
    let (client, task) = executor_view(&kit).await;
    client
        .try_send(work(1, "r".repeat(MAX_RUN_ID_BYTES + 1)))
        .unwrap();
    let Event::Reply {
        error: Some(rejection),
        ..
    } = next(&client).await
    else {
        panic!("Work under an oversized run id was started");
    };
    assert_eq!(rejection.code, ErrorCode::Invalid);
    client
        .try_send(work(2, "r".repeat(MAX_RUN_ID_BYTES)))
        .unwrap();
    assert!(matches!(
        next(&client).await,
        Event::Reply { error: None, .. }
    ));
    assert!(matches!(
        next(&client).await,
        Event::OperationStarted { .. }
    ));
    assert!(matches!(next(&client).await, Event::RunStarted { .. }));
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

/// RMK-1110: a terminal ignores a `RunStarted` that repeats its last run, so
/// a second `/build` under the same identity ran unseen and could not be
/// interrupted.
#[tokio::test]
async fn host_work_repeating_the_last_run_id_is_rejected() {
    let kit = RoomKit::new(Default::default());
    let (client, task) = executor_view(&kit).await;
    client.try_send(work(1, "build".into())).unwrap();
    assert_operation(
        &collect_operation(&client).await,
        "work-1",
        RunOutcome::Completed,
    );
    client.try_send(work(2, "build".into())).unwrap();
    let Event::Reply {
        error: Some(rejection),
        ..
    } = next(&client).await
    else {
        panic!("Work repeating the last run was started");
    };
    assert_eq!(rejection.code, ErrorCode::Invalid);
    client.try_send(work(3, "build-2".into())).unwrap();
    assert_operation(
        &collect_operation(&client).await,
        "work-3",
        RunOutcome::Completed,
    );
    client.try_send(work(4, "build".into())).unwrap();
    let accepted = next(&client).await;
    assert!(
        matches!(accepted, Event::Reply { error: None, .. }),
        "Another run came between: {accepted:?}"
    );
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}
