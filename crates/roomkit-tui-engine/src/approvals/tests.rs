#![allow(clippy::unwrap_used, reason = "Test fixture failures are assertions")]
use super::admission::Registration;
use super::*;
use roomkit_tui_protocol::limits::MAX_PENDING_PERMISSIONS;
use std::time::Duration;

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
async fn next_notice(events: &mut broadcast::Receiver<(String, Event)>) -> String {
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
            next_notice(&mut receive).await,
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
        next_notice(&mut receive).await,
        "Too many approvals are waiting; a new one was refused."
    );
}

const TOO_LARGE: &str = "An approval was too large to show in full; it was refused.";

fn oversized() -> Details {
    Details {
        detail: "x".repeat(roomkit_tui_protocol::limits::MAX_PERMISSION_DETAIL_BYTES + 1),
        ..details()
    }
}

/// RMK-1104 review: a host asking again flooded the view with one notice per
/// refusal, until it lagged and detached.
#[tokio::test]
async fn a_refusal_is_told_once_per_run() {
    let (events, mut receive) = broadcast::channel(8);
    let broker = Approvals::new(events);
    let _view = broker.attach("room", "agent");
    for run in ["run-1", "run-2"] {
        let _run = broker.start("room", "agent", run.into(), None);
        for _ in 0..3 {
            let decision = broker.ask("room", "agent", oversized(), Duration::from_secs(5));
            assert_eq!(decision.await, Decision::Deny);
        }
        assert_eq!(next_notice(&mut receive).await, TOO_LARGE);
        assert!(receive.try_recv().is_err(), "Told once during {run}");
    }
}

