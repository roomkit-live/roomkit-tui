//! Messages handed to the running turn and not read yet (RMK-1317). A turn
//! reads them at its next stop: once a round of calls ended, or after an
//! answer. What it writes until then goes above them, as it was written
//! without them; what it writes after goes below.
//!
//! No event says when a turn reads. The stop shows in what comes next: a round
//! announces all its calls before any of them ends, so what comes once they
//! all ended, like the text that follows an answer, is the model's next answer.

use roomkit_tui_protocol::{Item, MessageId, RunId, Snapshot};

use super::Session;
use super::steering::{Handed, Steering};

/// What a turn holds and has not read.
#[derive(Debug, Default)]
pub(super) struct Unread {
    messages: Vec<Held>,
    /// An answer ended since the last call: text that comes now is the next one.
    answered: bool,
}

#[derive(Debug)]
struct Held {
    id: MessageId,
    /// Handed once the turn had stopped: the model's next answer is still
    /// written without it, and it is read at the stop after that one.
    after_next_answer: bool,
}

impl Unread {
    fn holds(&self, id: &MessageId) -> bool {
        self.messages.iter().any(|held| held.id == *id)
    }

    pub(super) fn forget(&mut self, id: &MessageId) {
        self.messages.retain(|held| held.id != *id);
    }

    /// The turn stopped: what `self` read then, which it holds no more.
    fn read(&mut self) -> Vec<MessageId> {
        self.answered = false;
        let (waiting, read) = std::mem::take(&mut self.messages)
            .into_iter()
            .partition(|held| held.after_next_answer);
        self.messages = waiting;
        for held in &mut self.messages {
            held.after_next_answer = false;
        }
        read.into_iter().map(|held: Held| held.id).collect()
    }
}

/// Where `item` goes among `snapshot`'s: what a person writes at the end, what
/// the turn writes above what it has not read.
pub(super) fn place(handed: Option<&Handed>, snapshot: &Snapshot, item: &Item) -> usize {
    let own = matches!(item, Item::Message(m) if snapshot.local_actor.as_ref() == Some(&m.actor));
    if own {
        return snapshot.items.len();
    }
    handed.map_or(snapshot.items.len(), |handed| {
        snapshot
            .items
            .iter()
            .position(|old| matches!(old, Item::Message(m) if handed.unread.holds(&m.id)))
            .unwrap_or(snapshot.items.len())
    })
}

impl Session {
    /// `id` was just handed to the turn: it goes below all the turn wrote.
    pub(super) fn hold_unread(&mut self, id: &MessageId) {
        let Some(run) = self.handed.as_ref().map(|handed| handed.run.clone()) else {
            return;
        };
        let stopped = self.calls_ended(&run, Some(id));
        if let Some(handed) = &mut self.handed {
            handed.unread.messages.push(Held {
                id: id.clone(),
                after_next_answer: stopped || handed.unread.answered,
            });
        }
        let Some(snapshot) = &mut self.snapshot else {
            return;
        };
        let at = snapshot
            .items
            .iter()
            .rposition(|old| matches!(old, Item::Message(m) if m.id == *id));
        if let Some(at) = at {
            let message = snapshot.items.remove(at);
            snapshot.items.push(message);
        }
    }

    /// `run` starts a call. After an answer it comes from that same answer,
    /// whose text ended as its calls start.
    pub(super) fn call_starts(&mut self, run: &RunId) {
        if !self.holds_unread(run) {
            return;
        }
        if self.calls_ended(run, None) {
            self.turn_stopped();
        }
        if let Some(handed) = &mut self.handed {
            handed.unread.answered = false;
        }
    }

    /// `run` writes in `draft`. Text that extends words is the same answer.
    pub(super) fn text_comes(&mut self, run: &RunId, draft: &MessageId) {
        if !self.holds_unread(run) {
            return;
        }
        let extends = self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.items.iter().rev().any(
                |item| matches!(item, Item::Message(m) if m.id == *draft && !m.text.trim().is_empty()),
            )
        });
        if extends {
            return;
        }
        let answered = self
            .handed
            .as_ref()
            .is_some_and(|handed| handed.unread.answered);
        if answered || self.calls_ended(run, Some(draft)) {
            self.turn_stopped();
        }
    }

    /// An answer ended, or text ended as its calls start.
    pub(super) fn answer_ended(&mut self) {
        if let Some(handed) = &mut self.handed
            && !handed.unread.messages.is_empty()
        {
            handed.unread.answered = true;
        }
    }

    /// `run` writes no more: nothing it did not read still waits for it.
    pub(super) fn forget_unread(&mut self, run: &RunId) {
        if let Some(handed) = &mut self.handed
            && handed.run == *run
        {
            handed.unread = Unread::default();
        }
    }

    /// A snapshot may say a turn ended that no end was told of.
    pub(super) fn forget_unread_of_ended_turn(&mut self) {
        let ended = self
            .handed
            .as_ref()
            .filter(|handed| !self.is_live(&handed.run))
            .map(|handed| handed.run.clone());
        if let Some(run) = ended {
            self.forget_unread(&run);
        }
    }

    fn holds_unread(&self, run: &RunId) -> bool {
        self.handed
            .as_ref()
            .is_some_and(|handed| handed.run == *run && !handed.unread.messages.is_empty())
    }

    /// Whether what `run` last wrote is a call, and all its calls ended. The
    /// message `other` names, and what people wrote meanwhile, do not count.
    fn calls_ended(&self, run: &RunId, other: Option<&MessageId>) -> bool {
        let (Some(snapshot), Some(handed)) = (&self.snapshot, &self.handed) else {
            return false;
        };
        let last = snapshot.items.iter().rev().find(|item| match item {
            Item::Message(m) => {
                Some(&m.id) != other
                    && snapshot.local_actor.as_ref() != Some(&m.actor)
                    && !handed.unread.holds(&m.id)
            }
            Item::Tool { run: of, .. } => of == run,
            _ => false,
        });
        matches!(last, Some(Item::Tool { .. }))
            && !snapshot.items.iter().any(|item| {
                matches!(item, Item::Tool { run: of, call, .. } if of == run && !call.state.is_settled())
            })
    }

    /// The turn passed a stop: what it held before is read.
    fn turn_stopped(&mut self) {
        let Some(handed) = &mut self.handed else {
            return;
        };
        for id in handed.unread.read() {
            let entry = self.steered.iter_mut().find(|(steered, _)| *steered == id);
            if let Some((_, steering)) = entry {
                *steering = Steering::Read;
            }
        }
    }
}
