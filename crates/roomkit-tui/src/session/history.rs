//! History and runs: the snapshot, streamed text, items and run boundaries.

use std::time::Duration;

use roomkit_tui_protocol::limits::MAX_LABEL_BYTES;
use roomkit_tui_protocol::{
    ActorId, ConversationId, Item, Message, MessageId, RunId, RunOutcome, Snapshot,
};

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
        if !self.is_current(conversation) || self.last_run.as_ref() != Some(run) {
            return;
        }
        let limit = self.limits.text_bytes.get();
        let Some(snapshot) = &mut self.snapshot else {
            return;
        };
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
        if let Some(snapshot) = &mut self.snapshot {
            snapshot.items.retain(
                |item| !matches!(item, Item::Message(m) if m.id == *draft && m.actor == message.actor),
            );
        }
        self.on_item(conversation, Item::Message(message));
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

    pub(super) fn on_run_started(
        &mut self,
        conversation: &ConversationId,
        run: RunId,
        at: Option<Duration>,
    ) {
        if !self.is_current(conversation)
            || self.last_run.as_ref() == Some(&run)
            || run.as_str().len() > MAX_LABEL_BYTES
        {
            return;
        }
        if self
            .operation
            .as_ref()
            .is_some_and(|op| op.outcome.is_some())
        {
            self.operation = None;
        }
        self.observed_run = Some(ObservedRun {
            id: run.clone(),
            started_at: at,
            outcome: None,
            duration: None,
        });
        self.last_run = Some(run.clone());
        if let Some(snapshot) = &mut self.snapshot {
            if let Some(previous) = snapshot.context.take() {
                self.previous_context = Some(previous);
            }
            snapshot.usage = None;
            snapshot.active_run = Some(run);
        }
        self.interrupting = None;
        self.notice.clear();
    }

    pub(super) fn on_run_finished(
        &mut self,
        conversation: &ConversationId,
        run: &RunId,
        outcome: RunOutcome,
        at: Option<Duration>,
    ) {
        if !self.is_current(conversation) || self.active_run() != Some(run) {
            return;
        }
        self.interrupting = None;
        if let Some(observed) = &mut self.observed_run {
            observed.outcome = Some(outcome);
            observed.duration = observed
                .started_at
                .zip(at)
                .and_then(|(start, finish)| finish.checked_sub(start));
        }
        self.retain_permissions(|permission| permission.run != *run);
        if let Some(snapshot) = &mut self.snapshot {
            snapshot.active_run = None;
        }
    }
}
