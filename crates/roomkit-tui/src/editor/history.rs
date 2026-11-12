//! The inputs a composer recalls: those submitted here, and a
//! conversation's earlier messages, with the unfinished draft kept aside.

use super::Editor;
use crate::pasted::Pasted;

/// A restored message stays literal; a locally typed input keeps command syntax.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HistoryInput {
    text: String,
    restored: bool,
    pasted: Pasted,
}

impl Editor {
    /// Replace another conversation's recall entries without losing the draft.
    pub fn replace_history(&mut self, entries: impl Iterator<Item = String>) {
        if self.history_position.is_some() {
            self.text.clone_from(&self.draft);
            self.cursor = self.draft_cursor;
            self.restored_input = self.draft_restored;
            self.pasted = std::mem::take(&mut self.draft_pasted);
        }
        self.history.clear();
        self.history_position = None;
        self.browsing_history = false;
        self.draft.clear();
        self.draft_cursor = 0;
        self.draft_restored = false;
        for text in entries {
            let entry = HistoryInput {
                text,
                restored: true,
                pasted: Pasted::default(),
            };
            if !entry.text.trim().is_empty() && self.history.last() != Some(&entry) {
                self.history.push(entry);
                if self.history.len() > 100 {
                    self.history.remove(0);
                }
            }
        }
    }

    /// Remember successfully submitted text; keep at most 100 input entries.
    pub fn submitted(&mut self) {
        let entry = HistoryInput {
            text: self.text.clone(),
            restored: self.restored_input,
            pasted: self.pasted.clone(),
        };
        if !entry.text.trim().is_empty() && self.history.last() != Some(&entry) {
            self.history.push(entry);
            if self.history.len() > 100 {
                self.history.remove(0);
            }
        }
        self.clear();
    }

    /// Navigate submitted inputs while preserving the unfinished draft.
    pub fn history_previous(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let position = match self.history_position {
            None => {
                self.draft = self.text.clone();
                self.draft_cursor = self.cursor;
                self.draft_restored = self.restored_input;
                self.draft_pasted = self.pasted.clone();
                self.history.len() - 1
            }
            Some(position) => position.saturating_sub(1),
        };
        self.history_position = Some(position);
        if let Some(entry) = self.history.get(position).cloned() {
            self.recall(entry);
        }
    }

    /// Navigate toward the unfinished draft.
    pub fn history_next(&mut self) {
        let Some(position) = self.history_position else {
            return;
        };
        if let Some(entry) = self.history.get(position + 1).cloned() {
            self.history_position = Some(position + 1);
            self.recall(entry);
        } else {
            self.history_position = None;
            self.set(self.draft.clone());
            self.cursor = self.draft_cursor;
            self.restored_input = self.draft_restored;
            self.pasted = self.draft_pasted.clone();
        }
    }

    fn recall(&mut self, entry: HistoryInput) {
        self.set(entry.text);
        self.restored_input = entry.restored;
        self.pasted = entry.pasted;
        self.browsing_history = true;
    }

    /// Whether this draft is a restored message rather than a command input.
    pub fn is_restored_input(&self) -> bool {
        self.restored_input
    }

    /// Recalled entries use whole-input navigation until explicitly edited.
    pub fn is_browsing_history(&self) -> bool {
        self.browsing_history
    }

    /// Resume ordinary editing without losing the saved draft or history position.
    pub fn edit_history(&mut self) {
        self.browsing_history = false;
    }

    /// Move within a draft's lines; at the edge, or while browsing, recall inputs.
    pub fn history_or_vertical(&mut self, backwards: bool) {
        let another_line = if backwards {
            self.text[..self.cursor].contains('\n')
        } else {
            self.text[self.cursor..].contains('\n')
        };
        if !self.browsing_history && another_line {
            self.vertical(backwards);
        } else if backwards {
            self.history_previous();
        } else {
            self.history_next();
        }
    }
}
