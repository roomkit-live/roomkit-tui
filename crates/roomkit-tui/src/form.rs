use std::{collections::BTreeSet, fmt};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use roomkit_tui_protocol::{Command, FieldKind, FormSpec, FormValues};

use crate::{SessionError, editor::Editor};

/// A bounded host form with a draft independent of conversation history.
/// Debug never exposes field values. Secret defaults are discarded.
pub struct FormState {
    pub(crate) spec: FormSpec,
    pub(crate) editors: Vec<Editor>,
    pub(crate) selected: usize,
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
        if editor.text().len().saturating_add(text.len()) <= 8192 {
            editor.insert(&text);
        }
    }

    pub(crate) fn key(&mut self, key: KeyEvent) {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if key.code == KeyCode::Tab
            || key.code == KeyCode::BackTab
            || (key.code == KeyCode::Enter && !alt)
        {
            let count = self.editors.len();
            self.selected = (self.selected
                + if key.code == KeyCode::BackTab {
                    count - 1
                } else {
                    1
                })
                % count;
            return;
        }
        if let FieldKind::Choice { choices } = &self.spec.fields[self.selected].kind {
            let editor = &mut self.editors[self.selected];
            let index = choices
                .iter()
                .position(|c| c.value == editor.text())
                .unwrap_or(0);
            let next = match key.code {
                KeyCode::Left | KeyCode::Up => (index + choices.len() - 1) % choices.len(),
                KeyCode::Right | KeyCode::Down | KeyCode::Char(' ') => (index + 1) % choices.len(),
                _ => return,
            };
            editor.set(choices[next].value.clone());
            return;
        }
        match key.code {
            KeyCode::Char(c) if !control && !alt => self.insert(&c.to_string()),
            KeyCode::Enter if alt => self.insert("\n"),
            KeyCode::Backspace => self.editors[self.selected].backspace(),
            KeyCode::Delete => self.editors[self.selected].delete(),
            KeyCode::Left => self.editors[self.selected].left(),
            KeyCode::Right => self.editors[self.selected].right(),
            KeyCode::Home => self.editors[self.selected].home(),
            KeyCode::End => self.editors[self.selected].end(),
            KeyCode::Up => self.editors[self.selected].vertical(true),
            KeyCode::Down => self.editors[self.selected].vertical(false),
            KeyCode::Char('u') if control => self.editors[self.selected].set(String::new()),
            _ => {}
        }
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
