//! Short host labels scoped to the displayed conversation, including its
//! title. Each is bounded on arrival; rendering removes terminal controls
//! from every label it draws.

use roomkit_tui_protocol::ConversationId;
use roomkit_tui_protocol::limits::MAX_LABEL_BYTES;

use super::Session;

impl Session {
    pub(super) fn on_application_info(
        &mut self,
        conversation: &ConversationId,
        text: Option<String>,
    ) {
        if self.is_current(conversation) && text.as_deref().is_none_or(fits) {
            self.application_info = text;
        }
    }

    pub(super) fn on_exit_warning(&mut self, conversation: &ConversationId, text: Option<String>) {
        if self.is_current(conversation) && text.as_deref().is_none_or(fits) {
            self.exit_warning = text;
        }
    }

    /// Rename the view in place; the history, draft and run stay as they are.
    pub(super) fn on_title_changed(&mut self, conversation: &ConversationId, title: String) {
        if fits(&title)
            && self.is_current(conversation)
            && let Some(snapshot) = &mut self.snapshot
        {
            snapshot.title = title;
        }
    }
}

/// A longer label is refused, not truncated, so the view keeps the last one.
fn fits(text: &str) -> bool {
    text.len() <= MAX_LABEL_BYTES
}
