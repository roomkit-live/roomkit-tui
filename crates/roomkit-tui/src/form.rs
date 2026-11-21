use std::{collections::BTreeSet, fmt};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use roomkit_tui_protocol::{Command, FieldKind, FormSpec, FormValues};

use crate::{SessionError, editor::Editor};

/// The most bytes a field holds.
const FIELD_BYTES: usize = 8192;

/// A bounded host form with a draft independent of conversation history.
/// Debug never exposes field values. Secret defaults are discarded.
pub struct FormState {
    pub(crate) spec: FormSpec,
    pub(crate) editors: Vec<Editor>,
    pub(crate) selected: usize,
    pub(crate) description_scroll: usize,
    pub(crate) submitted: bool,
}

impl fmt::Debug for FormState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FormState")
            .field("fields", &self.editors.len())
            .finish_non_exhaustive()
    }
}

impl FormState {
    /// The host's form definition. Initial values are not retained.
    pub fn spec(&self) -> &FormSpec {
        &self.spec
    }

    /// Index of the field being edited.
    pub fn selected(&self) -> usize {
        self.selected
    }

    pub(crate) fn new(mut spec: FormSpec) -> Option<Self> {
        let mut names = BTreeSet::new();
        if spec.id.is_empty() || spec.id.len() > 256 || spec.title.len() > 256
            || spec.description.len() > 16_384 || spec.dismiss.len() > 1024
            || spec.fields.is_empty() || spec.fields.len() > 32
            || spec.fields.iter().any(|field| {
                field.name.is_empty() || field.name.len() > 64 || !names.insert(&field.name)
                    || field.label.len() > 256 || field.help.len() > 1024 || field.value.len() > 8192
                    || matches!(field.kind, FieldKind::Unknown)
                    || matches!(&field.kind, FieldKind::Choice { choices } if choices.is_empty() || choices.len() > 500 || choices.iter().any(|c| c.value.len() > 1024 || c.label.len() > 256 || c.description.len() > 512))
            })
        {
            return None;
        }
        let editors = spec
            .fields
            .iter_mut()
            .map(|field| {
                let mut editor = Editor::default();
                if field.kind != FieldKind::Secret {
                    if let FieldKind::Choice { choices } = &field.kind
                        && !choices.iter().any(|c| c.value == field.value)
                    {
                        field.value.clone_from(&choices[0].value);
                    }
                    editor.insert(&field.value);
                }
                // The original values are not retained in a second debug-visible copy.
                field.value.clear();
                editor
            })
            .collect();
        Some(Self {
            spec,
            editors,
            selected: 0,
            description_scroll: 0,
            submitted: false,
        })
    }

    pub(crate) fn insert(&mut self, text: &str) {
        let field = &self.spec.fields[self.selected];
        if matches!(field.kind, FieldKind::Choice { .. }) {
            return;
        }
        let text = if field.kind == FieldKind::Multiline {
            text.to_owned()
        } else {
            text.replace(['\n', '\r', '\t'], " ")
        };
        let editor = &mut self.editors[self.selected];
        if editor.text().len().saturating_add(text.len()) <= FIELD_BYTES {
            editor.insert(&text);
        }
    }

    /// Takes a paste. A secret's replaces what its field held: the field is
    /// masked, so a paste made again after a refusal joined the first one
    /// unseen. One too long for the field, or with nothing to show, leaves
    /// it as it was: a terminal sends an empty paste for an empty clipboard.
    pub(crate) fn paste(&mut self, text: &str) {
        let secret = self.spec.fields[self.selected].kind == FieldKind::Secret;
        let shows = text.chars().any(|c| !c.is_control() && !c.is_whitespace());
        if secret && shows && text.len() <= FIELD_BYTES {
            self.editors[self.selected].set(String::new());
        }
        self.insert(text);
    }

    fn scroll_description(&mut self, key: KeyCode) -> bool {
        if self.spec.description.is_empty() {
            return false;
        }
        match key {
            KeyCode::PageUp => self.description_scroll = self.description_scroll.saturating_sub(5),
            KeyCode::PageDown => {
                self.description_scroll = self.description_scroll.saturating_add(5).min(16_384);
            }
            _ => return false,
        }
        true
    }

