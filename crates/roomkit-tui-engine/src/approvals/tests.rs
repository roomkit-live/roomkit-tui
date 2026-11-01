#![allow(clippy::unwrap_used, reason = "Test fixture failures are assertions")]
use super::*;

async fn requested(events: &mut broadcast::Receiver<(String, Event)>) -> PermissionId {
    let (_, Event::PermissionRequested { permission, .. }) = events.recv().await.unwrap() else {
        unreachable!("Expected a permission request");
    };
    permission.id
}

fn asking(broker: &Approvals, timeout: Duration) -> tokio::task::JoinHandle<Decision> {
    let broker = broker.clone();
    tokio::spawn(async move {
        broker
            .ask(
                "room",
                "agent",
                Details {
                    title: "Change file".into(),
                    detail: "Concrete action".into(),
                    ..Default::default()
                },
                timeout,
            )
            .await
    })
}

#[tokio::test]
async fn answers_are_scoped_single_use_and_withdrawn_on_disconnect() {
    let (events, mut receive) = broadcast::channel(32);
    let broker = Approvals::new(events);
    assert_eq!(
        asking(&broker, Duration::from_secs(1)).await.unwrap(),
        Decision::Deny
    );
    let view = broker.attach("room", "agent");
    let _run = broker.start("room", "agent", "run".into(), None);
    let answer = asking(&broker, Duration::from_secs(1));
    let id = requested(&mut receive).await;
    assert!(
        broker
            .decide(("elsewhere", "agent"), &id, Decision::Allow, "user")
            .is_err()
    );
    assert!(
        broker
            .decide(("room", "other"), &id, Decision::Allow, "user")
            .is_err()
    );
    assert!(!answer.is_finished());
    broker
        .decide(("room", "agent"), &id, Decision::Allow, "user")
        .unwrap();
    assert!(
        matches!(receive.recv().await.unwrap().1, Event::Item { item: Item::Approval(record), .. } if record.decision == Decision::Allow && record.decided_by.as_str() == "person:user")
    );
    assert!(matches!(
        receive.recv().await.unwrap().1,
        Event::PermissionResolved { .. }
    ));
    assert_eq!(answer.await.unwrap(), Decision::Allow);
    assert!(
        broker
            .decide(("room", "agent"), &id, Decision::Allow, "user")
            .is_err()
    );
    let answer = asking(&broker, Duration::from_secs(1));
    let second = requested(&mut receive).await;
    assert_ne!(id, second);
    drop(view);
    assert_eq!(answer.await.unwrap(), Decision::Deny);
    assert!(matches!(
        receive.recv().await.unwrap().1,
        Event::PermissionResolved { .. }
    ));
}

#[tokio::test]
async fn timeout_cancel_and_run_end_never_leave_approvable_requests() {
    let (events, mut receive) = broadcast::channel(32);
    let broker = Approvals::new(events);
    let _view = broker.attach("room", "agent");
    let run = broker.start("room", "agent", "run".into(), None);
    let answer = asking(&broker, Duration::from_millis(30));
    let id = requested(&mut receive).await;
    assert_eq!(answer.await.unwrap(), Decision::Deny);
    assert!(
        broker
            .decide(("room", "agent"), &id, Decision::Allow, "user")
            .is_err()
    );
    assert!(matches!(
        receive.recv().await.unwrap().1,
        Event::PermissionResolved { .. }
    ));
    let answer = asking(&broker, Duration::from_secs(1));
    let id = requested(&mut receive).await;
    answer.abort();
    let _ = answer.await;
    assert!(
        broker
            .decide(("room", "agent"), &id, Decision::Allow, "user")
            .is_err()
    );
    assert!(matches!(
        receive.recv().await.unwrap().1,
        Event::PermissionResolved { .. }
    ));
    let answer = asking(&broker, Duration::from_secs(1));
    requested(&mut receive).await;
    drop(run);
    assert_eq!(answer.await.unwrap(), Decision::Deny);
    assert!(broker.state.lock().unwrap().pending.is_empty());
    assert!(
        broker.receipts("room").is_empty(),
        "Withdrawal is not a human refusal"
    );
    assert_eq!(
        broker
            .ask(
                "room",
                "agent",
                Details {
                    title: "title".into(),
                    detail: "x".repeat(65_537),
                    ..Default::default()
                },
                Duration::from_secs(1),
            )
            .await,
        Decision::Deny
    );
}

fn details() -> Details {
    Details {
        title: "Change file".into(),
        detail: "Concrete action".into(),
        ..Default::default()
    }
}

