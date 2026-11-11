//! RMK-1646: a host's estimate of the context, sent while no run measures it,
//! such as after compacting the conversation between turns.
#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]

use roomkit_tui_protocol::*;

use super::*;
use crate::Session;
use crate::render::{self, Renderers};

/// The latest measure before the compaction, as a snapshot restores it.
fn measured_before() -> Snapshot {
    let mut snapshot = snapshot();
    snapshot.context = Some(ContextUsage::new(651_571).with_capacity(1_000_000));
    snapshot
}

fn estimate(tokens: u64) -> Event {
    estimate_for("room-a", "model-a", tokens)
}

fn estimate_for(conversation: &str, model: &str, tokens: u64) -> Event {
    Event::ContextEstimated {
        conversation: conversation.into(),
        provider: "local".into(),
        model: model.into(),
        usage: ContextUsage::new(tokens).with_capacity(1_000_000),
    }
}

fn measured(run: &str, tokens: u64) -> Event {
    Event::ContextUpdated {
        conversation: "room-a".into(),
        run: run.into(),
        provider: "local".into(),
        model: "model-a".into(),
        usage: ContextUsage::new(tokens).with_capacity(1_000_000),
    }
}

fn finished(session: &mut Session, run: &str) {
    session.apply(Event::RunFinished {
        conversation: "room-a".into(),
        run: run.into(),
        outcome: RunOutcome::Completed,
    });
}

fn footer(session: &Session) -> String {
    render::screen(session, 230, 30, &Renderers::default())
        .lines
        .join("\n")
}

/// The gauge's tokens, and whether it reads them as estimated.
fn shown(session: &Session) -> Option<(u64, bool)> {
    session
        .context()
        .map(|usage| (usage.tokens, usage.estimated))
}

#[test]
fn an_estimate_at_rest_replaces_the_last_run_s_measure() {
    let mut session = session();
    started(&mut session, "run-1");
    session.apply(measured("run-1", 651_571));
    finished(&mut session, "run-1");

    session.apply(estimate(92_000));
    assert_eq!(shown(&session), Some((92_000, true)));
    let screen = footer(&session);
    assert!(screen.contains("context ~92000/1000000"), "{screen}");
    assert!(!screen.contains("last report"), "{screen}");
}

#[test]
fn an_estimate_replaces_the_measure_a_snapshot_restored() {
    let mut session = Session::default();
    session.apply(Event::Snapshot(measured_before()));
    session.apply(estimate(92_000));
    assert_eq!(shown(&session), Some((92_000, true)));

    // A reload shows the stored measure again, and the host's estimate,
    // sent again after it, is taken again.
    session.apply(Event::Snapshot(measured_before()));
    assert_eq!(shown(&session), Some((651_571, false)));
    session.apply(estimate(92_000));
    assert!(footer(&session).contains("context ~92000/1000000"));
}

#[test]
fn the_next_run_s_measure_replaces_the_estimate_and_a_resend_never_undoes_it() {
    let mut session = Session::default();
    session.apply(Event::Snapshot(measured_before()));
    session.apply(estimate(92_000));
    started(&mut session, "run-2");
    // The host's state, sent again while the run goes, is not news.
    session.apply(estimate(92_000));
    assert_eq!(shown(&session), None, "The run has measured nothing yet");
    session.apply(measured("run-2", 147_015));
    assert_eq!(shown(&session), Some((147_015, false)));
    session.apply(estimate(92_000));
    finished(&mut session, "run-2");
    // Nor once the run ended: the measure stays.
    session.apply(estimate(92_000));
    assert_eq!(shown(&session), Some((147_015, false)));
    assert!(footer(&session).contains("context 147015/1000000"));

    // Another compaction gives another estimate, which is taken.
    session.apply(estimate(80_000));
    assert_eq!(shown(&session), Some((80_000, true)));
}

#[test]
fn an_estimate_for_another_model_or_conversation_is_ignored() {
    let mut session = Session::default();
    session.apply(Event::Snapshot(measured_before()));
    session.apply(estimate_for("room-a", "model-b", 92_000));
    session.apply(estimate_for("room-b", "model-a", 92_000));
    assert_eq!(shown(&session), Some((651_571, false)));

    // Taken for model-a, the estimate is cleared by a change of model, and
    // taken again once the model is back.
    session.apply(estimate(92_000));
    for model in ["model-b", "model-a"] {
        session.apply(Event::ModelChanged {
            conversation: "room-a".into(),
            model: ModelInfo::new("local", model),
        });
    }
    assert_eq!(shown(&session), None);
    session.apply(estimate(92_000));
    assert_eq!(shown(&session), Some((92_000, true)));
}

#[test]
fn an_estimate_after_a_refused_request_is_no_refusal() {
    let mut session = Session::default();
    session.apply(Event::Snapshot(measured_before()));
    started(&mut session, "run-1");
    session.apply(Event::ContextExceeded {
        conversation: "room-a".into(),
        run: "run-1".into(),
    });
    finished(&mut session, "run-1");
    assert!(footer(&session).contains("context refused"));

    session.apply(estimate(92_000));
    let screen = footer(&session);
    assert!(screen.contains("context ~92000/1000000"), "{screen}");
    assert!(!screen.contains("refused"), "{screen}");
}
