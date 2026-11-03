//! Runs a room has started and not finished. Several can be live at once:
//! host work a person started, and an agent turn that another channel's
//! message started meanwhile. The earliest drives the timer and Esc: a view
//! starts work only in an idle room, so its own work is the earliest. Every
//! live run may ask for a decision.

use std::time::Duration;

use roomkit_tui_protocol::limits::MAX_LABEL_BYTES;
use roomkit_tui_protocol::{ConversationId, RunId, RunOutcome};

use super::{ObservedRun, Session};

/// Live runs one view follows. Beyond it the oldest run after the earliest
/// is forgotten: a host that never finishes its runs must not grow the view,
/// and the earliest is the view's own work.
const MAX_LIVE_RUNS: usize = 16;

/// A run started and not finished yet, and when this view saw it start.
#[derive(Debug)]
pub(super) struct LiveRun {
    id: RunId,
    started_at: Option<Duration>,
}

/// The more severe of two outcomes.
fn worse(a: Option<RunOutcome>, b: RunOutcome) -> RunOutcome {
    let rank = |outcome: RunOutcome| match outcome {
        RunOutcome::Failed => 3,
        RunOutcome::Interrupted => 2,
        RunOutcome::Completed => 0,
        _ => 1,
    };
    a.filter(|a| rank(*a) > rank(b)).unwrap_or(b)
}

impl Session {
    /// Active execution: the earliest run started and not finished.
    pub fn active_run(&self) -> Option<&RunId> {
        self.live_runs.first().map(|run| &run.id)
    }

    /// Whether `run` has started and not finished.
    pub(super) fn is_live(&self, run: &RunId) -> bool {
        self.live_runs.iter().any(|live| &live.id == run)
    }

    /// Follow the live runs a snapshot states, keeping the timing of a run
    /// this view already saw start.
    pub(super) fn restore_runs(&mut self, active: Option<&RunId>) {
        let started_at = |id: &RunId| {
            self.observed_run
                .as_ref()
                .filter(|run| &run.id == id)
                .and_then(|run| run.started_at)
        };
        self.held_outcome = None;
        self.live_runs = active
            .map(|id| LiveRun {
                id: id.clone(),
                started_at: started_at(id),
            })
            .into_iter()
            .collect();
    }

    pub(super) fn on_run_started(
        &mut self,
        conversation: &ConversationId,
        run: RunId,
        at: Option<Duration>,
    ) {
        // A replayed start never reopens the last run or doubles a live one.
        if !self.is_current(conversation)
            || self.last_run.as_ref() == Some(&run)
            || self.is_live(&run)
            || run.as_str().len() > MAX_LABEL_BYTES
        {
            return;
        }
        if self
            .operation
            .as_ref()
            .is_some_and(|op| op.outcome.is_some())
        {
            self.operation = None;
        }
        self.last_run = Some(run.clone());
        self.measure_latest_run();
        let earliest = self.live_runs.first().map(|live| live.id.clone());
        if self.live_runs.len() >= MAX_LIVE_RUNS {
            let forgotten = self.live_runs.remove(1).id;
            self.forget(&forgotten);
        }
        self.live_runs.push(LiveRun {
            id: run,
            started_at: at,
        });
        if earliest.is_none() {
            self.interrupting = None;
            self.held_outcome = None;
            self.notice.clear();
        }
        if self.active_run() != earliest.as_ref() {
            self.follow_earliest();
        }
        self.sync_active_run();
    }

    pub(super) fn on_run_finished(
        &mut self,
        conversation: &ConversationId,
        run: &RunId,
        outcome: RunOutcome,
        at: Option<Duration>,
    ) {
        if !self.is_current(conversation) {
            return;
        }
        let Some(index) = self.live_runs.iter().position(|live| &live.id == run) else {
            return;
        };
        self.live_runs.remove(index);
        self.forget(run);
        if outcome == RunOutcome::Completed {
            self.keep_answer_waiting_out();
        }
        if !self.live_runs.is_empty() {
            // Each run keeps its own outcome; `execution_outcome` reports the
            // most severe of the runs since the room was idle.
            self.held_outcome = Some(worse(self.held_outcome, outcome));
            if index == 0 {
                self.follow_earliest();
            }
            self.sync_active_run();
            return;
        }
        if index == 0 {
            // The outcome and duration stay shown until the next run.
            if let Some(observed) = &mut self.observed_run {
                observed.outcome = Some(outcome);
                observed.duration = observed
                    .started_at
                    .zip(at)
                    .and_then(|(start, finish)| finish.checked_sub(start));
            }
        }
        self.sync_active_run();
    }

    /// The most severe outcome of the runs that finished while another ran
    /// on, since the room was last idle.
    pub(super) fn held_outcome(&self) -> Option<RunOutcome> {
        self.held_outcome
    }

    /// Stop showing what belonged to `run`, which ended or was forgotten.
    fn forget(&mut self, run: &RunId) {
        if self.interrupting.as_ref() == Some(run) {
            self.interrupting = None;
        }
        self.retain_permissions(|permission| permission.run != *run);
        self.sync_active_run();
    }

    /// Usage and context describe the latest run.
    fn measure_latest_run(&mut self) {
        if let Some(snapshot) = &mut self.snapshot {
            if let Some(previous) = snapshot.context.take() {
                self.previous_context = Some(previous);
            }
            snapshot.usage = None;
        }
    }

    /// Time the earliest live run, the one the timer and Esc follow.
    fn follow_earliest(&mut self) {
        self.observed_run = self.live_runs.first().map(|live| ObservedRun {
            id: live.id.clone(),
            started_at: live.started_at,
            outcome: None,
            duration: None,
        });
    }

    /// Keep the snapshot's active run, which readers of the snapshot see, on
    /// the earliest live run.
    fn sync_active_run(&mut self) {
        let active = self.active_run().cloned();
        if let Some(snapshot) = &mut self.snapshot {
            snapshot.active_run = active;
        }
    }
}
