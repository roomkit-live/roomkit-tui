//! Human decisions, interruption and decision history.

use super::*;

mod host;
#[tokio::test]
async fn interruption_cancels_a_pending_permission_and_the_next_turn_still_runs() {
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
        ScriptedRound::answering("Continued after interruption"),
    ]);
    let calls = Arc::new(AtomicUsize::new(0));
    let channel = prepare(&kit, provider, calls.clone(), false).await;
    let gate = channel.clone();
    kit.before_tool_use(roomkit::ToolHookSpec::new("permission"), move |event, _| {
        let gate = gate.clone();
        async move {
            let decision = gate
                .ask_permission(
                    &event.room_id,
                    &event.channel_id,
                    PermissionRequest::new(
                        "Read record?",
                        "Read a fictional record.",
                        Duration::from_secs(60),
                    ),
                )
                .await;
            Ok(if decision == Decision::Allow {
                roomkit::ToolUse::Allow
            } else {
                roomkit::ToolUse::block("Denied")
            })
        }
    })
    .unwrap();
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel.clone(), config()).run(host));
    next(&client).await;
    client.try_send(request("interrupted-turn")).unwrap();
    let permission = loop {
        if let Event::PermissionRequested { permission, .. } = next(&client).await {
            break permission;
        }
    };
    let interrupt = |id, run| {
        Request::new(
            RequestId::new(id),
            format!("interrupt-{id}"),
            "room",
            Command::Interrupt { run },
        )
    };
    client.try_send(interrupt(2, "stale-run".into())).unwrap();
    loop {
        if let Event::Reply { request, error } = next(&client).await
            && request == RequestId::new(2)
        {
            assert!(error.is_some());
            break;
        }
    }
    client
        .try_send(interrupt(3, permission.run.clone()))
        .unwrap();
    let mut acknowledged = false;
    let mut settled = false;
    let mut run_ended = false;
    loop {
        match next(&client).await {
            Event::Reply { request, error } if request == RequestId::new(3) => {
                assert!(error.is_none());
                acknowledged = true;
            }
            Event::PermissionResolved { permission: id, .. } if id == permission.id => {
                settled = true
            }
            Event::RunFinished { run, outcome, .. } => {
                assert_eq!(run, permission.run);
                assert_eq!(outcome, RunOutcome::Interrupted);
                run_ended = true;
            }
            Event::OperationFinished {
                operation, outcome, ..
            } => {
                assert_eq!(operation.as_str(), "interrupted-turn");
                assert_eq!(outcome, RunOutcome::Interrupted);
                assert!(run_ended, "Operation completion must follow run cleanup");
                break;
            }
            _ => {}
        }
    }
    assert!(
        acknowledged && settled,
        "Completion preceded acknowledgment or permission cleanup"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    // A late approval cannot revive an interrupted call.
    client
        .try_send(Request::new(
            RequestId::new(4),
            "late-approval",
            "room",
            Command::Decide {
                permission: permission.id,
                decision: Decision::Allow,
            },
        ))
        .unwrap();
    loop {
        if let Event::Reply { request, error } = next(&client).await
            && request == RequestId::new(4)
        {
            assert!(error.is_some());
            break;
        }
    }
    client
        .try_send(request("continue-after-interrupt"))
        .unwrap();
    let events = collect_operation(&client).await;
    assert!(events.iter().any(|event| matches!(
        event,
        Event::RunFinished {
            outcome: RunOutcome::Completed,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(event, Event::MessageFinalized { message, .. } if message.text == "Continued after interruption")));
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

#[tokio::test]
async fn explicit_decisions_are_confirmed_once_restored_and_hidden_from_the_model() {
    verify_decision_history(true).await;
}

#[tokio::test]
async fn legacy_decisions_return_to_their_own_turn_without_a_tool_identity() {
    verify_decision_history(false).await;
}

async fn verify_decision_history(linked: bool) {
    let file = std::env::temp_dir().join(format!(
        "roomkit-approvals-{}.sqlite3",
        uuid::Uuid::new_v4()
    ));
    let kit = RoomKit::new(RoomKitOptions::with_store(
        roomkit_sqlite::SqliteStore::open(&file).await.unwrap(),
    ));
    let provider = ScriptedProvider::new([
        ScriptedRound::calling(
            "Checking",
            [(
                "call1",
                "lookup_record",
                r#"{"_roomkit_intent":"Checking the record"}"#,
            )],
        ),
        ScriptedRound::calling("Checking more", [("automatic", "lookup_record", "{}")]),
        ScriptedRound::answering("First answer"),
        ScriptedRound::calling("Checking again", [("call1", "lookup_record", "{}")]),
        ScriptedRound::answering("Second answer"),
    ]);
    let calls = Arc::new(AtomicUsize::new(0));
    let channel = prepare(&kit, provider.clone(), calls.clone(), false).await;
    let gate = channel.clone();
    kit.before_tool_use(roomkit::ToolHookSpec::new("permission"), move |event, _| {
        let gate = gate.clone();
        async move {
            if event.call.id.as_str() == "automatic" {
                return Ok(roomkit::ToolUse::Allow);
            }
            let request = PermissionRequest::new(
                "Read record?",
                "Private audit detail",
                Duration::from_secs(30),
            )
            .with_summary("Private audit summary");
            let decision = if linked {
                gate.ask_tool_permission(&event, request).await
            } else {
                gate.ask_permission(&event.room_id, &event.channel_id, request)
                    .await
            };
            Ok(if decision == Decision::Allow {
                roomkit::ToolUse::Allow
            } else {
                roomkit::ToolUse::block("Denied")
            })
        }
    })
    .unwrap();
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel.clone(), config()).run(host));
    next(&client).await;
    for (operation, decision) in [("allow", Decision::Allow), ("deny", Decision::Deny)] {
        client.try_send(request(operation)).unwrap();
        let permission = loop {
            match next(&client).await {
                Event::PermissionRequested { permission, .. } => break permission,
                Event::Item {
                    item: Item::Approval(_),
                    ..
                } => panic!("No decision has been given"),
                _ => {}
            }
        };
        client
            .try_send(Request::new(
                RequestId::new(20),
                "answer",
                "room",
                Command::Decide {
                    permission: permission.id.clone(),
                    decision,
                },
            ))
            .unwrap();
        let events = collect_operation(&client).await;
        let receipts: Vec<_> = events
            .iter()
            .filter_map(|e| {
                if let Event::Item {
                    item: Item::Approval(r),
                    ..
                } = e
                {
                    Some(r)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].permission, permission);
        assert_eq!(receipts[0].decision, decision);
        assert_eq!(receipts[0].decided_by.as_str(), "person:alice");
        assert!(!events.iter().any(
            |e| matches!(e, Event::Notice { text, .. } if text.contains("could not be saved"))
        ));
        client
            .try_send(Request::new(
                RequestId::new(21),
                "duplicate-answer",
                "room",
                Command::Decide {
                    permission: permission.id,
                    decision,
                },
            ))
            .unwrap();
        assert!(matches!(
            next(&client).await,
            Event::Reply { error: Some(_), .. }
        ));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(!format!("{:?}", provider.requests()).contains("Private audit"));
    let rows = kit
        .get_timeline(&config().room, GetTimelineOptions::default())
        .await
        .unwrap();
    let receipts: Vec<_> = rows
        .iter()
        .filter(|row| row.metadata.contains_key("roomkit_tui_approval"))
        .collect();
    assert_eq!(receipts.len(), 2);
    assert!(
        receipts
            .iter()
            .all(|row| row.metadata.contains_key("roomkit_tui_approval_tool_id") == linked)
    );
    assert!(receipts.iter().all(|r| !r.visibility.admits(
        &config().agent,
        roomkit::model::ChannelCategory::Intelligence
    )));
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
    drop(kit);
    let kit = RoomKit::new(RoomKitOptions::with_store(
        roomkit_sqlite::SqliteStore::open(&file).await.unwrap(),
    ));
    let channel = ConversationChannel::new(ChannelId::new("console").unwrap());
    kit.register_channel(channel.clone()).unwrap();
    kit.register_channel(AiAgent::new(config().agent, ScriptedProvider::default()))
        .unwrap();
    let snapshot = LocalSession::new(kit.clone(), channel, config())
        .snapshot()
        .await
        .unwrap();
    assert_eq!(
        snapshot
            .items
            .iter()
            .filter(|i| matches!(i, Item::Approval(_)))
            .count(),
        2
    );
    for (decision, answer) in [
        (Decision::Allow, "First answer"),
        (Decision::Deny, "Second answer"),
    ] {
        let approval = snapshot
            .items
            .iter()
            .position(|i| matches!(i, Item::Approval(r) if r.decision == decision))
            .unwrap();
        let response = snapshot
            .items
            .iter()
            .position(|i| matches!(i, Item::Message(m) if m.text == answer))
            .unwrap();
        assert!(
            approval < response,
            "A restored decision must precede the answer to its turn"
        );
        if linked {
            assert!(
                matches!(&snapshot.items[approval - 1], Item::Tool { call, .. } if call.id.as_str() == "call1")
            );
            if decision == Decision::Allow {
                let next_message = snapshot
                    .items
                    .iter()
                    .position(|i| matches!(i, Item::Message(m) if m.text == "Checking more"))
                    .unwrap();
                assert!(
                    approval < next_message,
                    "The decision belongs beside its call, before subsequent agent messages and automatic tools"
                );
            }
        }
        if decision == Decision::Deny {
            let first_answer = snapshot
                .items
                .iter()
                .position(|i| matches!(i, Item::Message(m) if m.text == "First answer"))
                .unwrap();
            assert!(
                approval > first_answer,
                "Reused tool IDs must not move decisions into earlier turns"
            );
        }
    }
    kit.close().await.unwrap();
    std::fs::remove_file(file).unwrap();
}
