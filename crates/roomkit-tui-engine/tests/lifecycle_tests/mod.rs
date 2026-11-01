use super::*;
use roomkit_tui_engine::{LifecycleFuture, SessionLifecycle, TurnOutcome};
use std::sync::Mutex;

#[derive(Debug, Default)]
struct Lifecycle {
    seen: Mutex<Vec<String>>,
    hold: bool,
    deny: bool,
    entered: tokio::sync::Notify,
}

impl SessionLifecycle for Lifecycle {
    fn attached(&self, _: &SessionConfig) {
        self.seen.lock().unwrap().push("attached".into());
    }
    fn admit<'a>(
        &'a self,
        _: &'a SessionConfig,
        operation: &'a OperationId,
        content: &'a roomkit::model::EventContent,
    ) -> LifecycleFuture<'a, Result<(), Rejection>> {
        Box::pin(async move {
            assert!(
                matches!(content, roomkit::model::EventContent::Text(t) if t.body == "Look up record 42")
            );
            self.seen
                .lock()
                .unwrap()
                .push(format!("admit:{}", operation.as_str()));
            self.entered.notify_one();
            if self.hold {
                std::future::pending::<()>().await;
            }
            if self.deny {
                Err("Fixture policy refused input".into())
            } else {
                Ok(())
            }
        })
    }
    fn started<'a>(
        &'a self,
        _: &'a SessionConfig,
        operation: &'a OperationId,
    ) -> LifecycleFuture<'a, ()> {
        Box::pin(async move {
            self.seen
                .lock()
                .unwrap()
                .push(format!("start:{}", operation.as_str()));
        })
    }
    fn finished<'a>(
        &'a self,
        _: &'a SessionConfig,
        operation: &'a OperationId,
        outcome: TurnOutcome,
    ) -> LifecycleFuture<'a, ()> {
        Box::pin(async move {
            self.seen
                .lock()
                .unwrap()
                .push(format!("end:{}:{outcome:?}", operation.as_str()));
        })
    }
}

