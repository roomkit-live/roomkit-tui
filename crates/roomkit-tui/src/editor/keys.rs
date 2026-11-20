//! The editing keys every text input shares: the composer, a form's fields,
//! a question's own words and a picker's filter. Each input keeps its own
//! keys for typing, sending and moving between lines or rows.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::Editor;

impl Editor {
    /// Apply a key that moves the cursor along the line or deletes: Left and
    /// Right, Home and End, Ctrl+A and Ctrl+E as in a shell, Backspace and
    /// Delete. Whether `key` was one of them.
    pub(crate) fn edit_key(&mut self, key: KeyEvent) -> bool {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.left(),
            KeyCode::Right => self.right(),
            KeyCode::Home => self.home(),
            KeyCode::Char('a') if control => self.home(),
            KeyCode::End => self.end(),
            KeyCode::Char('e') if control => self.end(),
            _ => return false,
        }
        true
    }
}
