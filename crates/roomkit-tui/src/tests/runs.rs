//! Several runs live at once in a room: host work a person started, and an
//! agent turn that another channel's message started meanwhile.

#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]

use std::time::Duration;

use roomkit_tui_protocol::*;

use super::*;

fn start(run: &str) -> Event {
    Event::RunStarted {
        conversation: "room-a".into(),
        run: run.into(),
    }
}

fn finish(run: &str, outcome: RunOutcome) -> Event {
    Event::RunFinished {
        conversation: "room-a".into(),
        run: run.into(),
        outcome,
    }
}

fn ask(id: &str, run: &str) -> Event {
    Event::PermissionRequested {
        conversation: "room-a".into(),
        permission: Permission::new(id, "agent", run, "Deploy?", "Deploy to production"),
    }
}

fn shown(session: &crate::Session) -> Vec<&str> {
    session
        .permissions()
        .iter()
        .map(|permission| permission.id.as_str())
        .collect()
}

fn seconds(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// RMK-1135: the terminal followed one run, the last started. An agent turn
/// for another channel's message replaced the person's `/deploy`: its
/// requests were no longer shown and expired, and Esc aimed at the turn.
#[test]
fn a_run_started_meanwhile_leaves_the_earlier_one_visible_and_answerable() {
    let mut session = session();
    session.apply_at(start("deploy-1"), seconds(0));
    session.apply(ask("p1", "deploy-1"));
    assert_eq!(shown(&session), ["p1"]);

    session.apply_at(start("agent:sms-42"), seconds(10));
    let deploy = RunId::from("deploy-1");
    assert_eq!(session.active_run(), Some(&deploy), "Esc follows the work");
    assert_eq!(session.run_elapsed(seconds(25)), Some(seconds(25)));
    session.apply(ask("p2", "deploy-1"));
    assert_eq!(shown(&session), ["p1", "p2"], "Shown during the turn");
    session.apply(Event::TextDelta {
        conversation: "room-a".into(),
        message: "draft-sms".into(),
        actor: "agent".into(),
        run: "agent:sms-42".into(),
        text: "Hello Carol".into(),
    });
    let items = &session.snapshot().unwrap().items;
    assert!(
        items
            .iter()
            .any(|item| matches!(item, Item::Message(m) if m.text == "Hello Carol"))
    );

    session.apply_at(finish("agent:sms-42", RunOutcome::Completed), seconds(30));
    assert_eq!(session.active_run(), Some(&deploy));
    assert!(session.run_outcome().is_none(), "The work still runs");
    session.apply(ask("p3", "deploy-1"));
    assert_eq!(shown(&session), ["p1", "p2", "p3"], "Shown after the turn");
    assert!(
        session
            .request(Command::Interrupt {
                run: deploy.clone()
            })
            .is_ok()
    );

    session.apply_at(finish("deploy-1", RunOutcome::Interrupted), seconds(40));
    assert!(session.active_run().is_none());
    assert!(session.permissions().is_empty());
    assert_eq!(session.run_outcome(), Some(RunOutcome::Interrupted));
    assert_eq!(session.run_elapsed(seconds(50)), Some(seconds(40)));
}

/// A request of the later run is shown beside the earlier run's.
#[test]
fn every_live_run_can_ask() {
    let mut session = session();
    session.apply(start("deploy-1"));
    session.apply(start("agent:sms-42"));
    session.apply(ask("from-turn", "agent:sms-42"));
    session.apply(ask("from-work", "deploy-1"));
    assert_eq!(shown(&session), ["from-turn", "from-work"]);
    session.apply(finish("agent:sms-42", RunOutcome::Completed));
    assert_eq!(
        shown(&session),
        ["from-work"],
        "A finished run's requests go"
    );
}

/// A run starting meanwhile neither restarts the work's timer nor forgets an
/// interruption already on its way.
#[test]
fn a_run_started_meanwhile_keeps_the_work_s_timer_and_interruption() {
    let mut session = session();
    session.apply_at(start("deploy-1"), seconds(0));
    let interrupt = session
        .request(Command::Interrupt {
            run: "deploy-1".into(),
        })
        .unwrap();
    session.sent(&interrupt);
    assert!(session.interrupting());
    session.apply_at(start("agent:sms-42"), seconds(5));
    assert!(session.interrupting(), "Still interrupting the work");
    assert_eq!(session.run_elapsed(seconds(8)), Some(seconds(8)));
}

/// A replayed start of a live run, or of the last run started, is ignored.
#[test]
fn a_replayed_start_does_not_duplicate_or_reopen_a_run() {
    let mut session = session();
    session.apply(start("deploy-1"));
    session.apply(start("agent:sms-42"));
    session.apply(start("deploy-1"));
    session.apply(finish("deploy-1", RunOutcome::Completed));
    assert_eq!(session.active_run(), Some(&RunId::from("agent:sms-42")));
    session.apply(finish("agent:sms-42", RunOutcome::Completed));
    session.apply(start("agent:sms-42"));
    assert!(session.active_run().is_none(), "Replay cannot reopen it");
}

/// RMK-1135 review: streamed text of the earlier run, no longer the last
/// started, was dropped.
#[test]
fn the_earlier_run_still_streams_while_a_later_one_runs() {
    let mut session = session();
    session.apply(start("turn-1"));
    session.apply(start("agent:sms-42"));
    session.apply(Event::TextDelta {
        conversation: "room-a".into(),
        message: "draft-1".into(),
        actor: "agent".into(),
        run: "turn-1".into(),
        text: "Still answering".into(),
    });
    let items = &session.snapshot().unwrap().items;
    assert!(
        items
            .iter()
            .any(|item| matches!(item, Item::Message(m) if m.text == "Still answering"))
    );
}

/// When the earliest run finishes while another runs on, the timer, Esc and
/// the snapshot follow that run from its own start, and the earliest's
/// failure is not hidden by the later run completing.
#[test]
fn the_next_run_takes_over_when_the_earliest_finishes() {
    let mut session = session();
    session.apply_at(start("deploy-1"), seconds(0));
    session.apply_at(start("agent:sms-42"), seconds(10));
    let sms = RunId::from("agent:sms-42");
    assert!(
        session
            .request(Command::Interrupt { run: sms.clone() })
            .is_ok()
    );
    session.apply_at(finish("deploy-1", RunOutcome::Failed), seconds(20));
    assert_eq!(session.active_run(), Some(&sms));
    assert_eq!(session.snapshot().unwrap().active_run.as_ref(), Some(&sms));
    assert_eq!(session.run_elapsed(seconds(25)), Some(seconds(15)));
    assert_eq!(session.observation().unwrap().run.unwrap().id, sms);
    session.apply_at(finish("agent:sms-42", RunOutcome::Completed), seconds(30));
    assert_eq!(session.snapshot().unwrap().active_run, None);
    assert_eq!(
        session.run_outcome(),
        Some(RunOutcome::Completed),
        "Its own"
    );
    let observed = session.observation().unwrap().run.unwrap();
    assert_eq!(
        (observed.id, observed.outcome),
        (sms, Some(RunOutcome::Completed))
    );
    assert_eq!(session.execution_outcome(), Some(RunOutcome::Failed));
}

/// The room's outcome counts every run that overlapped, whichever finished
/// first, and a snapshot or a new start in an idle view begins afresh.
#[test]
fn the_room_s_outcome_counts_the_runs_since_it_was_idle() {
    let mut session = session();
    session.apply(start("deploy-1"));
    session.apply(start("agent:sms-42"));
    session.apply(finish("agent:sms-42", RunOutcome::Failed));
    session.apply(finish("deploy-1", RunOutcome::Completed));
    assert_eq!(session.execution_outcome(), Some(RunOutcome::Failed));
    session.apply(start("deploy-2"));
    session.apply(finish("deploy-2", RunOutcome::Completed));
    assert_eq!(
        session.execution_outcome(),
        Some(RunOutcome::Completed),
        "Idle in between"
    );

    session.apply(start("deploy-3"));
    session.apply(start("agent:sms-43"));
    session.apply(finish("deploy-3", RunOutcome::Failed));
    let mut state = snapshot();
    state.active_run = Some("agent:sms-43".into());
    session.apply(Event::Snapshot(state));
    session.apply(finish("agent:sms-43", RunOutcome::Completed));
    assert_eq!(
        session.execution_outcome(),
        Some(RunOutcome::Completed),
        "A snapshot is afresh"
    );
}

/// Beyond the bound, the oldest run after the earliest is forgotten with
/// its requests; the view's own work and its interruption stay.
#[test]
fn the_bound_on_live_runs_keeps_the_earliest() {
    let mut session = session();
    session.apply(start("deploy-1"));
    let interrupt = session
        .request(Command::Interrupt {
            run: "deploy-1".into(),
        })
        .unwrap();
    session.sent(&interrupt);
    session.apply(start("other-0"));
    session.apply(ask("from-other-0", "other-0"));
    for n in 1..16 {
        session.apply(start(&format!("other-{n}")));
    }
    assert_eq!(session.active_run(), Some(&RunId::from("deploy-1")));
    assert!(session.interrupting(), "Still interrupting the work");
    assert!(session.permissions().is_empty(), "other-0 was forgotten");
    session.apply(finish("other-0", RunOutcome::Completed));
    assert_eq!(session.active_run(), Some(&RunId::from("deploy-1")));
}