#[tokio::test]
async fn callbacks_cover_one_whole_turn_across_tool_rounds_and_survive_detach() {
    let kit = RoomKit::new(Default::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let channel = prepare(
        &kit,
        ScriptedProvider::new([
            ScriptedRound::calling("Inspecting", [("a", "lookup_record", "{}")]),
            ScriptedRound::calling("Checking", [("b", "lookup_record", "{}")]),
            ScriptedRound::answering("Done"),
        ]),
        calls.clone(),
        false,
    )
    .await;
    let gate = Arc::new(tokio::sync::Notify::new());
    let reached = Arc::new(tokio::sync::Notify::new());
    let release = gate.clone();
    let entered = reached.clone();
    kit.before_tool_use("hold-first", move |event, _| {
        let (release, entered) = (release.clone(), entered.clone());
        async move {
            if event.call.id == "a" {
                entered.notify_one();
                release.notified().await;
            }
            Ok(roomkit::ToolUse::Allow)
        }
    })
    .unwrap();
    let lifecycle = Arc::new(Lifecycle::default());
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let task = tokio::spawn(
        LocalSession::new(kit.clone(), channel.clone(), config())
            .with_lifecycle(lifecycle.clone())
            .run(host),
    );
    next(&client).await;
    client.try_send(request("whole-turn")).unwrap();
    tokio::time::timeout(Duration::from_secs(5), reached.notified())
        .await
        .unwrap();
    drop(client);
    task.await.unwrap().unwrap();
    assert_eq!(
        *lifecycle.seen.lock().unwrap(),
        ["attached", "admit:whole-turn", "start:whole-turn"]
    );
    gate.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if lifecycle
                .seen
                .lock()
                .unwrap()
                .last()
                .is_some_and(|s| s == "end:whole-turn:Completed")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(lifecycle.seen.lock().unwrap().len(), 4);
    kit.close().await.unwrap();
}

#[tokio::test]
async fn rejected_admission_does_not_start_a_turn_or_enter_history() {
    let kit = RoomKit::new(Default::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let channel = prepare(
        &kit,
        ScriptedProvider::new([ScriptedRound::answering("Should not run")]),
        calls,
        false,
    )
    .await;
    let lifecycle = Arc::new(Lifecycle {
        deny: true,
        ..Default::default()
    });
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let task = tokio::spawn(
        LocalSession::new(kit.clone(), channel.clone(), config())
            .with_lifecycle(lifecycle.clone())
            .run(host),
    );
    next(&client).await;
    client.try_send(request("refused")).unwrap();
    let events = collect_operation(&client).await;
    assert_operation(&events, "refused", RunOutcome::Failed);
    assert_eq!(
        *lifecycle.seen.lock().unwrap(),
        ["attached", "admit:refused", "end:refused:Blocked"]
    );
    let snapshot = LocalSession::new(kit.clone(), channel, config())
        .snapshot()
        .await
        .unwrap();
    assert!(snapshot.items.is_empty());
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

#[tokio::test]
async fn preparation_can_be_interrupted_and_stale_run_ids_cannot_cancel_it() {
    let kit = RoomKit::new(Default::default());
    let channel = prepare(
        &kit,
        ScriptedProvider::new([]),
        Arc::new(AtomicUsize::new(0)),
        false,
    )
    .await;
    let lifecycle = Arc::new(Lifecycle {
        hold: true,
        ..Default::default()
    });
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let task = tokio::spawn(
        LocalSession::new(kit.clone(), channel, config())
            .with_lifecycle(lifecycle.clone())
            .run(host),
    );
    next(&client).await;
    client.try_send(request("prepare")).unwrap();
    tokio::time::timeout(Duration::from_secs(5), lifecycle.entered.notified())
        .await
        .unwrap();
    let interrupt = |id, run: &str| {
        Request::new(
            RequestId::new(id),
            format!("interrupt-{id}"),
            "room",
            Command::Interrupt { run: run.into() },
        )
    };
    client.try_send(interrupt(2, "old-run")).unwrap();
    loop {
        if let Event::Reply { request, error } = next(&client).await
            && request == RequestId::new(2)
        {
            assert!(error.is_some());
            break;
        }
    }
    client.try_send(interrupt(3, "prepare:prepare")).unwrap();
    let events = collect_operation(&client).await;
    assert!(matches!(
        events.last(),
        Some(Event::OperationFinished {
            outcome: RunOutcome::Interrupted,
            ..
        })
    ));
    assert_eq!(
        *lifecycle.seen.lock().unwrap(),
        ["attached", "admit:prepare", "end:prepare:Interrupted"]
    );
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

/// RMK-1079: `prepare:{operation}` reached 264 bytes for a 256-byte operation,
/// so the terminal ignored the run and Esc had nothing to interrupt.
#[tokio::test]
async fn a_long_operation_id_still_gives_its_preparation_a_run_to_interrupt() {
    let kit = RoomKit::new(Default::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let channel = prepare(&kit, ScriptedProvider::new([]), calls, false).await;
    let lifecycle = Arc::new(Lifecycle {
        hold: true,
        ..Default::default()
    });
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let session = LocalSession::new(kit.clone(), channel, config()).with_lifecycle(lifecycle);
    let task = tokio::spawn(session.run(host));
    next(&client).await;
    let operation = "o".repeat(roomkit_tui_protocol::limits::MAX_OPERATION_ID_BYTES);
    client.try_send(request(&operation)).unwrap();
    let run = loop {
        if let Event::RunStarted { run, .. } = next(&client).await {
            break run;
        }
    };
    let bytes = run.as_str().len();
    assert!(
        bytes <= roomkit_tui_protocol::limits::MAX_LABEL_BYTES,
        "A run id of {bytes} bytes is ignored"
    );
    let interrupt = Request::new(
        RequestId::new(2),
        "interrupt",
        "room",
        Command::Interrupt { run },
    );
    client.try_send(interrupt).unwrap();
    let events = collect_operation(&client).await;
    assert!(matches!(
        events.last(),
        Some(Event::OperationFinished {
            outcome: RunOutcome::Interrupted,
            ..
        })
    ));
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}
