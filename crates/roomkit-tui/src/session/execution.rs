//! Execution status: operations, runs, timing and passive observation.

use std::time::Duration;

use roomkit_tui_protocol::{ContextUsage, RunOutcome};

use super::Session;
use crate::{RunObservation, SessionObservation};

impl Session {
    /// Read activity and display identity without exposing messages or tool arguments.
    pub fn observation(&self) -> Option<SessionObservation> {
        let snapshot = self.snapshot.as_ref()?;
        Some(SessionObservation {
            conversation: snapshot.conversation.clone(),
            title: snapshot.title.clone(),
            selected_agent: snapshot.selected_agent.clone(),
            mode: snapshot.mode.clone(),
            operation: self.operation.clone(),
            run: self.observed_run.as_ref().map(|run| RunObservation {
                id: run.id.clone(),
                outcome: run.outcome,
            }),
            permissions: snapshot
                .permissions
                .iter()
                .map(|permission| permission.id.clone())
                .collect(),
        })
    }

    /// Whether accepted work or a run is still awaiting host-confirmed completion.
    pub fn is_executing(&self) -> bool {
        self.active_run().is_some()
            || self
                .operation
                .as_ref()
                .is_some_and(|op| op.outcome.is_none())
    }

    /// Settled execution outcome, including failures before any run existed.
    /// Processing success never erases an individual run's failure.
    pub fn execution_outcome(&self) -> Option<RunOutcome> {
        if self.is_executing() {
            return None;
        }
        let operation = self.operation.as_ref().and_then(|op| op.outcome);
        let run = self.run_outcome();
        for severe in [
            RunOutcome::Failed,
            RunOutcome::Interrupted,
            RunOutcome::Unknown,
        ] {
            if [operation, run].contains(&Some(severe)) {
                return Some(severe);
            }
        }
        operation.or(run)
    }

    /// Elapsed time for the active run, or the frozen last observed duration.
    /// Unknown when a start or finish was not timed, including restored runs.
    pub fn run_elapsed(&self, now: Duration) -> Option<Duration> {
        let run = self.observed_run.as_ref()?;
        if run.outcome.is_some() {
            run.duration
        } else {
            now.checked_sub(run.started_at?)
        }
    }

    /// Last host-confirmed outcome, retained until a new run or snapshot.
    pub fn run_outcome(&self) -> Option<RunOutcome> {
        self.observed_run.as_ref().and_then(|run| run.outcome)
    }

    /// A current measurement, or the last known one explicitly marked stale.
    /// Previous context is presentation-only and never becomes current usage.
    pub(crate) fn displayed_context(&self) -> Option<(&ContextUsage, bool)> {
        self.context()
            .map(|usage| (usage, false))
            .or_else(|| self.previous_context.as_ref().map(|usage| (usage, true)))
    }
}