    pub(crate) fn key(&mut self, key: KeyEvent) {
        if self.scroll_description(key.code)
            || self.move_between_fields(key)
            || self.cycle_choice(key.code)
        {
            return;
        }
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char(c) if !control && !alt => self.insert(&c.to_string()),
            KeyCode::Enter if alt => self.insert("\n"),
            KeyCode::Up => self.editors[self.selected].vertical(true),
            KeyCode::Down => self.editors[self.selected].vertical(false),
            KeyCode::Char('u') if control => self.editors[self.selected].set(String::new()),
            _ => {
                self.editors[self.selected].edit_key(key);
            }
        }
    }

    /// Whether Enter saves the form: it holds one field, so the key has no
    /// next field to move to. Not a field of several lines, where Enter is
    /// the key pressed for the next line.
    pub(crate) fn enter_saves(&self) -> bool {
        self.editors.len() == 1 && self.spec.fields[0].kind != FieldKind::Multiline
    }

    /// Tab or Enter moves to the next field, Shift+Tab to the previous one;
    /// Alt+Enter is left to the field. Whether `key` moved.
    fn move_between_fields(&mut self, key: KeyEvent) -> bool {
        let count = self.editors.len();
        self.selected = match key.code {
            KeyCode::BackTab => (self.selected + count - 1) % count,
            KeyCode::Tab => (self.selected + 1) % count,
            KeyCode::Enter if !key.modifiers.contains(KeyModifiers::ALT) => {
                (self.selected + 1) % count
            }
            _ => return false,
        };
        true
    }

    /// On a choice field, the arrows and Space move to the previous or the
    /// next choice. Whether the field is a choice: it takes no other key.
    fn cycle_choice(&mut self, key: KeyCode) -> bool {
        let FieldKind::Choice { choices } = &self.spec.fields[self.selected].kind else {
            return false;
        };
        let editor = &mut self.editors[self.selected];
        let index = choices
            .iter()
            .position(|c| c.value == editor.text())
            .unwrap_or(0);
        let next = match key {
            KeyCode::Left | KeyCode::Up => (index + choices.len() - 1) % choices.len(),
            KeyCode::Right | KeyCode::Down | KeyCode::Char(' ') => (index + 1) % choices.len(),
            _ => return true,
        };
        editor.set(choices[next].value.clone());
        true
    }

    pub(crate) fn submission(&mut self) -> Result<Command, SessionError> {
        for (index, (field, editor)) in self.spec.fields.iter().zip(&self.editors).enumerate() {
            if field.required && editor.text().trim().is_empty() {
                self.selected = index;
                return Err(SessionError::RequiredField(field.label.clone()));
            }
        }
        let values: FormValues = self
            .spec
            .fields
            .iter()
            .zip(&self.editors)
            .map(|(field, editor)| (field.name.clone(), editor.text().to_owned()))
            .collect();
        Ok(Command::SubmitForm {
            name: self.spec.command.clone(),
            form: self.spec.id.clone(),
            values: Box::new(values),
        })
    }

    /// Display text of a field: the label of a choice, never its opaque value.
    pub(crate) fn display_value(&self, index: usize) -> String {
        let text = self.editors[index].text();
        match &self.spec.fields[index].kind {
            FieldKind::Choice { choices } => choices
                .iter()
                .find(|c| c.value == text)
                .map(|c| c.label.clone())
                .unwrap_or_default(),
            _ => text.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use roomkit_tui_protocol::{Choice, FormField};

    fn form() -> FormState {
        FormState::new(FormSpec::new(
            "revision-1",
            "settings",
            "Settings",
            vec![
                FormField::new("key", "API key", FieldKind::Secret)
                    .with_value("discard-this-default")
                    .required(),
                FormField::new(
                    "choice",
                    "Choice",
                    FieldKind::Choice {
                        choices: vec![Choice::new("a", "A"), Choice::new("b", "B")],
                    },
                )
                .with_value("a")
                .required(),
                FormField::new("notes", "Notes", FieldKind::Multiline),
            ],
        ))
        .unwrap()
    }

    #[test]
    fn secret_input_is_redacted_from_debug_and_frames_but_reaches_submission() {
        let mut form = form();
        assert!(form.editors[0].text().is_empty());
        assert!(form.submission().is_err());
        form.insert("fixture-secret-value");
        let command = form.submission().unwrap();
        assert!(!format!("{command:?}").contains("fixture-secret-value"));
        assert!(!format!("{form:?}").contains("fixture-secret-value"));
        let encoded = serde_json::to_value(&command).unwrap();
        assert_eq!(encoded["values"]["key"], "fixture-secret-value");
        let mut session = crate::Session::default();
        session.form = Some(form);
        for (width, height) in [(80, 24), (32, 16), (120, 40), (20, 10)] {
            let frame = crate::render::screen(
                &session,
                width,
                height,
                &crate::render::Renderers::default(),
            );
            assert!(!frame.lines.join("\n").contains("fixture-secret-value"));
            assert!(!format!("{session:?}").contains("fixture-secret-value"));
            assert!(frame.lines.len() <= usize::from(height));
            if let Some((x, y)) = frame.cursor {
                assert!(x < width && y < height);
            }
        }
    }

    #[test]
    fn tab_choice_and_multiline_editing_preserve_distinct_values() {
        let mut form = form();
        form.insert("secret");
        form.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        form.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        form.insert("ignored paste");
        form.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        form.insert("first");
        form.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        form.insert("second");
        form.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(form.selected, 1);
        let Command::SubmitForm { values, .. } = form.submission().unwrap() else {
            unreachable!()
        };
        assert_eq!(values.get("choice"), Some("b"));
        assert_eq!(values.get("notes"), Some("first\nsecond"));
        assert_eq!(values.get("key"), Some("secret"));
    }
}
