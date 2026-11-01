//! Short host labels scoped to the displayed conversation, including its
//! title. Each is bounded on arrival; rendering removes terminal controls
//! from every label it draws.

use roomkit_tui_protocol::ConversationId;

use super::Session;
use super::bounds::fits_label as fits;

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