/// Review finding 15: an answer arriving as the request expired was
/// recorded as a human approval although the action never received it.
#[tokio::test]
async fn an_answer_the_action_never_received_is_not_recorded() {
    let (events, mut receive) = broadcast::channel(32);
    let broker = Approvals::new(events);
    let _view = broker.attach("room", "agent");
    let _run = broker.start("room", "agent", "run".into(), None);
    let scope = ("room".to_owned(), "agent".to_owned());
    let Registration::Waiting(_guard, receiver) = broker.register(&scope, &details()) else {
        unreachable!("A view and a run are attached");
    };
    let id = requested(&mut receive).await;
    drop(receiver);
    let answer = broker.decide(("room", "agent"), &id, Decision::Allow, "user");
    assert!(answer.is_err(), "The expired request cannot be answered");
    assert!(broker.receipts("room").is_empty(), "Nothing was decided");
    assert!(matches!(
        receive.recv().await.unwrap().1,
        Event::PermissionResolved { .. }
    ));
    assert!(
        receive.try_recv().is_err(),
        "No approval receipt was published"
    );
}

/// Review finding 15: any view of the room could answer a run another
/// person started.
#[tokio::test]
async fn only_the_requester_answers_a_run_they_started() {
    let (events, mut receive) = broadcast::channel(32);
    let broker = Approvals::new(events);
    let _view = broker.attach("room", "agent");
    let _run = broker.start("room", "agent", "run".into(), Some("alice".into()));
    let broker_task = broker.clone();
    let answer = tokio::spawn(async move {
        broker_task
            .ask("room", "agent", details(), Duration::from_secs(5))
            .await
    });
    let id = requested(&mut receive).await;
    let refused = broker.decide(("room", "agent"), &id, Decision::Allow, "bob");
    assert_eq!(
        refused.unwrap_err().code,
        roomkit_tui_protocol::ErrorCode::Refused
    );
    broker
        .decide(("room", "agent"), &id, Decision::Allow, "alice")
        .unwrap();
    assert_eq!(answer.await.unwrap(), Decision::Allow);
}

/// The next event, which must be a notice published within a second.
async fn notice(events: &mut broadcast::Receiver<(String, Event)>) -> String {
    match tokio::time::timeout(Duration::from_secs(1), events.recv()).await {
        Ok(Ok((_, Event::Notice { text, .. }))) => text,
        other => unreachable!("Expected a notice, got {other:?}"),
    }
}

/// RMK-1079: the broker kept its own copy of the terminal's rule, without
/// the identity bound, so a request a terminal hides waited for its timeout.
/// RMK-1104: the person was not told, and saw a denial they did not make.
#[tokio::test]
async fn a_request_a_terminal_cannot_show_in_full_is_denied_at_once() {
    let (events, mut receive) = broadcast::channel(32);
    let broker = Approvals::new(events);
    let _view = broker.attach("room", "agent");
    let oversized = Details {
        detail: "x".repeat(roomkit_tui_protocol::limits::MAX_PERMISSION_DETAIL_BYTES + 1),
        ..details()
    };
    let long_run = RunId::new("r".repeat(roomkit_tui_protocol::limits::MAX_LABEL_BYTES));
    for (run, details) in [("run".into(), oversized), (long_run, details())] {
        let _run = broker.start("room", "agent", run, None);
        let decision = broker.ask("room", "agent", details, Duration::from_secs(60));
        let decision = tokio::time::timeout(Duration::from_secs(1), decision).await;
        assert_eq!(decision.ok(), Some(Decision::Deny));
        assert_eq!(
            notice(&mut receive).await,
            "An approval was too large to show in full; it was refused."
        );
    }
    assert!(receive.try_recv().is_err(), "No request was shown");
}

/// RMK-1104: a request beyond the pending cap was refused without a word.
#[tokio::test]
async fn a_request_beyond_the_pending_cap_is_refused_and_the_person_told() {
    let (events, mut receive) = broadcast::channel(MAX_PENDING_PERMISSIONS + 8);
    let broker = Approvals::new(events);
    let _view = broker.attach("room", "agent");
    let _run = broker.start("room", "agent", "run".into(), None);
    let _waiting: Vec<_> = (0..MAX_PENDING_PERMISSIONS)
        .map(|_| asking(&broker, Duration::from_secs(60)))
        .collect();
    for _ in 0..MAX_PENDING_PERMISSIONS {
        requested(&mut receive).await;
    }
    let decision = broker.ask("room", "agent", details(), Duration::from_secs(60));
    let decision = tokio::time::timeout(Duration::from_secs(1), decision).await;
    assert_eq!(decision.ok(), Some(Decision::Deny));
    assert_eq!(
        notice(&mut receive).await,
        "Too many approvals are waiting; a new one was refused."
    );
}
