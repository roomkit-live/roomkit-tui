//! A first-argument lookup belongs to one editing gesture and one conversation.
use super::{Focus, Session, bounded};
use crate::{menu::MenuEntry, picker::bounded_choices};
use roomkit_tui_protocol::{Choice, Command, RequestId};

#[derive(Debug)]
pub(super) struct ActionChoices {
    prefix: char,
    state: Lookup,
}

#[derive(Debug)]
enum Lookup {
    New,
    Pending(RequestId),
    Ready(Vec<Choice>),
    Failed(String),
}

impl Session {
    pub(crate) fn choosing_action_argument(&self) -> bool {
        self.action_choices.is_some()
    }

    pub(crate) fn chosen_action(&self) -> Option<Command> {
        let action = self.input_action()?;
        let entry = self.menu.entries().get(self.menu.selected())?;
        Some(Command::Invoke {
            name: action.command.clone(),
            arguments: entry
                .replacement
                .strip_prefix(action.prefix)?
                .trim_end()
                .into(),
        })
    }

    pub(crate) fn choice_request(&self) -> Option<Command> {
        if self.focus() != Focus::Composer
            || !self.menu.visible
            || !matches!(self.action_choices.as_ref()?.state, Lookup::New)
        {
            return None;
        }
        Some(Command::CompleteInput {
            name: self.input_action()?.command.clone(),
        })
    }

    pub(crate) fn started_choice_request(&mut self, request: Option<RequestId>) {
        if let Some(choices) = self.action_choices.as_mut() {
            choices.state =
                request.map_or_else(|| Lookup::Failed(self.notice.clone()), Lookup::Pending);
            self.refresh_menu();
        }
    }

    pub(super) fn receive_action_choices(&mut self, id: RequestId, choices: Vec<Choice>) {
        if let Some(lookup) = self.pending_choices(id) {
            *lookup = Lookup::Ready(bounded_choices(choices));
            self.refresh_menu();
        }
    }

    pub(super) fn refuse_action_choices(&mut self, id: RequestId, reason: &str) {
        if let Some(lookup) = self.pending_choices(id) {
            *lookup = Lookup::Failed(bounded(reason, 256));
            self.refresh_menu();
        }
    }

    fn pending_choices(&mut self, id: RequestId) -> Option<&mut Lookup> {
        let lookup = &mut self.action_choices.as_mut()?.state;
        matches!(lookup, Lookup::Pending(request) if *request == id).then_some(lookup)
    }

    pub(super) fn refresh_action_choices(&mut self) {
        let prefix = self
            .input_action()
            .filter(|action| action.complete)
            .map(|action| action.prefix);
        let text = self.editor.text();
        let Some((prefix, query)) = prefix
            .and_then(|p| text.strip_prefix(p).map(|q| (p, q)))
            .filter(|(_, q)| !q.contains(char::is_whitespace))
            .filter(|_| {
                self.editor.cursor() == text.len()
                    && !self.editor.is_browsing_history()
                    && self.images.is_empty()
            })
        else {
            self.action_choices = None;
            return;
        };
        if self
            .action_choices
            .as_ref()
            .is_none_or(|choices| choices.prefix != prefix)
        {
            self.action_choices = Some(ActionChoices {
                prefix,
                state: Lookup::New,
            });
        }
        if let Some(choices) = &self.action_choices {
            let (entries, message) = choices.matches(query);
            self.menu.show_action(entries, message);
        }
    }
}

impl ActionChoices {
    fn matches(&self, query: &str) -> (Vec<MenuEntry>, Option<String>) {
        let choices = match &self.state {
            Lookup::New | Lookup::Pending(_) => return (vec![], Some("Loading choices…".into())),
            Lookup::Failed(reason) => return (vec![], Some(reason.clone())),
            Lookup::Ready(choices) => choices,
        };
        let query = query.to_lowercase();
        let entries = choices
            .iter()
            .filter(|c| !c.value.is_empty() && !c.value.contains(char::is_whitespace))
            .filter(|c| {
                [&c.label, &c.description, &c.value]
                    .iter()
                    .any(|s| s.to_lowercase().contains(&query))
            })
            .map(|c| MenuEntry {
                label: c.label.clone(),
                description: c.description.clone(),
                replacement: format!("{}{} ", self.prefix, c.value),
                argument: true,
            })
            .collect();
        let empty = choices.is_empty().then(|| "No choices available".into());
        (entries, empty)
    }
}
