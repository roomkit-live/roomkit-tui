use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::pasted::Pasted;
use history::HistoryInput;

mod history;

/// A Unicode grapheme-aware composer, independent of terminal input events.
#[derive(Clone, Debug, Default)]
pub(crate) struct Editor {
    text: String,
    cursor: usize,
    /// The long pastes the text holds as blocks.
    pasted: Pasted,
    history: Vec<HistoryInput>,
    restored_input: bool,
    history_position: Option<usize>,
    browsing_history: bool,
    draft: String,
    draft_cursor: usize,
    draft_restored: bool,
    draft_pasted: Pasted,
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

    /// The text as it is sent: each block of a long paste replaced by the
    /// paste.
    pub fn expanded(&self) -> String {
        self.pasted.expand(&self.text)
    }

    /// How many bytes the text takes as it is sent.
    pub fn expanded_len(&self) -> usize {
        self.pasted.expanded_len(&self.text)
    }

    /// Replace the draft and put the cursor at its end.
    pub fn set(&mut self, text: String) {
        self.edit_history();
        self.restored_input = false;
        self.text = text;
        self.cursor = self.text.len();
        self.pasted = Pasted::default();
    }

    /// Discard the draft and leave history navigation without submitting it.
    pub(crate) fn clear(&mut self) {
        self.set(String::new());
        self.history_position = None;
        self.draft.clear();
        self.draft_cursor = 0;
        self.draft_restored = false;
        self.draft_pasted = Pasted::default();
    }

    /// Insert pasted text at the cursor: a long paste goes in as one block,
    /// which the message sent replaces by the paste.
    pub fn paste(&mut self, text: &str) {
        let clean = clean(text);
        match self.pasted.block(&clean) {
            Some(label) => self.insert_clean(&label),
            None => self.insert_clean(&clean),
        }
    }

    /// Insert text at the cursor. Control characters other than newlines and
    /// tabs are removed; pasting never submits. Inputs normalize CRLF to LF.
    pub fn insert(&mut self, text: &str) {
        self.insert_clean(&clean(text));
    }

    fn insert_clean(&mut self, clean: &str) {
        self.edit_history();
        if self.text.is_empty() {
            self.restored_input = false;
        }
        self.text.insert_str(self.cursor, clean);
        let end = self.cursor + clean.len();
        // Inserting a joiner can merge surrounding graphemes. Advance to the
        // resulting cluster boundary before accepting another editing command.
        self.cursor = end;
        self.align_cursor();
    }

    /// Move one grapheme left, or over a block.
    pub fn left(&mut self) {
        self.edit_history();
        if let Some(block) = self.pasted.ending_at(&self.text, self.cursor) {
            self.cursor = block.start;
            return;
        }
        self.cursor = self
            .text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .take_while(|i| *i < self.cursor)
            .last()
            .unwrap_or(0);
        // A mark typed after a block merges with its `]`: step over it whole.
        if let Some(block) = self.pasted.around(&self.text, self.cursor) {
            self.cursor = block.start;
        }
    }

    /// Move one grapheme right, or over a block.
    pub fn right(&mut self) {
        self.edit_history();
        if let Some(block) = self.pasted.starting_at(&self.text, self.cursor) {
            self.cursor = block.end;
            return;
        }
        self.cursor = self
            .text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .find(|i| *i > self.cursor)
            .unwrap_or(self.text.len());
        if let Some(block) = self.pasted.around(&self.text, self.cursor) {
            self.cursor = block.end;
        }
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
        self.align_cursor();
    }

    /// Delete the preceding grapheme, or block.
    pub fn backspace(&mut self) {
        let end = self.cursor;
        self.left();
        self.text.replace_range(self.cursor..end, "");
        self.pasted.forget_absent(&self.text);
        self.align_cursor();
    }

    /// Delete the next grapheme, or block.
    pub fn delete(&mut self) {
        let start = self.cursor;
        self.right();
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.pasted.forget_absent(&self.text);
        self.align_cursor();
    }

    /// Put the cursor on a grapheme boundary, after a block it fell in.
    fn align_cursor(&mut self) {
        if let Some(block) = self.pasted.around(&self.text, self.cursor) {
            self.cursor = block.end;
            return;
        }
        self.cursor = self
            .text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .find(|i| *i >= self.cursor)
            .unwrap_or(self.text.len());
    }
}

/// `text` as an input takes it: CRLF and CR as LF, no control characters
/// other than newlines and tabs.
fn clean(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}