/// RMK-1104 review: a refusal decided before its run was told at once, and
/// the terminal erased the notice when the run started.
#[tokio::test]
async fn a_refusal_before_its_run_is_told_once_the_run_has_started() {
    let (events, mut receive) = broadcast::channel(MAX_PENDING_PERMISSIONS + 8);
    let broker = Approvals::new(events);
    let _busy_view = broker.attach("busy", "agent");
    let _busy_run = broker.start("busy", "agent", "busy-run".into(), None);
    let _waiting: Vec<_> = (0..MAX_PENDING_PERMISSIONS)
        .map(|_| {
            let broker = broker.clone();
            tokio::spawn(async move {
                let wait = Duration::from_secs(60);
                broker.ask("busy", "agent", details(), wait).await
            })
        })
        .collect();
    for _ in 0..MAX_PENDING_PERMISSIONS {
        requested(&mut receive).await;
    }
    let _view = broker.attach("room", "agent");
    let asking = {
        let broker = broker.clone();
        tokio::spawn(async move {
            let wait = Duration::from_secs(5);
            broker.ask("room", "agent", details(), wait).await
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        receive.try_recv().is_err(),
        "Nothing is said before the run"
    );
    let _run = broker.start("room", "agent", "run".into(), None);
    assert_eq!(
        next_notice(&mut receive).await,
        "Too many approvals are waiting; a new one was refused."
    );
    assert_eq!(asking.await.unwrap(), Decision::Deny);
}

/// The next event, which must be a request published within a second.
async fn requested_permission(events: &mut broadcast::Receiver<(String, Event)>) -> Permission {
    match tokio::time::timeout(Duration::from_secs(1), events.recv()).await {
        Ok(Ok((_, Event::PermissionRequested { permission, .. }))) => permission,
        other => unreachable!("Expected a permission request, got {other:?}"),
    }
}

/// Ask in `room`/`agent`, in the run started with `token` when given.
fn ask_in(broker: &Approvals, token: Option<u64>) -> tokio::task::JoinHandle<Decision> {
    let broker = broker.clone();
    let details = Details {
        run: token,
        ..details()
    };
    tokio::spawn(async move {
        broker
            .ask("room", "agent", details, Duration::from_secs(5))
            .await
    })
}

/// RMK-1111: the broker kept one run per scope, so a later run erased the
/// earlier one. A request naming its run stays in it and keeps its
/// requester; one naming none goes to the most recent run, and to the
/// earlier one again once the later run ends.
#[tokio::test]
async fn a_request_belongs_to_its_run_while_a_later_one_holds_the_scope() {
    let (events, mut receive) = broadcast::channel(16);
    let broker = Approvals::new(events);
    let _view = broker.attach("room", "agent");
    let host = broker.start("room", "agent", "deploy-1".into(), Some("alice".into()));
    let turn = broker.start("room", "agent", "agent:evt_9".into(), None);

    let named = ask_in(&broker, Some(host.token()));
    let permission = requested_permission(&mut receive).await;
    assert_eq!(permission.run.as_str(), "deploy-1");
    let bob = broker.decide(("room", "agent"), &permission.id, Decision::Allow, "bob");
    assert_eq!(
        bob.unwrap_err().code,
        roomkit_tui_protocol::ErrorCode::Refused
    );
    broker
        .decide(("room", "agent"), &permission.id, Decision::Allow, "alice")
        .unwrap();
    assert_eq!(named.await.unwrap(), Decision::Allow);
    while receive.try_recv().is_ok() {}

    let latest = ask_in(&broker, None);
    assert_eq!(
        requested_permission(&mut receive).await.run.as_str(),
        "agent:evt_9"
    );
    drop(turn);
    assert_eq!(
        latest.await.unwrap(),
        Decision::Deny,
        "Withdrawn with its run"
    );
    while receive.try_recv().is_ok() {}
    let _back = ask_in(&broker, None);
    assert_eq!(
        requested_permission(&mut receive).await.run.as_str(),
        "deploy-1"
    );
}

/// RMK-1111 review: interrupting host work while a later run held the scope
/// must stop the work, not that later run.
#[tokio::test]
async fn interrupting_a_run_leaves_a_later_one_answerable() {
    let (events, mut receive) = broadcast::channel(16);
    let broker = Approvals::new(events);
    let _view = broker.attach("room", "agent");
    let host = broker.start("room", "agent", "deploy-1".into(), Some("alice".into()));
    let _turn = broker.start("room", "agent", "agent:evt_9".into(), None);
    broker.stop_run(&("room".into(), "agent".into()), host.token());
    let stopped = ask_in(&broker, Some(host.token()));
    assert_eq!(stopped.await.unwrap(), Decision::Deny);
    let _latest = ask_in(&broker, None);
    let permission = requested_permission(&mut receive).await;
    assert_eq!(
        permission.run.as_str(),
        "agent:evt_9",
        "The turn still asks"
    );
}

/// RMK-1111 review: a context outliving its work named its run by identity,
/// so it reached a later run under the same identity and that run's person
/// answered.
#[tokio::test]
async fn a_request_naming_an_ended_run_is_refused_even_if_its_identity_returns() {
    let (events, mut receive) = broadcast::channel(16);
    let broker = Approvals::new(events);
    let _view = broker.attach("room", "agent");
    let first = broker.start("room", "agent", "deploy-1".into(), Some("alice".into()));
    let leaked = first.token();
    drop(first);
    let _again = broker.start("room", "agent", "deploy-1".into(), Some("bob".into()));
    let decision =
        tokio::time::timeout(Duration::from_secs(1), ask_in(&broker, Some(leaked))).await;
    assert_eq!(decision.unwrap().unwrap(), Decision::Deny);
    assert!(
        receive.try_recv().is_err(),
        "Nothing was shown in Bob's run"
    );
}

/// RMK-1110 review: the last run of every room was kept for the life of the
/// channel. It is kept while a view of the room exists, for any agent, and
/// for the room announced last, as a view may be about to attach there.
#[test]
fn the_last_run_of_a_room_is_kept_while_a_view_watches_it() {
    let (events, _receive) = broadcast::channel(4);
    let broker = Approvals::new(events);
    broker.announced("room", &"early".into());
    assert_eq!(
        broker.last_run("room"),
        Some("early".into()),
        "A view may attach"
    );
    broker.announced("other", &"elsewhere".into());
    assert_eq!(broker.last_run("room"), None, "Nobody watched the room");

    let first = broker.attach("room", "agent");
    let second = broker.attach("room", "helper");
    broker.announced("room", &"build".into());
    broker.announced("other", &"elsewhere-2".into());
    drop(first);
    assert_eq!(
        broker.last_run("room"),
        Some("build".into()),
        "Another agent's view"
    );
    drop(second);
    assert_eq!(broker.last_run("room"), None, "The last view detached");
}
