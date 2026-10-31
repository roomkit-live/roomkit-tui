use roomkit_tui_protocol::CommandSpec;

/// One completion supplied by an application's command catalogue.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct MenuEntry {
    /// Visible command name or argument value.
    pub label: String,
    /// Help supplied by the application.
    pub description: String,
    /// Text inserted into the composer, never directly executed.
    pub replacement: String,
    pub(crate) argument: bool,
}

/// A filtered command/choice menu. Selecting an entry only edits the draft.
#[derive(Clone, Debug, Default)]
pub struct CommandMenu {
    /// Current matches in host order.
    pub(crate) entries: Vec<MenuEntry>,
    /// Selected match, clamped after every refresh.
    pub(crate) selected: usize,
    /// Whether the picker is visible, including its empty state.
    pub(crate) visible: bool,
    /// Whether the menu contains first-argument choices.
    pub(crate) arguments: bool,
    query: String,
    dismissed: Option<String>,
}

impl CommandMenu {
    /// Whether the menu is shown beside the composer.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Completions matching the draft.
    pub fn entries(&self) -> &[MenuEntry] {
        &self.entries
    }

    /// Index of the highlighted completion.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Whether the completions are a command's argument choices.
    pub fn lists_arguments(&self) -> bool {
        self.arguments
    }

    /// Recompute choices after input or the host catalogue changes.
    pub(crate) fn refresh(
        &mut self,
        commands: &[CommandSpec],
        text: &str,
        cursor: usize,
        trigger: char,
    ) {
        if self.query != text {
            self.selected = 0;
            self.dismissed = None;
            self.query = text.into();
        }
        self.entries.clear();
        self.visible = false;
        self.arguments = false;
        if cursor != text.len() || text.contains('\n') || commands.is_empty() {
            return;
        }
        let Some(input) = text.strip_prefix(trigger) else {
            return;
        };
        if let Some((name, prefix)) = input.split_once(' ') {
            let Some(command) = commands.iter().find(|command| command.name == name) else {
                return;
            };
            if command.choices.is_empty() {
                return;
            }
            self.arguments = true;
            for choice in command
                .choices
                .iter()
                .filter(|choice| choice.starts_with(prefix))
            {
                self.entries.push(MenuEntry {
                    label: choice.clone(),
                    description: command.description.clone(),
                    replacement: format!("{trigger}{name} {choice}"),
                    argument: true,
                });
            }
            // A fully typed value is ready to submit; do not intercept Enter.
            if self.entries.len() == 1 && self.entries[0].replacement == text {
                return;
            }
        } else {
            for command in commands
                .iter()
                .filter(|command| command.name.starts_with(input))
            {
                self.entries.push(MenuEntry {
                    label: format!("{trigger}{}", command.name),
                    description: format!("{}  {}", command.arguments, command.description)
                        .trim()
                        .into(),
                    replacement: format!("{trigger}{} ", command.name),
                    argument: false,
                });
            }
        }
        self.selected = self.selected.min(self.entries.len().saturating_sub(1));
        self.visible = self.dismissed.as_deref() != Some(text);
    }

    /// Move the highlight, wrapping at either end of the result list.
    pub(crate) fn navigate(&mut self, backwards: bool) {
        if self.entries.is_empty() {
            return;
        }
        self.selected = if backwards {
            (self.selected + self.entries.len() - 1) % self.entries.len()
        } else {
            (self.selected + 1) % self.entries.len()
        };
    }

    /// Hide the menu until the draft changes, preserving the draft itself.
    pub(crate) fn dismiss(&mut self) {
        self.dismissed = Some(self.query.clone());
        self.visible = false;
    }

    pub(crate) fn suppress(&mut self, text: &str) {
        self.query = text.into();
        self.dismiss();
    }

    pub(crate) fn reopen(&mut self) {
        self.dismissed = None;
    }
}
