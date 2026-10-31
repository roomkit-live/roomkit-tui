use roomkit_tui_protocol::{Choice, Command};

use crate::{editor::Editor, session::bounded};

/// A host-owned choice catalogue with a local filter and keyboard selection.
/// Filtering never changes the application's opaque values.
#[derive(Clone, Debug)]
pub struct ChoicePicker {
    /// Advertised command to invoke when an entry is confirmed.
    pub(crate) command: String,
    /// Selector heading supplied by the application.
    pub(crate) title: String,
    /// Current filter, independent of the conversation draft.
    pub(crate) filter: Editor,
    /// Index into the currently filtered matches.
    pub(crate) selected: usize,
    choices: Vec<Choice>,
}

impl ChoicePicker {
    pub(crate) fn new(command: String, title: String, choices: Vec<Choice>) -> Self {
        Self {
            command,
            title: bounded(&title, 256),
            choices: choices
                .into_iter()
                .take(500)
                .filter(|choice| choice.value.len() <= 1024)
                .map(|choice| {
                    Choice::new(choice.value, bounded(&choice.label, 256))
                        .with_description(bounded(&choice.description, 512))
                })
                .collect(),
            filter: Editor::default(),
            selected: 0,
        }
    }

    /// Matching choices in host order, searchable by label, description or value.
    /// Advertised command receiving the chosen value.
    pub fn command(&self) -> &str {
        &self.command
    }

    /// Selector heading.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Text typed to filter the choices.
    pub fn filter(&self) -> &str {
        self.filter.text()
    }

    /// Index of the highlighted entry among [`Self::entries`].
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Choices matching the filter, in host order.
    pub fn entries(&self) -> Vec<&Choice> {
        let query = self.filter.text().to_lowercase();
        self.choices
            .iter()
            .filter(|choice| {
                [&choice.label, &choice.description, &choice.value]
                    .iter()
                    .any(|text| text.to_lowercase().contains(&query))
            })
            .collect()
    }

    pub(crate) fn navigate(&mut self, backwards: bool) {
        let count = self.entries().len();
        if count > 0 {
            self.selected = if backwards {
                (self.selected + count - 1) % count
            } else {
                (self.selected + 1) % count
            };
        }
    }

    pub(crate) fn selected_command(&self) -> Option<Command> {
        self.entries()
            .get(self.selected)
            .map(|choice| Command::Invoke {
                name: self.command.clone(),
                arguments: choice.value.clone(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtering_keeps_opaque_values_separate_from_labels_and_cycles_matches() {
        let mut picker = ChoicePicker::new(
            "resume".into(),
            "Previous conversations".into(),
            vec![
                Choice::new("opaque-1", "Project review").with_description("2026-10-29"),
                Choice::new("opaque-2", "Résumé du module").with_description("2026-10-28"),
            ],
        );
        picker.filter.insert("RÉSUMÉ");
        assert_eq!(
            picker.selected_command(),
            Some(Command::Invoke {
                name: "resume".into(),
                arguments: "opaque-2".into()
            })
        );
        picker.filter.set(String::new());
        picker.navigate(true);
        assert_eq!(picker.selected, 1);
        picker.navigate(false);
        assert_eq!(picker.selected, 0);
        picker.filter.insert("does not exist");
        assert!(picker.selected_command().is_none());
    }
}
