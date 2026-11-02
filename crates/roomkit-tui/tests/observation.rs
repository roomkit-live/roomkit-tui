//! Generic session facts are observable without a terminal or external service.
#![allow(clippy::unwrap_used, reason = "Invalid fixture state fails the test")]

use roomkit_tui::{Session, SessionObserver, protocol::*};

fn snapshot(room: &str) -> Snapshot {
    {
        let mut snapshot = Snapshot::new(room, "Order support", Actor::new("support", "Support"));
        snapshot.can_interrupt = true;
        snapshot
    }
}

fn operation_started() -> Event {
    Event::OperationStarted {
        conversation: "orders".into(),
        operation: "request-1".into(),
    }
}

fn operation_finished(outcome: RunOutcome) -> Event {
    Event::OperationFinished {
        conversation: "orders".into(),
        operation: "request-1".into(),
        outcome,
    }
}

#[test]
fn accepted_work_is_observable_and_busy_before_a_run_and_after_its_stream() {
    let observer = SessionObserver::default();
    let mut session = Session::default().with_observer(observer.clone());
    assert!(observer.latest().is_none());
    session.apply(Event::Snapshot(snapshot("orders")));
    session.apply(operation_started());
    let admitted = observer.latest().unwrap();
    assert_eq!(admitted.operation.unwrap().outcome, None);
    assert!(admitted.run.is_none());
    session.insert("Another order");
    assert!(session.is_executing());
    assert!(session.input_command().is_err());

    session.apply(Event::RunStarted {
        conversation: "orders".into(),
        run: "lookup".into(),
        answers: None,
    });
    session.apply(Event::RunFinished {
        conversation: "orders".into(),
        run: "lookup".into(),
        outcome: RunOutcome::Failed,
    });
    assert!(
        session.is_executing(),
        "Stream closure is not operation cleanup"
    );
    assert!(session.execution_outcome().is_none());
    session.apply(operation_finished(RunOutcome::Completed));
    assert!(!session.is_executing());
    assert_eq!(session.execution_outcome(), Some(RunOutcome::Failed));
    let settled = observer.latest().unwrap();
    assert_eq!(settled.run.unwrap().outcome, Some(RunOutcome::Failed));
    assert_eq!(
        settled.operation.unwrap().outcome,
        Some(RunOutcome::Completed)
    );
    assert!(session.input_command().is_ok());
}

#[test]
fn admission_failure_is_scoped_and_a_new_snapshot_does_not_replay_completion() {
    let observer = SessionObserver::default();
    let mut session = Session::default().with_observer(observer.clone());
    session.apply(Event::Snapshot(snapshot("orders")));
    session.apply(operation_started());
    for (room, operation) in [("other", "request-1"), ("orders", "stale")] {
        session.apply(Event::OperationFinished {
            conversation: room.into(),
            operation: operation.into(),
            outcome: RunOutcome::Completed,
        });
        assert!(session.is_executing());
    }
    session.apply(operation_finished(RunOutcome::Failed));
    assert_eq!(session.execution_outcome(), Some(RunOutcome::Failed));
    assert!(observer.latest().unwrap().run.is_none());
    session.apply(operation_finished(RunOutcome::Completed));
    assert_eq!(session.execution_outcome(), Some(RunOutcome::Failed));
    session.apply(Event::Snapshot(snapshot("other")));
    session.apply(operation_finished(RunOutcome::Failed));
    let restored = observer.latest().unwrap();
    assert_eq!(restored.conversation.as_str(), "other");
    assert!(restored.operation.is_none() && restored.run.is_none());
    assert!(session.execution_outcome().is_none());
}

#[test]
fn permissions_stay_open_until_the_host_settles_each_decision() {
    let observer = SessionObserver::default();
    let mut session = Session::default().with_observer(observer.clone());
    session.apply(Event::Snapshot(snapshot("orders")));
    session.apply(Event::RunStarted {
        conversation: "orders".into(),
        run: "lookup".into(),
        answers: None,
    });
    for id in ["first", "second"] {
        session.apply(Event::PermissionRequested {
            conversation: "orders".into(),
            permission: Permission::new(
                id,
                "support",
                "lookup",
                "Read record?",
                "Private arguments",
            ),
        });
    }
    let request = session
        .request(session.decide(Decision::Allow).unwrap())
        .unwrap();
    session.sent(&request);
    session.apply(Event::Reply {
        request: request.id,
        error: None,
    });
    assert_eq!(observer.latest().unwrap().permissions.len(), 2);
    session.apply(Event::PermissionResolved {
        conversation: "orders".into(),
        permission: "first".into(),
    });
    assert_eq!(
        observer.latest().unwrap().permissions,
        vec![PermissionId::new("second")]
    );
    session.apply(Event::PermissionResolved {
        conversation: "other".into(),
        permission: "second".into(),
    });
    assert_eq!(observer.latest().unwrap().permissions.len(), 1);
    session.apply(Event::RunFinished {
        conversation: "orders".into(),
        run: "stale".into(),
        outcome: RunOutcome::Interrupted,
    });
    assert_eq!(observer.latest().unwrap().permissions.len(), 1);
    session.apply(Event::RunFinished {
        conversation: "orders".into(),
        run: "lookup".into(),
        outcome: RunOutcome::Interrupted,
    });
    let settled = observer.latest().unwrap();
    assert!(settled.permissions.is_empty());
    assert_eq!(
        settled.run.as_ref().unwrap().outcome,
        Some(RunOutcome::Interrupted)
    );
    assert!(!format!("{settled:?}").contains("Private"));
}

