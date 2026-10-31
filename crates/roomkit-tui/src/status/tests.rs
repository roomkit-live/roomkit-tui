#![allow(clippy::unwrap_used, reason = "Invalid fixtures fail the test")]

use super::*;
use crate::protocol::*;

fn session() -> Session {
    let mut session = Session::default();
    session.apply(Event::Snapshot({
        let mut snapshot = Snapshot::new("room", "Private title", Actor::new("agent", "Agent"));
        snapshot.can_interrupt = true;
        snapshot
    }));
    session
}

fn reporter() -> Reporter {
    Reporter::new(TerminalStatus::new("test-app").unwrap())
}

fn reports(bytes: &[u8]) -> Vec<&str> {
    std::str::from_utf8(bytes)
        .unwrap()
        .split("\x1b\\")
        .filter(|part| !part.is_empty())
        .map(|part| part.strip_prefix("\x1b]7501;").unwrap())
        .collect()
}

fn apply(reporter: &mut Reporter, bytes: &mut Vec<u8>, session: &mut Session, event: Event) {
    session.apply(event);
    reporter.observe(bytes, session).unwrap();
}

#[test]
fn identities_cannot_inject_pairs_controls_or_oversized_reports() {
    for invalid in [
        "",
        "app:state=done",
        "app;",
        "white space",
        "\x1b\\",
        "\n",
        "é",
        &"x".repeat(33),
    ] {
        assert!(TerminalStatus::new(invalid).is_err(), "{invalid:?}");
    }
    for valid in ["roomkit-code", "Example_1.2+3", &"x".repeat(32)] {
        assert!(TerminalStatus::new(valid).is_ok());
    }
    assert!(crate::UiOptions::default().terminal_status.is_none());
}

#[test]
fn permissions_and_finalization_follow_host_facts_without_leaking_content() {
    let mut session = session();
    let mut reporter = reporter();
    let mut bytes = vec![];
    reporter.observe(&mut bytes, &session).unwrap();
    let mut emit = |event| apply(&mut reporter, &mut bytes, &mut session, event);
    emit(Event::OperationStarted {
        conversation: "room".into(),
        operation: "op".into(),
    });
    emit(Event::RunStarted {
        conversation: "room".into(),
        run: "run".into(),
    });
    for id in ["first", "second"] {
        emit(Event::PermissionRequested {
            conversation: "room".into(),
            permission: Permission::new(id, "agent", "run", "Private action", "Private arguments"),
        });
    }
    emit(Event::Reply {
        request: RequestId::new(1),
        error: None,
    });
    emit(Event::PermissionResolved {
        conversation: "room".into(),
        permission: "first".into(),
    });
    assert_eq!(
        reports(&bytes).last(),
        Some(&"state=blocked:kind=permission:app=test-app")
    );
    reporter.drawn();
    reporter.acknowledge(&mut bytes).unwrap();
    assert_eq!(session.permissions().len(), 1);
    apply(
        &mut reporter,
        &mut bytes,
        &mut session,
        Event::PermissionResolved {
            conversation: "room".into(),
            permission: "second".into(),
        },
    );
    apply(
        &mut reporter,
        &mut bytes,
        &mut session,
        Event::RunFinished {
            conversation: "room".into(),
            run: "run".into(),
            outcome: RunOutcome::Failed,
        },
    );
    assert_eq!(reports(&bytes).last(), Some(&"state=working:app=test-app"));
    apply(
        &mut reporter,
        &mut bytes,
        &mut session,
        Event::OperationFinished {
            conversation: "room".into(),
            operation: "op".into(),
            outcome: RunOutcome::Completed,
        },
    );
    assert_eq!(
        reports(&bytes),
        [
            "state=idle:app=test-app",
            "state=working:app=test-app",
            "state=blocked:kind=permission:app=test-app",
            "state=working:app=test-app",
            "state=error:app=test-app"
        ]
    );
}

#[test]
fn results_survive_refreshes_and_are_acknowledged_only_after_display() {
    for outcome in [RunOutcome::Completed, RunOutcome::Failed] {
        let mut session = session();
        let mut reporter = reporter();
        let mut bytes = vec![];
        apply(
            &mut reporter,
            &mut bytes,
            &mut session,
            Event::OperationStarted {
                conversation: "room".into(),
                operation: "op".into(),
            },
        );
        apply(
            &mut reporter,
            &mut bytes,
            &mut session,
            Event::OperationFinished {
                conversation: "room".into(),
                operation: "op".into(),
                outcome,
            },
        );
        let completed = bytes.clone();
        reporter.acknowledge(&mut bytes).unwrap();
        for _ in 0..100 {
            apply(
                &mut reporter,
                &mut bytes,
                &mut session,
                Event::Notice {
                    conversation: "room".into(),
                    text: "Unrelated refresh".into(),
                },
            );
            reporter.drawn();
        }
        assert_eq!(bytes, completed);
        reporter.acknowledge(&mut bytes).unwrap();
        assert_eq!(reports(&bytes).last(), Some(&"state=idle:app=test-app"));
        assert_eq!(session.execution_outcome(), Some(outcome));
        let dismissed = bytes.clone();
        reporter.observe(&mut bytes, &session).unwrap();
        reporter.drawn();
        reporter.acknowledge(&mut bytes).unwrap();
        assert_eq!(bytes, dismissed);
        apply(
            &mut reporter,
            &mut bytes,
            &mut session,
            Event::OperationStarted {
                conversation: "room".into(),
                operation: "next".into(),
            },
        );
        assert_eq!(reports(&bytes).last(), Some(&"state=working:app=test-app"));
    }
}

#[test]
fn snapshots_interruption_and_cleanup_do_not_replay_completion() {
    let mut session = session();
    let mut reporter = reporter();
    let mut bytes = vec![];
    reporter.observe(&mut bytes, &Session::default()).unwrap();
    reporter.clear(&mut bytes).unwrap();
    assert!(bytes.is_empty());
    apply(
        &mut reporter,
        &mut bytes,
        &mut session,
        Event::RunStarted {
            conversation: "room".into(),
            run: "old".into(),
        },
    );
    apply(
        &mut reporter,
        &mut bytes,
        &mut session,
        Event::RunFinished {
            conversation: "room".into(),
            run: "old".into(),
            outcome: RunOutcome::Interrupted,
        },
    );
    let mut restored = session.snapshot.clone().unwrap();
    restored.conversation = "other".into();
    apply(
        &mut reporter,
        &mut bytes,
        &mut session,
        Event::Snapshot(restored),
    );
    apply(
        &mut reporter,
        &mut bytes,
        &mut session,
        Event::RunFinished {
            conversation: "room".into(),
            run: "old".into(),
            outcome: RunOutcome::Completed,
        },
    );
    reporter.clear(&mut bytes).unwrap();
    let cleared = bytes.clone();
    reporter.clear(&mut bytes).unwrap();
    assert_eq!(bytes, cleared);
    assert_eq!(
        reports(&bytes),
        [
            "state=working:app=test-app",
            "state=idle:app=test-app",
            "state=clear:app=test-app"
        ]
    );
}

#[test]
fn output_failures_propagate_and_still_allow_a_cleanup_attempt() {
    struct Failing;
    impl Write for Failing {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut reporter = reporter();
    assert_eq!(
        reporter
            .observe(&mut Failing, &session())
            .unwrap_err()
            .kind(),
        io::ErrorKind::BrokenPipe
    );
    let mut recovered = vec![];
    reporter.clear(&mut recovered).unwrap();
    assert_eq!(reports(&recovered), ["state=clear:app=test-app"]);
}
