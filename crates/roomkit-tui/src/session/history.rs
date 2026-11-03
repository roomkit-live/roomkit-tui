//! History: the snapshot, streamed text and items. Run boundaries are in
//! `runs`.

use roomkit_tui_protocol::{ActorId, ConversationId, Item, Message, MessageId, RunId, Snapshot};

use super::bounds::{bound_in_place, bound_item, bound_labels, bounded, trim_items};
use super::{ObservedRun, Session};

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
        self.snapshot = Some(snapshot);
        self.reset_view_state();
        self.refresh_menu();
    }

    /// A replacement snapshot invalidates every transient projection of the
    /// previous one; the draft and its history are kept.
    fn reset_view_state(&mut self) {
        self.operation = None;
        self.previous_context = None;
        self.permission_scroll = 0;
        self.permission_choice = 0;
        self.deciding.clear();
        self.interrupting = None;
        self.pending.clear();
        self.forget_waited_send();
        self.scroll = 0;
        self.approval_drawn = None;
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
        let limit = self.limits.text_bytes.get();
        let Some(snapshot) = &mut self.snapshot else {
            return;
        };
        // Blank so far, as what a model streams before a tool call, a draft
        // takes no room on screen: its text starts here, below what came since,
        // and a finished answer takes its draft's place.
        let is_draft =
            |item: &Item| matches!(item, Item::Message(m) if m.id == message && m.actor == actor);
        if let Some(at) = snapshot.items.iter().rposition(is_draft) {
            let blank = matches!(&snapshot.items[at], Item::Message(m) if m.text.trim().is_empty());
            if blank {
                let draft = snapshot.items.remove(at);
                snapshot.items.push(draft);
            }
        }
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
        snapshot.items.push(Item::Message(Message::new(
            message,
            actor,
            bounded(text, limit),
        )));
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
        self.on_item(conversation, item);
    }

    pub(super) fn on_item(&mut self, conversation: &ConversationId, mut item: Item) {
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
        snapshot.items.push(item);
        if trim_items(&mut snapshot.items, self.limits.items.get()) {
            self.forget_missing_tools();
        }
    }
}
