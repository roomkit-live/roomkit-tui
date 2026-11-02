//! Messages sent while the agent's turn ran (RMK-1161): handed to that turn,
//! or, when the turn no longer read them, answered by a turn of their own.

use roomkit_tui_protocol::limits::MAX_LABEL_BYTES;
use roomkit_tui_protocol::{ConversationId, MessageId, RunId};

use super::Session;

/// Steered messages one view remembers. Beyond it the oldest is forgotten: a
/// host that steers without end must not grow the view.
const MAX_STEERED: usize = 64;

/// Where a message sent during a turn went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Steering {
    /// Handed to the turn under way, which reads it at its next step.
    Handed,
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
        remember(&mut self.steered, (message, steering));
    }

    /// A run started for `message`: if it was handed to a turn, that turn
    /// did not read it.
    pub(super) fn on_answered(&mut self, conversation: &ConversationId, message: &MessageId) {
        if !self.is_current(conversation) || message.as_str().len() > MAX_LABEL_BYTES {
            return;
        }
        match self.steered.iter_mut().find(|(id, _)| id == message) {
            Some((_, steering)) => *steering = Steering::OwnTurn,
            None if !self.answered.contains(message) => {
                remember(&mut self.answered, message.clone());
            }
            None => {}
        }
    }

    /// Forget where messages went, as a conversation switch does.
    pub(super) fn forget_steering(&mut self) {
        self.steered.clear();
        self.answered.clear();
    }
}

/// Keep `entry`, forgetting the oldest beyond the bound.
fn remember<T>(entries: &mut Vec<T>, entry: T) {
    if entries.len() >= MAX_STEERED {
        entries.remove(0);
    }
    entries.push(entry);
}
