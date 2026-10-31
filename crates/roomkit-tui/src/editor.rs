use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// A Unicode grapheme-aware composer, independent of terminal input events.
#[derive(Clone, Debug, Default)]
pub(crate) struct Editor {
    text: String,
    cursor: usize,
    history: Vec<String>,
    history_position: Option<usize>,
    draft: String,
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
        self.text = text;
        self.cursor = self.text.len();
    }

    /// Discard the draft and leave history navigation without submitting it.
    pub(crate) fn clear(&mut self) {
        self.set(String::new());
        self.history_position = None;
        self.draft.clear();
    }

    /// Insert text at the cursor. Control characters other than newlines and
    /// tabs are removed; pasting never submits. Inputs normalize CRLF to LF.
    pub fn insert(&mut self, text: &str) {
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
        self.cursor = self
            .text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .find(|i| *i > self.cursor)
            .unwrap_or(self.text.len());
    }

    /// Move to the start of the current line.
    pub fn home(&mut self) {
        self.cursor = self
            .text
            .get(..self.cursor)
            .and_then(|s| s.rfind('\n'))
            .map_or(0, |i| i + 1);
    }

    /// Move to the end of the current line.
    pub fn end(&mut self) {
        self.cursor = self
            .text
            .get(self.cursor..)
            .and_then(|s| s.find('\n'))
            .map_or(self.text.len(), |i| self.cursor + i);
    }

    /// Move between logical input lines, preserving display columns where possible.
    pub fn vertical(&mut self, backwards: bool) {
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

    /// Remember successfully submitted text; keep at most 100 input entries.
    pub fn submitted(&mut self) {
        if !self.text.trim().is_empty() && self.history.last() != Some(&self.text) {
            self.history.push(self.text.clone());
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
                self.history.len() - 1
            }
            Some(position) => position.saturating_sub(1),
        };
        self.history_position = Some(position);
        if let Some(text) = self.history.get(position) {
            self.set(text.clone());
        }
    }

    /// Navigate toward the unfinished draft.
    pub fn history_next(&mut self) {
        let Some(position) = self.history_position else {
            return;
        };
        if let Some(text) = self.history.get(position + 1) {
            self.history_position = Some(position + 1);
            self.set(text.clone());
        } else {
            self.history_position = None;
            self.set(self.draft.clone());
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
