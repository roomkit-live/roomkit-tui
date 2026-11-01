//! Short host labels scoped to the displayed conversation. Each is bounded on
//! arrival; rendering removes terminal controls from every label it draws.

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
}

/// A longer label is refused, not truncated, so the view keeps the last one.
fn fits(text: &str) -> bool {
    text.len() <= MAX_LABEL_BYTES
}
