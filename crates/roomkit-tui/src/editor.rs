use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// A Unicode grapheme-aware composer, independent of terminal input events.
#[derive(Clone, Debug, Default)]
pub(crate) struct Editor {
    text: String,
    cursor: usize,
    history: Vec<HistoryInput>,
    restored_input: bool,
    history_position: Option<usize>,
    browsing_history: bool,
    draft: String,
    draft_cursor: usize,
    draft_restored: bool,
}

/// A restored message stays literal; a locally typed input keeps command syntax.
#[derive(Clone, Debug, PartialEq, Eq)]
struct HistoryInput {
    text: String,
    restored: bool,
}

impl Editor {
    /// Current text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// UTF-8 byte offset at a valid grapheme boundary.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Replace the draft and put the cursor at its end.
    pub fn set(&mut self, text: String) {
        self.edit_history();
        self.restored_input = false;
        self.text = text;
        self.cursor = self.text.len();
    }

    /// Discard the draft and leave history navigation without submitting it.
    pub(crate) fn clear(&mut self) {
        self.set(String::new());
        self.history_position = None;
        self.draft.clear();
        self.draft_cursor = 0;
        self.draft_restored = false;
    }

    /// Insert text at the cursor. Control characters other than newlines and
    /// tabs are removed; pasting never submits. Inputs normalize CRLF to LF.
    pub fn insert(&mut self, text: &str) {
        self.edit_history();
        if self.text.is_empty() {
            self.restored_input = false;
        }
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let clean: String = normalized
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .collect();
        self.text.insert_str(self.cursor, &clean);
        let end = self.cursor + clean.len();
        // Inserting a joiner can merge surrounding graphemes. Advance to the
        // resulting cluster boundary before accepting another editing command.
        self.cursor = end;
        self.align_cursor();
    }

    /// Move one grapheme left.
    pub fn left(&mut self) {
        self.edit_history();
        self.cursor = self
            .text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .take_while(|i| *i < self.cursor)
            .last()
            .unwrap_or(0);
    }

    /// Move one grapheme right.
    pub fn right(&mut self) {
        self.edit_history();
        self.cursor = self
            .text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .find(|i| *i > self.cursor)
            .unwrap_or(self.text.len());
    }

    /// Move to the start of the current line.
    pub fn home(&mut self) {
        self.edit_history();
        self.cursor = self
            .text
            .get(..self.cursor)
            .and_then(|s| s.rfind('\n'))
            .map_or(0, |i| i + 1);
    }

    /// Move to the end of the current line.
    pub fn end(&mut self) {
        self.edit_history();
        self.cursor = self
            .text
            .get(self.cursor..)
            .and_then(|s| s.find('\n'))
            .map_or(self.text.len(), |i| self.cursor + i);
    }

    /// Move between logical input lines, preserving display columns where possible.
    pub fn vertical(&mut self, backwards: bool) {
        self.edit_history();
        let start = self.text[..self.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let column = self.text[start..self.cursor].replace('\t', "    ").width();
        let target = if backwards {
            if start == 0 {
                return;
            }
            self.text[..start - 1]
                .rfind('\n')
                .map_or(0, |index| index + 1)
        } else {
            let Some(next) = self.text[self.cursor..].find('\n') else {
                return;
            };
            self.cursor + next + 1
        };
        let end = self.text[target..]
            .find('\n')
            .map_or(self.text.len(), |index| target + index);
        let mut used = 0;
        self.cursor = target;
        for grapheme in self.text[target..end].graphemes(true) {
            let width = if grapheme == "\t" {
                4
            } else {
                grapheme.width()
            };
            if used + width > column {
                break;
            }
            used += width;
            self.cursor += grapheme.len();
        }
    }

    /// Delete the preceding grapheme.
    pub fn backspace(&mut self) {
        let end = self.cursor;
        self.left();
        self.text.replace_range(self.cursor..end, "");
        self.align_cursor();
    }

    /// Delete the next grapheme.
    pub fn delete(&mut self) {
        let start = self.cursor;
        self.right();
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.align_cursor();
    }

    /// Replace another conversation's recall entries without losing the draft.
    pub fn replace_history(&mut self, entries: impl Iterator<Item = String>) {
        if self.history_position.is_some() {
            self.text.clone_from(&self.draft);
            self.cursor = self.draft_cursor;
            self.restored_input = self.draft_restored;
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
        }
    }

    fn recall(&mut self, entry: HistoryInput) {
        self.set(entry.text);
        self.restored_input = entry.restored;
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

    fn align_cursor(&mut self) {
        self.cursor = self
            .text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .find(|i| *i >= self.cursor)
            .unwrap_or(self.text.len());
    }
}
