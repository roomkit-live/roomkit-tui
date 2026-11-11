//! Rebuild input recall from the selected person's persisted conversation messages.

use roomkit_tui_protocol::{Item, Snapshot};

use super::Session;

impl Session {
    pub(super) fn restore_input_history(&mut self, snapshot: &Snapshot) {
        if self.snapshot.as_ref().is_some_and(|old| {
            old.conversation == snapshot.conversation && old.local_actor == snapshot.local_actor
        }) {
            return;
        }
        let entries = snapshot.items.iter().filter_map(|item| match item {
            Item::Message(message) if Some(&message.actor) == snapshot.local_actor.as_ref() => {
                Some(message.text.clone())
            }
            _ => None,
        });
        self.editor.replace_history(entries);
    }
}
