//! Send the draft now, cutting whatever runs (RMK-1285, RMK-1303). A message
//! handed to a turn that is being cancelled would be taken and never read,
//! so the draft waits in the composer until nothing runs, then leaves as any
//! message does. The host may still hold the room with no run to show, as
//! for the own turn of a message sent earlier: a draft it refuses as busy
//! goes on waiting, and a run that starts meanwhile is cut too. A message
//! whose turn is cut stays in the conversation, and the next turn reads it.
//!
//! The draft stays in the composer until the host takes it: a refusal then
//! changes nothing on screen, and the person can still edit or drop it.
//!
//! With an empty composer, Ctrl+S acts for the message last handed to a turn
//! that still runs (RMK-1311), which may be far from its next step: it cuts
//! that turn, and asks the host for a turn that answers what the conversation
//! holds, the message included, as no new message would.

use std::time::Duration;

use roomkit_tui_protocol::{
    Command, ErrorCode, ImageUpload, MessageId, Rejection, RequestId, RunId,
};

use super::{Session, SessionError};

/// How long after a busy refusal the draft is tried again.
const RETRY_EVERY: Duration = Duration::from_millis(200);
/// How long a host that shows no run may keep refusing the draft as busy
/// before it stays in the composer: long enough for a turn's closing hooks.
const RETRY_FOR: Duration = Duration::from_secs(30);

/// The draft kept for once the work under way stops, as it was when the
/// person asked: an edited draft, a command among them, never leaves unseen.
#[derive(Debug)]
pub(super) struct Waiting {
    text: String,
    images: Vec<ImageUpload>,
    /// With an empty composer: the message handed to the turn being cut, which
    /// a turn of its own then answers.
    answers: Option<MessageId>,
    /// The run last asked to stop for it: a run started since is cut too.
    cut: Option<RunId>,
    /// Not sent before then: the host has just refused it as busy.
    not_before: Duration,
    /// When a host that keeps refusing it stops being asked: set by a busy
    /// refusal, and forgotten while a run shows why the host is busy.
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

    /// Keep the draft for once nothing runs. Pressed again for the same
    /// draft, nothing changes.
    pub(crate) fn send_after_interruption(&mut self) -> Result<(), SessionError> {
        let snapshot = self.snapshot.as_ref().ok_or(SessionError::WaitingForHost)?;
        if self.sends_after_interruption() {
            return Ok(());
        }
        let empty = self.editor.text().trim().is_empty() && self.images.is_empty();
        let answers = self.handed_to_a_live_turn().filter(|_| empty).cloned();
        if empty && answers.is_none() {
            return Err(SessionError::EmptyMessage);
        }
        if self.is_executing() && !snapshot.can_interrupt {
            return Err(SessionError::NotInterruptible);
        }
        self.waiting = Some(Waiting {
            text: self.editor.text().to_owned(),
            images: self.images.clone(),
            answers,
            cut: None,
            not_before: Duration::ZERO,
            give_up: None,
        });
        Ok(())
    }

    /// Send nothing after the interruption; the draft stays.
    pub(crate) fn keep_draft(&mut self) {
        self.waiting = None;
    }

    /// The host's answer to a waited draft is lost, as a new snapshot loses
    /// every pending request: it may have left, so it is not sent again.
    pub(super) fn forget_waited_send(&mut self) {
        if self.flight.take().is_some() {
            self.waiting = None;
        }
    }

    /// The run to stop for the waiting draft: the one under way, unless it
    /// was asked already or an interruption is on its way. A refused
    /// interruption is not asked again; the draft then waits for the end.
    pub(crate) fn run_to_cut(&mut self) -> Option<RunId> {
        let executing = self.is_executing();
        let waiting = self.waiting.as_mut()?;
        if executing {
            waiting.give_up = None;
        }
        // A run seen while the draft is on its way may be the one it started.
        let can_interrupt = self.snapshot.as_ref().is_some_and(|s| s.can_interrupt);
        if !self.sends_after_interruption()
            || self.interrupting.is_some()
            || self.flight.is_some()
            || !can_interrupt
        {
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

    /// Whether the waited-for draft may leave at `now`: nothing runs any
    /// more and no message awaits its reply. An edited draft ends the wait
    /// and stays in the composer.
    pub(crate) fn draft_may_leave(&mut self, now: Duration) -> bool {
        let Some(waiting) = self.waiting.as_ref() else {
            return false;
        };
        if self.is_executing()
            || self.interrupting.is_some()
            || self.message_pending()
            || self.flight.is_some()
            || now < waiting.not_before
        {
            return false;
        }
        if !self.holds(waiting) {
            self.waiting = None;
            return false;
        }
        true
    }

    /// What the wait sends: the draft, or the request that answers the message
    /// handed to the cut turn.
    pub(crate) fn waited_command(&self) -> Result<Command, SessionError> {
        if self.waiting.as_ref().is_none_or(|w| w.answers.is_none()) {
            return self.input_command();
        }
        let snapshot = self.snapshot.as_ref().ok_or(SessionError::WaitingForHost)?;
        Ok(Command::Answer {
            agent: snapshot.selected_agent.clone(),
        })
    }

    /// The waited draft left as request `id`.
    pub(crate) fn sent_waited(&mut self, id: RequestId) {
        self.flight = Some((id, self.editor.text().to_owned()));
    }

    /// Request `id` was answered at `at`. Accepted, the waited draft leaves
    /// the composer. Refused as busy, it waits again, until [`RETRY_FOR`]
    /// has passed. Whether the refusal has nothing to tell the person.
    pub(super) fn answered_waited(
        &mut self,
        id: RequestId,
        refusal: Option<&Rejection>,
        at: Option<Duration>,
    ) -> bool {
        let Some((_, text)) = self.flight.take_if(|(sent, _)| *sent == id) else {
            return false;
        };
        let Some(refusal) = refusal else {
            self.waited_draft_left(&text);
            return false;
        };
        // Dropped meanwhile, or replaced by an edited draft sent next: being
        // busy no longer matters, any other refusal still does.
        let Some(waiting) = self.waiting.as_mut().filter(|w| w.text == text) else {
            return refusal.code == ErrorCode::Busy;
        };
        let now = at.unwrap_or_default();
        let give_up = *waiting.give_up.get_or_insert(now.saturating_add(RETRY_FOR));
        if refusal.code != ErrorCode::Busy || now >= give_up {
            self.waiting = None;
            return false;
        }
        waiting.not_before = now.saturating_add(RETRY_EVERY);
        true
    }

    /// The host took the draft `text`: it leaves the composer, unless the
    /// person edited it meanwhile. Esc that came too late changes nothing: the
    /// message left and shows in the conversation, and a copy left behind
    /// would be sent twice. Images leave as any image message.
    fn waited_draft_left(&mut self, text: &str) {
        if self.waiting.as_ref().is_some_and(|w| w.text != text) {
            return;
        }
        if let Some(message) = self.waiting.take().and_then(|w| w.answers) {
            self.answered_by_a_turn_of_its_own(&message);
        }
        self.notice.clear();
        if self.editor.text() == text && self.images.is_empty() {
            self.editor.submitted();
            self.draft_epoch = self.draft_epoch.wrapping_add(1);
            self.scroll_to_bottom();
            self.refresh_menu();
        }
    }
}
