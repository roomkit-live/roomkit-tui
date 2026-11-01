//! Identities the engine derives from the agent and its triggers stay within
//! the bound terminals apply to identities, however long the agent's id.

use super::*;
use roomkit_tui_protocol::limits::MAX_LABEL_BYTES;

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
    kit.before_tool_use(roomkit::ToolHookSpec::new("permission"), move |event, _| {
        let gate = gate.clone();
        async move {
            let request = PermissionRequest::new(
                "Read record?",
                "Read a fictional record.",
                Duration::from_secs(60),
            );
            let decision = gate
                .ask_permission(&event.room_id, &event.channel_id, request)
                .await;
            Ok(if decision == Decision::Allow {
                roomkit::ToolUse::Allow
            } else {
                roomkit::ToolUse::block("Denied")
            })
        }
    })
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
