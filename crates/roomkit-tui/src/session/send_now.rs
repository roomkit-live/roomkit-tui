//! Send the draft now, cutting whatever runs (RMK-1285, RMK-1303). A message
//! handed to a turn that is being cancelled would be taken and never read,
//! so the draft waits in the composer until nothing runs, then leaves as any
//! message does. The host may still hold the room with no run to show, as
//! for the own turn of a message sent earlier: a draft it refuses as busy
//! goes on waiting, and a run that starts meanwhile is cut too. A message
//! whose turn is cut stays in the conversation, and the next turn reads it.

use std::time::Duration;

use roomkit_tui_protocol::{ErrorCode, ImageUpload, Rejection, RequestId, RunId};

use super::{Session, SessionError};

/// How long after a busy refusal the draft is tried again.
const RETRY_EVERY: Duration = Duration::from_millis(200);
/// How long a host may keep refusing a draft as busy before it stays in the
/// composer: long enough for a turn's closing hooks.
const RETRY_FOR: Duration = Duration::from_secs(30);

/// The draft kept for once the work under way stops, as it was when the
/// person asked: an edited draft, a command among them, never leaves unseen.
#[derive(Debug)]
pub(crate) struct Waiting {
    text: String,
    images: Vec<ImageUpload>,
    /// The run last asked to stop for it: a run started since is cut too.
    cut: Option<RunId>,
    /// Not sent before then: the host has just refused it as busy.
    not_before: Duration,
    /// When a host that keeps refusing it stops being asked; set by the
    /// first busy refusal.
    give_up: Option<Duration>,
}

impl Session {
    /// Whether the draft waits to be sent once nothing runs any more: the
    /// composer still holds it as it was when Ctrl+S was pressed.
    pub fn sends_after_interruption(&self) -> bool {
        self.waiting.as_ref().is_some_and(|w| self.holds(w))
    }

    fn holds(&self, waiting: &Waiting) -> bool {
        waiting.text == self.editor.text() && waiting.images == self.images
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
            not_before: Duration::ZERO,
            give_up: None,
        });
        Ok(())
    }

    /// Send nothing after the interruption; the draft stays. A draft on its
    /// way no longer comes back either.
    pub(crate) fn keep_draft(&mut self) {
        self.waiting = None;
        self.waited_send = None;
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

    /// The waited-for draft, once it may leave at `now`: nothing runs any
    /// more and no message awaits its reply. An edited draft ends the wait
    /// and stays in the composer.
    pub(crate) fn take_ready_draft(&mut self, now: Duration) -> Option<Waiting> {
        let waiting = self.waiting.as_ref()?;
        if self.is_executing()
            || self.interrupting.is_some()
            || self.message_pending()
            || now < waiting.not_before
        {
            return None;
        }
        let unedited = self.holds(waiting);
        let waiting = self.waiting.take()?;
        unedited.then_some(waiting)
    }

    /// The waited draft left as request `id`.
    pub(crate) fn sent_waited(&mut self, id: RequestId, waiting: Waiting) {
        self.waited_send = Some((id, waiting));
    }

    /// Request `id` was answered at `at`. A refused waited draft comes back
    /// to an empty composer, and a busy refusal keeps it waiting, until
    /// [`RETRY_FOR`] has passed. Whether it waits again.
    pub(super) fn answered_waited(
        &mut self,
        id: RequestId,
        refusal: Option<&Rejection>,
        at: Option<Duration>,
    ) -> bool {
        if self
            .waited_send
            .as_ref()
            .is_none_or(|(sent, _)| *sent != id)
        {
            return false;
        }
        let Some((_, mut waiting)) = self.waited_send.take() else {
            return false;
        };
        let Some(refusal) = refusal else {
            return false;
        };
        if self.editor.text().is_empty() {
            self.editor.set(waiting.text.clone());
            self.refresh_menu();
        }
        let now = at.unwrap_or_default();
        let give_up = *waiting.give_up.get_or_insert(now.saturating_add(RETRY_FOR));
        if refusal.code != ErrorCode::Busy || now >= give_up || !self.holds(&waiting) {
            return false;
        }
        waiting.not_before = now.saturating_add(RETRY_EVERY);
        waiting.cut = None;
        self.waiting = Some(waiting);
        true
    }
}
