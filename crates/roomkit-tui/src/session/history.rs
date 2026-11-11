//! History: the snapshot, streamed text and items. Run boundaries are in
//! `runs`.

use roomkit_tui_protocol::{ActorId, ConversationId, Item, Message, MessageId, RunId, Snapshot};

use super::bounds::{bound_in_place, bound_item, bound_labels, bounded, trim_items};
use super::steering::Handed;
use super::unread::{is_output_of, place};
use super::{ObservedRun, Session};

/// Blank so far, as what a model streams before a tool call, a draft takes no
/// room on screen: its text starts where the turn's output ends, below what
/// came since, and a finished answer takes its draft's place.
fn blank_draft_to_the_end(
    items: &mut Vec<Item>,
    handed: Option<&Handed>,
    message: &MessageId,
    actor: &ActorId,
) {
    let blank = items.iter().rposition(|item| {
        matches!(item, Item::Message(m) if m.id == *message && m.actor == *actor && m.text.trim().is_empty())
    });
    if let Some(at) = blank {
        let draft = items.remove(at);
        items.insert(place(handed, items), draft);
    }
}

impl Session {
    pub(super) fn on_snapshot(&mut self, mut snapshot: Snapshot) {
        if let Err(error) = snapshot.validate() {
            self.notice = error.to_string();
            return;
        }
        if self
            .snapshot
            .as_ref()
            .is_some_and(|old| old.conversation != snapshot.conversation)
        {
            self.images.clear();
            self.forget_steering();
            self.keep_draft();
            self.draft_epoch = self.draft_epoch.wrapping_add(1);
        }
        bound_labels(&mut snapshot);
        snapshot.items.retain(|item| !matches!(item, Item::Unknown));
        for item in &mut snapshot.items {
            bound_item(item, self.limits.text_bytes.get());
        }
        trim_items(&mut snapshot.items, self.limits.items.get());
        let preserve_timing = self.is_current(&snapshot.conversation)
            && self.observed_run.as_ref().is_some_and(|run| {
                snapshot.active_run.as_ref() == Some(&run.id) && run.outcome.is_none()
            });
        if !preserve_timing {
            self.observed_run = snapshot.active_run.clone().map(|id| ObservedRun {
                id,
                started_at: None,
                outcome: None,
                duration: None,
            });
        }
        self.last_run = snapshot.active_run.clone();
        self.restore_runs(snapshot.active_run.as_ref());
        self.forget_unread_of_ended_turn();
        self.snapshot = Some(snapshot);
        self.reset_view_state();
        self.refresh_menu();
    }

    /// A replacement snapshot invalidates every transient projection of the
    /// previous one; the draft and its history are kept.
    fn reset_view_state(&mut self) {
        self.operation = None;
        self.forget_thoughts();
        self.previous_context = None;
        self.context_exceeded = false;
        self.known_estimate = None;
        self.held_estimate = None;
        self.permission_scroll = 0;
        self.permission_choice = 0;
        self.deciding.clear();
        self.interrupting = None;
        self.pending.clear();
        self.forget_waited_send();
        self.scroll = 0;
        self.approval_drawn = None;
        // The host tells the questions still asked again after a snapshot.
        self.questions.clear();
        self.question_drawn = None;
        self.expanded_tools.clear();
        self.focused_tool = None;
        self.picker = None;
        self.form = None;
        self.application_info = None;
        self.background = None;
        self.exit_warning = None;
        self.sidebar = None;
        self.panel = None;
        self.notice.clear();
        self.quit_until = None;
    }

    pub(super) fn on_text_delta(
        &mut self,
        conversation: &ConversationId,
        message: MessageId,
        actor: ActorId,
        run: &RunId,
        text: &str,
    ) {
        if !self.is_current(conversation)
            || (self.last_run.as_ref() != Some(run) && !self.is_live(run))
        {
            return;
        }
        self.text_comes(run);
        let limit = self.limits.text_bytes.get();
        let Some(snapshot) = &mut self.snapshot else {
            return;
        };
        let handed = self.handed.as_ref().filter(|handed| handed.run == *run);
        blank_draft_to_the_end(&mut snapshot.items, handed, &message, &actor);
        // The streamed message is almost always the newest item.
        let existing = snapshot.items.iter_mut().rev().find_map(|item| match item {
            Item::Message(m) if m.id == message && m.actor == actor => Some(m),
            _ => None,
        });
        if let Some(existing) = existing {
            if existing.text.len() < limit {
                existing.text.push_str(text);
                bound_in_place(&mut existing.text, limit);
            }
            return;
        }
        let draft = Item::Message(Message::new(message, actor, bounded(text, limit)));
        snapshot.items.insert(place(handed, &snapshot.items), draft);
        if trim_items(&mut snapshot.items, self.limits.items.get()) {
            self.forget_missing_tools();
        }
    }

    pub(super) fn on_message_finalized(
        &mut self,
        conversation: &ConversationId,
        draft: &MessageId,
        message: Message,
    ) {
        if !self.is_current(conversation) {
            return;
        }
        self.answer_ended();
        let limit = self.limits.text_bytes.get();
        let mut item = Item::Message(message);
        if let Some(snapshot) = &mut self.snapshot {
            let is_draft = |old: &Item| {
                matches!((old, &item), (Item::Message(o), Item::Message(m))
                    if o.id == *draft && o.actor == m.actor)
            };
            let at = snapshot.items.iter().rposition(is_draft);
            let known = snapshot
                .items
                .iter()
                .enumerate()
                .any(|(index, old)| Some(index) != at && old.same_identity(&item));
            match at {
                // In its draft's place: a message written during the answer
                // already follows it.
                Some(at) if !known => {
                    bound_item(&mut item, limit);
                    snapshot.items[at] = item;
                    return;
                }
                Some(at) => {
                    snapshot.items.remove(at);
                }
                None => {}
            }
        }
        // No draft was seen of it: an answer all the same.
        self.add_item(conversation, item, true);
    }

    pub(super) fn on_item(&mut self, conversation: &ConversationId, item: Item) {
        let of_turn = self
            .handed
            .as_ref()
            .is_some_and(|handed| is_output_of(handed, &item));
        self.add_item(conversation, item, of_turn);
    }

    /// Add `item`, or update the one it replaces, above what the turn has not
    /// read if it is `of_turn`.
    fn add_item(&mut self, conversation: &ConversationId, mut item: Item, of_turn: bool) {
        if !self.is_current(conversation) || matches!(item, Item::Unknown) {
            return;
        }
        bound_item(&mut item, self.limits.text_bytes.get());
        let Some(snapshot) = &mut self.snapshot else {
            return;
        };
        if let Some(existing) = snapshot
            .items
            .iter_mut()
            .rev()
            .find(|old| old.same_identity(&item))
        {
            *existing = item;
            return;
        }
        if let Item::Tool { run, .. } = &item {
            self.call_starts(run);
        }
        let Some(snapshot) = &mut self.snapshot else {
            return;
        };
        let handed = self.handed.as_ref().filter(|_| of_turn);
        snapshot.items.insert(place(handed, &snapshot.items), item);
        if trim_items(&mut snapshot.items, self.limits.items.get()) {
            self.forget_missing_tools();
        }
    }
}
