//! Message delivery, restoration and engine isolation.

use super::*;
#[tokio::test]
async fn an_accepted_submission_settles_when_the_engine_fails_before_streaming() {
    let kit = RoomKit::new(Default::default());
    let provider = ScriptedProvider::new([ScriptedRound::answering("Unused")]);
    let channel = prepare(&kit, provider.clone(), Arc::default(), false).await;
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    assert!(matches!(next(&client).await, Event::Snapshot(_)));
    kit.close().await.unwrap();

    client.try_send(request("unavailable-engine")).unwrap();
    let events = collect_operation(&client).await;
    assert_operation(&events, "unavailable-engine", RunOutcome::Failed);
    assert!(matches!(
        events.first(),
        Some(Event::Reply { error: None, .. })
    ));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::RunStarted { .. }))
    );
    assert!(
        matches!(events.last(), Some(Event::OperationFinished { operation, outcome: RunOutcome::Failed, .. }) if operation.as_str() == "unavailable-engine")
    );
    assert!(provider.requests().is_empty());
    drop(client);
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn admission_remains_busy_and_a_policy_block_settles_without_a_run() {
    let kit = RoomKit::new(Default::default());
    let provider = ScriptedProvider::new([]);
    let channel = prepare(&kit, provider.clone(), Arc::default(), false).await;
    let release = Arc::new(tokio::sync::Notify::new());
    let gate = release.clone();
    kit.hook(
        roomkit::HookTrigger::BeforeBroadcast,
        move |_: roomkit::model::RoomEvent, _| {
            let gate = gate.clone();
            async move {
                gate.notified().await;
                Ok(roomkit::model::HookResult::block(
                    "Closed for maintenance",
                    Default::default(),
                ))
            }
        },
        hook_options("admission"),
    )
    .unwrap();
    let check = LocalSession::new(kit.clone(), channel.clone(), config());
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    next(&client).await;
    client.try_send(request("blocked-operation")).unwrap();
    let mut events = vec![next(&client).await, next(&client).await];
    assert!(matches!(events[0], Event::Reply { error: None, .. }));
    assert!(matches!(events[1], Event::OperationStarted { .. }));
    assert!(matches!(check.snapshot().await, Err(AdapterError::Busy)));
    client.try_send(request("concurrent-operation")).unwrap();
    assert!(matches!(
        next(&client).await,
        Event::Reply { error: Some(_), .. }
    ));
    release.notify_one();
    events.extend(collect_operation(&client).await);
    assert_operation(&events, "blocked-operation", RunOutcome::Failed);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::RunStarted { .. }))
    );
    assert!(provider.requests().is_empty());
    assert!(check.snapshot().await.is_ok());
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

#[tokio::test]
async fn engine_streams_runs_real_tools_and_restores_their_durable_identities() {
    let kit = RoomKit::new(Default::default());
    let provider = ScriptedProvider::new([
        ScriptedRound::calling(
            "Checking. ",
            [(
                "call1",
                "lookup_record",
                r#"{"_roomkit_intent":"Checking the record"}"#,
            )],
        ),
        ScriptedRound::writing(["Record ", "42 is ready."]),
    ]);
    let calls = Arc::new(AtomicUsize::new(0));
    let channel = prepare(&kit, provider.clone(), calls.clone(), false).await;
    let adapter = LocalSession::new(kit.clone(), channel.clone(), config());
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let task = tokio::spawn(adapter.run(host));
    assert!(
        matches!(next(&client).await, Event::Snapshot(s) if s.can_interrupt && s.commands.is_empty() && s.local_actor == Some("person:alice".into()))
    );
    client.try_send(request("one")).unwrap();
    let events = collect_operation(&client).await;
    assert_operation(&events, "one", RunOutcome::Completed);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::TextDelta { text, .. } if text.contains("Record")))
    );
    let final_id = events
        .iter()
        .find_map(|e| match e {
            Event::MessageFinalized { message, .. } if message.text == "Record 42 is ready." => {
                Some(message.id.clone())
            }
            _ => None,
        })
        .unwrap();
    assert!(events.iter().any(|e| matches!(e, Event::Item { item: Item::Tool { call, .. }, .. } if call.state == ToolState::Succeeded && call.text.contains("ready") && call.data.is_none())));
    let restored = LocalSession::new(kit.clone(), channel, config())
        .snapshot()
        .await
        .unwrap();
    assert_eq!(restored.local_actor, Some("person:alice".into()));
    assert!(
        restored.items.iter().any(
            |i| matches!(i, Item::Message(m) if Some(&m.actor) == restored.local_actor.as_ref())
        )
    );
    assert!(
        restored
            .items
            .iter()
            .any(|i| matches!(i, Item::Message(m) if m.id == final_id))
    );
    assert_eq!(
        restored
            .items
            .iter()
            .filter(|i| matches!(i, Item::Tool { .. }))
            .count(),
        1
    );
    for event in &events {
        if let Event::Item {
            item: Item::Tool { call, .. },
            ..
        } = event
        {
            assert_eq!(call.intent.as_deref(), Some("Checking the record"));
        }
    }
    assert!(restored.items.iter().any(|item| matches!(item, Item::Tool { call, .. } if call.intent.as_deref() == Some("Checking the record"))));
    assert_eq!(provider.requests().len(), 2);
    drop(client);
    assert!(task.await.unwrap().is_ok());
    kit.close().await.unwrap();
}

