//! Send the draft now, cutting the turn under way (RMK-1285). A message
//! handed to a turn that is being cancelled would be taken and never read,
//! so the draft waits in the composer until the interrupted turn has
//! stopped, then leaves as any message does.

use roomkit_tui_protocol::{ImageUpload, RequestId, RunId};

use super::{Session, SessionError};

/// The draft kept for once the work under way stops, as it was when the
/// person asked: an edited draft, a command among them, never leaves unseen.
#[derive(Debug)]
pub(super) struct Waiting {
    text: String,
    images: Vec<ImageUpload>,
    /// The run last asked to stop for it: a run started since is cut too.
    cut: Option<RunId>,
}

impl Session {
    /// Whether the draft waits to be sent once the interrupted turn stops:
    /// the composer still holds it as it was when Ctrl+S was pressed.
    pub fn sends_after_interruption(&self) -> bool {
        self.waiting
            .as_ref()
            .is_some_and(|w| w.text == self.editor.text() && w.images == self.images)
    }

    /// Keep the draft for once the work under way stops.
    pub(crate) fn send_after_interruption(&mut self) -> Result<(), SessionError> {
        let snapshot = self.snapshot.as_ref().ok_or(SessionError::WaitingForHost)?;
        if self.editor.text().trim().is_empty() && self.images.is_empty() {
            return Err(SessionError::EmptyMessage);
        }
        if !snapshot.can_interrupt {
            return Err(SessionError::NotInterruptible);
        }
        self.waiting = Some(Waiting {
            text: self.editor.text().to_owned(),
            images: self.images.clone(),
            cut: None,
        });
        Ok(())
    }

    /// Send nothing after the interruption; the draft stays.
    pub(crate) fn keep_draft(&mut self) {
        self.waiting = None;
    }

    /// The run to stop for the waiting draft: the one under way, unless it
    /// was asked already or an interruption is on its way. A refused
    /// interruption is not asked again; the draft then waits for the end.
    pub(crate) fn run_to_cut(&mut self) -> Option<RunId> {
        if !self.sends_after_interruption() || self.interrupting.is_some() {
            return None;
        }
        let run = self.active_run().cloned()?;
        let waiting = self.waiting.as_mut()?;
        if waiting.cut.as_ref() == Some(&run) {
            return None;
        }
        waiting.cut = Some(run.clone());
        Some(run)
    }

    /// Whether the waited-for draft may leave now: nothing runs any more,
    /// and the composer still holds it. Clears the wait once nothing runs.
    pub(crate) fn take_ready_draft(&mut self) -> bool {
        if self.waiting.is_none() || self.is_executing() || self.interrupting.is_some() {
            return false;
        }
        let ready = self.sends_after_interruption();
        self.waiting = None;
        ready
    }

    /// The waited draft left as request `id`: should the host refuse it, as
    /// when it still holds the room, the draft comes back to an empty
    /// composer.
    pub(crate) fn sent_waited(&mut self, id: RequestId, draft: String) {
        self.waited_send = Some((id, draft));
    }

    /// Request `id` was answered: a refused waited draft comes back.
    pub(super) fn answered_waited(&mut self, id: RequestId, refused: bool) {
        if self
            .waited_send
            .as_ref()
            .is_none_or(|(sent, _)| *sent != id)
        {
            return;
        }
        let Some((_, draft)) = self.waited_send.take() else {
            return;
        };
        if refused && self.editor.text().is_empty() {
            self.editor.set(draft);
            self.refresh_menu();
        }
    }
}
