//! Messages sent while the agent's turn ran (RMK-1161): handed to that turn,
//! or, when the turn no longer read them, answered by a turn of their own.

use roomkit_tui_protocol::limits::MAX_LABEL_BYTES;
use roomkit_tui_protocol::{ConversationId, MessageId, RunId};

use super::Session;
use super::unread::Unread;

/// Steered messages one view remembers. Beyond it the oldest is forgotten: a
/// host that steers without end must not grow the view.
const MAX_STEERED: usize = 64;

/// The messages handed to the turn under way (RMK-1311), oldest first.
#[derive(Debug)]
pub(super) struct Handed {
    pub(super) run: RunId,
    messages: Vec<MessageId>,
    pub(super) unread: Unread,
}

/// Where a message sent during a turn went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Steering {
    /// Handed to the turn under way, which reads it at its next step.
    Handed,
    /// The turn under way passed a step since: what it wrote from there on
    /// was written with this message.
    Read,
    /// That turn no longer read it: a turn of its own answers it.
    OwnTurn,
}

impl Session {
    /// Where `message` went, when it was sent while a turn ran.
    pub fn steering(&self, message: &MessageId) -> Option<Steering> {
        self.steered
            .iter()
            .find(|(id, _)| id == message)
            .map(|(_, steering)| *steering)
    }

    /// The message last handed to a turn that still runs and can be cut: Ctrl+S
    /// on an empty composer cuts that turn to answer what it was handed, on a
    /// host that can. A message the turn read counts: the model may go on
    /// without answering it.
    pub(crate) fn handed_to_a_live_turn(&self) -> Option<&MessageId> {
        let handed = self.handed.as_ref()?;
        let can_cut = self
            .snapshot
            .as_ref()
            .is_some_and(|s| s.can_interrupt && s.can_answer);
        if !can_cut || !self.is_live(&handed.run) {
            return None;
        }
        handed.messages.last()
    }

    /// Whether a message may be sent while the agent's turn runs: the host
    /// hands it to that turn.
    pub(super) fn can_steer(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|s| s.can_steer)
    }

    pub(super) fn on_message_steered(
        &mut self,
        conversation: &ConversationId,
        message: MessageId,
        run: &RunId,
    ) {
        // A replay never undoes the turn of its own that answers it.
        if !self.is_current(conversation)
            || message.as_str().len() > MAX_LABEL_BYTES
            || run.as_str().len() > MAX_LABEL_BYTES
            || self.steering(&message).is_some()
        {
            return;
        }
        // Its own turn may have started before the host said where it went.
        let steering = if self.answered.contains(&message) {
            Steering::OwnTurn
        } else {
            Steering::Handed
        };
        // Its turn may have ended before the host said where it went: nothing
        // then waits for it, and the turn under way keeps what it was handed.
        if steering == Steering::Handed && self.is_live(run) {
            let handed = self
                .handed
                .take()
                .filter(|handed| handed.run == *run)
                .unwrap_or_else(|| Handed {
                    run: run.clone(),
                    messages: Vec::new(),
                    unread: Unread::default(),
                });
            self.handed = Some(handed);
            if let Some(handed) = &mut self.handed {
                remember(&mut handed.messages, message.clone());
            }
            self.hold_unread(&message);
        }
        remember(&mut self.steered, (message, steering));
    }

    /// A run started for `message`: if it was handed to a turn, that turn
    /// did not read it.
    pub(super) fn on_answered(&mut self, conversation: &ConversationId, message: &MessageId) {
        if !self.is_current(conversation) || message.as_str().len() > MAX_LABEL_BYTES {
            return;
        }
        self.own_turn_for_handed(message);
        match self.steered.iter_mut().find(|(id, _)| id == message) {
            Some((_, steering)) => *steering = Steering::OwnTurn,
            None if !self.answered.contains(message) => {
                remember(&mut self.answered, message.clone());
            }
            None => {}
        }
    }

    /// Whether messages were handed to `run`.
    pub(super) fn was_handed_to(&self, run: &RunId) -> bool {
        self.handed
            .as_ref()
            .is_some_and(|handed| handed.run == *run)
    }

    /// A run started for `message`: handed to a turn that no longer read it, it
    /// gets the answer a wait was to ask for, which no longer cuts anything.
    fn own_turn_for_handed(&mut self, message: &MessageId) {
        let Some(handed) = &mut self.handed else {
            return;
        };
        handed.unread.forget(message);
        let before = handed.messages.len();
        handed.messages.retain(|id| id != message);
        if handed.messages.len() != before {
            self.keep_answer_waiting_out();
        }
    }

    /// A turn now answers what the cut turn had not read, and the message the
    /// cut was for. What a stop read stays read.
    pub(super) fn handed_answered_by_their_own_turn(&mut self) {
        let Some(handed) = self.handed.take() else {
            return;
        };
        let target = handed.messages.last();
        for (id, steering) in &mut self.steered {
            let unread = *steering == Steering::Handed && handed.messages.contains(id);
            if unread || target == Some(id) {
                *steering = Steering::OwnTurn;
            }
        }
    }

    /// Forget where messages went, as a conversation switch does.
    pub(super) fn forget_steering(&mut self) {
        self.handed = None;
        self.steered.clear();
        self.answered.clear();
    }
}

/// Keep `entry`, forgetting the oldest beyond the bound.
pub(super) fn remember<T>(entries: &mut Vec<T>, entry: T) {
    if entries.len() >= MAX_STEERED {
        entries.remove(0);
    }
    entries.push(entry);
}