#[tokio::test]
async fn detaching_a_view_does_not_stop_a_tool_or_close_the_shared_engine() {
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
        ScriptedRound::answering("Still completed"),
    ]);
    let calls = Arc::new(AtomicUsize::new(0));
    let channel = prepare(&kit, provider, calls.clone(), false).await;
    let gate = Arc::new(tokio::sync::Notify::new());
    let reached = Arc::new(tokio::sync::Notify::new());
    let release = gate.clone();
    let entered = reached.clone();
    kit.hook(
        roomkit::HookTrigger::BeforeToolUse,
        move |_: roomkit::ToolCallEvent, _| {
            let release = release.clone();
            let entered = entered.clone();
            async move {
                entered.notify_one();
                release.notified().await;
                Ok(roomkit::model::HookResult::allow(Default::default()))
            }
        },
        hook_options("hold"),
    )
    .unwrap();
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel.clone(), config()).run(host));
    next(&client).await;
    client.try_send(request("detach")).unwrap();
    tokio::time::timeout(Duration::from_secs(5), reached.notified())
        .await
        .unwrap();
    drop(client);
    assert!(task.await.unwrap().is_ok());
    gate.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = LocalSession::new(kit.clone(), channel.clone(), config())
                .snapshot()
                .await;
            if snapshot.is_ok_and(|s| {
                s.items
                    .iter()
                    .any(|i| matches!(i, Item::Message(m) if m.text == "Still completed"))
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    kit.close().await.unwrap();
}

#[tokio::test]
async fn sqlite_restart_and_retried_operation_do_not_execute_the_tool_twice() {
    let file = std::env::temp_dir().join(format!("roomkit-tui-{}.sqlite3", uuid::Uuid::new_v4()));
    let calls = Arc::new(AtomicUsize::new(0));
    {
        let kit = RoomKit::new(RoomKitOptions::with_store(
            roomkit_sqlite::SqliteStore::new(roomkit_sqlite::SqliteStoreOptions {
                path: std::path::PathBuf::from(&file),
            }),
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
            ScriptedRound::answering("Stored answer"),
        ]);
        let channel = prepare(&kit, provider, calls.clone(), false).await;
        let (client, host) = connection(NonZeroUsize::new(128).unwrap());
        let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
        next(&client).await;
        client.try_send(request("stable-operation")).unwrap();
        collect_operation(&client).await;
        drop(client);
        task.await.unwrap().unwrap();
        kit.close().await.unwrap();
    }
    {
        let kit = RoomKit::new(RoomKitOptions::with_store(
            roomkit_sqlite::SqliteStore::new(roomkit_sqlite::SqliteStoreOptions {
                path: std::path::PathBuf::from(&file),
            }),
        ));
        let channel = ConversationChannel::new(ChannelId::new("console").unwrap());
        kit.register_channel(channel.clone()).unwrap();
        let provider = ScriptedProvider::default();
        kit.register_channel(
            AiChannel::new(
                config().agent,
                provider.clone(),
                AiChannelOptions::default(),
            )
            .unwrap(),
        )
        .unwrap();
        let adapter = LocalSession::new(kit.clone(), channel, config());
        let snapshot = adapter.snapshot().await.unwrap();
        assert!(snapshot.items.iter().any(|item| matches!(item, Item::Tool { call, .. } if call.intent.as_deref() == Some("Checking the record"))));
        assert!(
            snapshot
                .items
                .iter()
                .any(|i| matches!(i, Item::Message(m) if m.text == "Stored answer"))
        );
        let (client, host) = connection(NonZeroUsize::new(128).unwrap());
        let task = tokio::spawn(adapter.run(host));
        next(&client).await;
        client.try_send(request("stable-operation")).unwrap();
        // The replay returns the already committed inbound message, no new turn.
        let replay = collect_operation(&client).await;
        assert!(replay.iter().any(|event| matches!(event, Event::Item { item: Item::Message(m), .. } if m.text == "Look up record 42")));
        assert!(
            !replay
                .iter()
                .any(|event| matches!(event, Event::RunStarted { .. }))
        );
        assert!(matches!(
            replay.last(),
            Some(Event::OperationFinished {
                outcome: RunOutcome::Completed,
                ..
            })
        ));
        drop(client);
        task.await.unwrap().unwrap();
        assert!(provider.requests().is_empty());
        assert_eq!(
            kit.get_timeline(&config().room, GetTimelineOptions::default())
                .await
                .unwrap()
                .iter()
                .filter(|e| roomkit_core::rules::transcoding::text_of(&e.content)
                    == "Look up record 42")
                .count(),
            1
        );
        // The kit refuses reads once closed.
        kit.close().await.unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn restoration_checks_read_access_and_engine_visibility() {
    let kit = RoomKit::new(Default::default());
    let channel = prepare(&kit, ScriptedProvider::default(), Arc::default(), true).await;
    kit.send_event(
        &config().room,
        channel.id(),
        roomkit::model::TextContent::new("visible"),
        Default::default(),
    )
    .await
    .unwrap();
    kit.send_event(
        &config().room,
        &config().agent,
        roomkit::model::TextContent::new("not for console"),
        roomkit::SendEventOptions {
            visibility: roomkit::model::Visibility::Intelligence,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let adapter = LocalSession::new(kit.clone(), channel.clone(), config());
    assert!(!format!("{:?}", adapter.snapshot().await.unwrap()).contains("not for console"));
    kit.set_access(
        &config().room,
        channel.id(),
        roomkit::model::Access::WriteOnly,
        Default::default(),
    )
    .await
    .unwrap();
    assert!(matches!(
        adapter.snapshot().await,
        Err(AdapterError::InvalidBinding)
    ));
    kit.close().await.unwrap();
}

#[tokio::test]
async fn provider_errors_do_not_expose_private_diagnostics_to_the_client() {
    let kit = RoomKit::new(Default::default());
    let channel = prepare(
        &kit,
        ScriptedProvider::new([
            ScriptedRound::answering("Partial text").failing("credential=PRIVATE")
        ]),
        Arc::default(),
        false,
    )
    .await;
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    next(&client).await;
    client.try_send(request("failure")).unwrap();
    let events = collect_operation(&client).await;
    assert!(events.iter().any(|event| matches!(
        event,
        Event::RunFinished {
            outcome: RunOutcome::Failed,
            ..
        }
    )));
    assert!(!format!("{events:?}").contains("PRIVATE"));
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

/// A provider failure is explained once: a streamed run tells it from its
/// stream, and the turn that delivered the message does not tell it again.
#[tokio::test]
async fn a_provider_failure_is_told_once() {
    for buffered in [false, true] {
        let kit = RoomKit::new(Default::default());
        let unreachable = ScriptedRound::answering("").refusing(503, "connect error");
        let provider = ScriptedProvider::new([unreachable]);
        let channel = prepare(&kit, provider, Arc::default(), buffered).await;
        let (client, host) = connection(NonZeroUsize::new(128).unwrap());
        let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
        next(&client).await;
        client.try_send(request("unreachable")).unwrap();
        let events = collect_operation(&client).await;
        assert_operation(&events, "unreachable", RunOutcome::Failed);
        let notices: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                Event::Notice { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            notices,
            ["Cannot reach the model server. Check its address, network and availability."],
            "buffered: {buffered}"
        );
        drop(client);
        task.await.unwrap().unwrap();
        kit.close().await.unwrap();
    }
}

#[tokio::test]
async fn a_slow_view_is_disconnected_without_stopping_another_view_or_the_engine() {
    let kit = RoomKit::new(Default::default());
    let channel = prepare(
        &kit,
        ScriptedProvider::new([ScriptedRound::answering("Shared answer")]),
        Arc::default(),
        false,
    )
    .await;
    let (slow, small) = connection(NonZeroUsize::MIN);
    let lagging =
        tokio::spawn(LocalSession::new(kit.clone(), channel.clone(), config()).run(small));
    next(&slow).await;
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let active = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    next(&client).await;
    client.try_send(request("shared-operation")).unwrap();
    let events = collect_operation(&client).await;
    assert!(events.iter().any(
        |e| matches!(e, Event::MessageFinalized { message, .. } if message.text == "Shared answer")
    ));
    assert!(matches!(lagging.await.unwrap(), Err(AdapterError::Lagged)));
    drop(slow);
    drop(client);
    active.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

#[tokio::test]
async fn buffered_delivery_respects_moderation_before_showing_the_answer() {
    let kit = RoomKit::new(Default::default());
    let channel = prepare(
        &kit,
        ScriptedProvider::new([ScriptedRound::answering("Unreviewed content")]),
        Arc::default(),
        true,
    )
    .await;
    kit.hook(
        roomkit::HookTrigger::BeforeBroadcast,
        |mut event: roomkit::model::RoomEvent, _| async move {
            if event.source.channel_id.as_str() == "agent" {
                event.content = roomkit::model::TextContent::new("Reviewed content").into();
                Ok(roomkit::model::HookResult::modify(
                    event,
                    Default::default(),
                ))
            } else {
                Ok(roomkit::model::HookResult::allow(Default::default()))
            }
        },
        hook_options("review"),
    )
    .unwrap();
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    next(&client).await;
    client.try_send(request("moderated")).unwrap();
    let events = collect_operation(&client).await;
    assert_operation(&events, "moderated", RunOutcome::Completed);
    assert!(!format!("{events:?}").contains("Unreviewed content"));
    assert!(events.iter().any(
        |e| matches!(e, Event::Item { item: Item::Message(m), .. } if m.text == "Reviewed content")
    ));
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

#[tokio::test]
async fn context_and_turn_consumption_survive_streaming_and_buffered_restart() {
    for buffered in [false, true] {
        let kit = RoomKit::new(Default::default());
        let provider = NamedProvider(ScriptedProvider::new([
            ScriptedRound::calling("Checking.", [("c1", "lookup_record", "{}")]).with_usage(
                json!({"input_tokens":100, "output_tokens":20})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            ScriptedRound::answering("Done.").with_usage(
                json!({"input_tokens":50, "cache_read_input_tokens":200, "output_tokens":10})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        ]));
        let channel = prepare(&kit, provider, Arc::new(AtomicUsize::new(0)), buffered).await;
        let mut config = config();
        config.model = Some(ModelInfo::new("local", "fixture"));
        config.context_capacity = Some(1000);
        let adapter = LocalSession::new(kit.clone(), channel.clone(), config.clone());
        let (client, host) = connection(NonZeroUsize::new(128).unwrap());
        let task = tokio::spawn(adapter.run(host));
        next(&client).await;
        client.try_send(request("measure")).unwrap();
        let events = collect_operation(&client).await;
        let (context, usage) = events
            .iter()
            .find_map(|event| match event {
                Event::UsageUpdated { context, usage, .. } => {
                    Some((context.as_ref().unwrap(), usage))
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(context.tokens, 260);
        assert_eq!(context.capacity, Some(1000));
        assert_eq!(usage.input_tokens, Some(350));
        assert_eq!(usage.output_tokens, Some(30));
        assert_eq!(usage.cached_input_tokens, 200);
        assert!(usage.complete);
        let expected = (Some(context.clone()), Some(usage.clone()));
        drop(client);
        task.await.unwrap().unwrap();
        roomkit::testing::idle(&kit).await;
        let snapshot = LocalSession::new(kit.clone(), channel.clone(), config.clone())
            .snapshot()
            .await
            .unwrap();
        assert_eq!((snapshot.context, snapshot.usage), expected);
        config.model.as_mut().unwrap().model = "another".into();
        let snapshot = LocalSession::new(kit.clone(), channel.clone(), config)
            .snapshot()
            .await
            .unwrap();
        assert!(snapshot.context.is_none() && snapshot.usage.is_none());
        kit.close().await.unwrap();
    }
}

/// RMK-1461: a priced view tells the conversation's cost after each turn, and
/// a restore sums every turn of its history again.
#[tokio::test]
async fn a_priced_conversation_tells_its_cost_and_restores_it() {
    let usage = |counters: serde_json::Value| counters.as_object().unwrap().clone();
    let kit = RoomKit::new(Default::default());
    let provider = NamedProvider(ScriptedProvider::new([
        ScriptedRound::answering("One.").with_usage(usage(json!({
            "input_tokens": 1000,
            "cache_read_input_tokens": 200,
            "cache_creation_input_tokens": 300,
            "output_tokens": 10,
        }))),
        ScriptedRound::answering("Two.")
            .with_usage(usage(json!({"input_tokens": 500, "output_tokens": 20}))),
    ]));
    let channel = prepare(&kit, provider, Arc::new(AtomicUsize::new(0)), false).await;
    let mut config = config();
    config.model = Some(ModelInfo::new("local", "fixture"));
    let mut pricing =
        roomkit_ai::ModelPricing::new(2.0, 10.0, roomkit_ai::Date::new(2026, 9, 30).unwrap())
            .unwrap();
    pricing.cache_read_per_million = Some(0.1);
    pricing.cache_write_per_million = Some(2.5);
    config.pricing = Some(pricing);
    let adapter = LocalSession::new(kit.clone(), channel.clone(), config.clone());
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let task = tokio::spawn(adapter.run(host));
    let Event::Snapshot(snapshot) = next(&client).await else {
        unreachable!("A view starts from its snapshot");
    };
    assert_eq!(snapshot.cost, Some(ConversationCost::new(0, "USD", true)));
    let told = |events: &[Event]| {
        events.iter().find_map(|event| match event {
            Event::CostUpdated { cost, .. } => Some(cost.clone()),
            _ => None,
        })
    };
    // 1,000 × 2.0 + 200 × 0.1 + 300 × 2.5 + 10 × 10.0 millionths of a dollar.
    client.try_send(request("one")).unwrap();
    let events = collect_operation(&client).await;
    assert_eq!(
        told(&events),
        Some(ConversationCost::new(2_870, "USD", true))
    );
    // And 500 × 2.0 + 20 × 10.0.
    client.try_send(request("two")).unwrap();
    let events = collect_operation(&client).await;
    assert_eq!(
        told(&events),
        Some(ConversationCost::new(4_070, "USD", true))
    );
    drop(client);
    task.await.unwrap().unwrap();
    roomkit::testing::idle(&kit).await;
    let restored = LocalSession::new(kit.clone(), channel.clone(), config.clone())
        .snapshot()
        .await
        .unwrap();
    assert_eq!(
        restored.cost,
        Some(ConversationCost::new(4_070, "USD", true))
    );
    let cost = roomkit_tui_engine::conversation_cost(&kit, &config)
        .await
        .unwrap();
    assert_eq!(cost, restored.cost);
    config.pricing = None;
    let unpriced = LocalSession::new(kit.clone(), channel, config)
        .snapshot()
        .await
        .unwrap();
    assert_eq!(unpriced.cost, None);
    kit.close().await.unwrap();
}