#[test]
fn a_late_run_start_cannot_reopen_the_previous_operations_execution() {
    let mut session = Session::default();
    session.apply(Event::Snapshot(snapshot("orders")));
    let started = Event::RunStarted {
        conversation: "orders".into(),
        run: "old".into(),
        answers: None,
    };
    let finished = Event::RunFinished {
        conversation: "orders".into(),
        run: "old".into(),
        outcome: RunOutcome::Failed,
    };
    session.apply(started.clone());
    session.apply(finished.clone());
    session.apply(operation_started());
    session.apply(started);
    session.apply(finished);
    assert!(session.observation().unwrap().run.is_none());
    session.apply(operation_finished(RunOutcome::Completed));
    assert_eq!(session.execution_outcome(), Some(RunOutcome::Completed));
    assert!(!session.is_executing());
}

#[test]
fn restored_activity_and_validated_mode_changes_keep_permissions_open() {
    let observer = SessionObserver::default();
    let mut session = Session::default();
    let mut state = snapshot("orders");
    state.commands.push(CommandSpec::new("policy", "Policy"));
    let mut mode = ModeState::new(
        "policy",
        "observe",
        ["observe", "assist"]
            .map(|value| Choice::new(value, value))
            .to_vec(),
    );
    state.mode = Some(mode.clone());
    state.active_run = Some("restored".into());
    state.permissions.push(Permission::new(
        "pending",
        "support",
        "restored",
        "Read record?",
        "Details",
    ));
    session.apply(Event::Snapshot(state));
    let mut session = session.with_observer(observer.clone());
    let restored = observer.latest().unwrap();
    assert_eq!(restored.run.as_ref().unwrap().id.as_str(), "restored");
    assert_eq!(restored.run.as_ref().unwrap().outcome, None);
    assert!(restored.operation.is_none());
    mode.selected = "unknown".into();
    session.apply(Event::ModeChanged {
        conversation: "orders".into(),
        mode: mode.clone(),
    });
    assert_eq!(observer.latest(), Some(restored.clone()));
    mode.selected = "assist".into();
    session.apply(Event::ModeChanged {
        conversation: "orders".into(),
        mode: mode.clone(),
    });
    let changed = observer.latest().unwrap();
    assert_eq!(changed.mode, Some(mode));
    assert_eq!(changed.permissions, restored.permissions);
    assert_eq!(changed.run, restored.run);
    session.apply(Event::Snapshot(snapshot("other")));
    assert!(observer.latest().unwrap().permissions.is_empty());
    assert!(observer.latest().unwrap().run.is_none());
}

#[test]
fn optional_observation_keeps_only_latest_facts_and_never_changes_the_session() {
    let observer = SessionObserver::default();
    let mut observed = Session::default().with_observer(observer.clone());
    let mut plain = Session::default();
    let mut events = vec![
        Event::Snapshot(snapshot("orders")),
        operation_started(),
        operation_finished(RunOutcome::Failed),
    ];
    events.extend((0..1000).map(|index| {
        let mut state = snapshot("orders");
        state.title = format!("Order {index}");
        Event::Snapshot(state)
    }));
    for event in events {
        observed.apply(event.clone());
        plain.apply(event);
    }
    assert_eq!(observer.latest(), plain.observation());
    assert_eq!(observer.latest().unwrap().title, "Order 999");
    let latest = observer.latest();
    observed.apply(Event::Notice {
        conversation: "orders".into(),
        text: "Private diagnostics".into(),
    });
    assert_eq!(observer.latest(), latest);
    drop(observed);
    assert_eq!(
        observer.latest(),
        latest,
        "Consumer lifetime is application-owned"
    );
}

#[test]
fn a_title_change_is_observed_in_its_own_view_only() {
    let observer = SessionObserver::default();
    let mut session = Session::default().with_observer(observer.clone());
    session.apply(Event::Snapshot(snapshot("orders")));
    session.apply(operation_started());
    let before = observer.latest().unwrap();
    session.apply(Event::TitleChanged {
        conversation: "other".into(),
        title: "Elsewhere".into(),
    });
    assert_eq!(observer.latest().unwrap(), before);

    session.apply(Event::TitleChanged {
        conversation: "orders".into(),
        title: "Refund for order 42".into(),
    });
    let renamed = observer.latest().unwrap();
    assert_eq!(renamed.title, "Refund for order 42");
    assert_eq!(
        renamed.operation, before.operation,
        "The operation is untouched"
    );
    assert_eq!(Some(renamed), session.observation());
}
