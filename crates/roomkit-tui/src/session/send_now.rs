//! Send the draft now, cutting the turn under way (RMK-1285). A message
//! handed to a turn that is being cancelled would be taken and never read,
//! so the draft waits in the composer until the interrupted turn has
//! stopped, then leaves as any message does.

use roomkit_tui_protocol::RunId;

use super::{Session, SessionError};

impl Session {
    /// Whether the draft waits to be sent once the interrupted turn stops.
    pub fn sends_after_interruption(&self) -> bool {
        self.send_after_interruption
    }

    /// Keep the draft for once the work under way stops, and say which run
    /// to interrupt for it: none when an interruption is already on its
    /// way, or when no run has started yet.
    pub(crate) fn send_after_interruption(&mut self) -> Result<Option<RunId>, SessionError> {
        let snapshot = self.snapshot.as_ref().ok_or(SessionError::WaitingForHost)?;
        if self.editor.text().trim().is_empty() && self.images.is_empty() {
            return Err(SessionError::EmptyMessage);
        }
        let run = self.active_run().cloned();
        if run.is_some() && !snapshot.can_interrupt {
            return Err(SessionError::NotInterruptible);
        }
        self.send_after_interruption = true;
        Ok(run.filter(|_| self.interrupting.is_none()))
    }

    /// Send nothing after the interruption; the draft stays.
    pub(crate) fn keep_draft(&mut self) {
        self.send_after_interruption = false;
    }

    /// Whether the waited-for draft may leave now: nothing runs any more
    /// and the composer still holds it. Clears the wait either way once
    /// nothing runs.
    pub(crate) fn take_ready_draft(&mut self) -> bool {
        if !self.send_after_interruption || self.is_executing() || self.interrupting.is_some() {
            return false;
        }
        self.send_after_interruption = false;
        !self.editor.text().trim().is_empty() || !self.images.is_empty()
    }
}
