use roomkit_tui_protocol::{Command, PanelSpec};

/// A host-owned read-only panel, independent of the conversation composer.
#[derive(Clone, Debug)]
pub struct PanelState {
    /// Current host description and opaque actions.
    pub(crate) spec: PanelSpec,
    /// Selected action index.
    pub(crate) selected: usize,
    /// Wrapped body lines skipped from the top.
    pub(crate) scroll: usize,
}

impl PanelState {
    /// The host's panel definition.
    pub fn spec(&self) -> &PanelSpec {
        &self.spec
    }

    /// Index of the highlighted action.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Body lines scrolled past.
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub(crate) fn new(spec: PanelSpec) -> Option<Self> {
        if spec.id.is_empty()
            || spec.id.len() > 256
            || spec.title.len() > 256
            || spec.body.len() > 65_536
            || spec.actions.len() > 64
            || spec.dismiss.len() > 1024
            || spec
                .actions
                .iter()
                .any(|a| a.value.len() > 1024 || a.label.len() > 256 || a.description.len() > 512)
        {
            return None;
        }
        Some(Self {
            spec,
            selected: 0,
            scroll: 0,
        })
    }

    pub(crate) fn update(&mut self, spec: PanelSpec) {
        let Some(mut next) = Self::new(spec) else {
            return;
        };
        let value = self.spec.actions.get(self.selected).map(|a| &a.value);
        next.selected = next
            .spec
            .actions
            .iter()
            .position(|a| Some(&a.value) == value)
            .unwrap_or(0);
        next.scroll = self.scroll;
        *self = next;
    }

    pub(crate) fn navigate(&mut self, backwards: bool) {
        let count = self.spec.actions.len();
        if count > 0 {
            self.selected = if backwards {
                (self.selected + count - 1) % count
            } else {
                (self.selected + 1) % count
            };
        }
    }

    pub(crate) fn selected_command(&self) -> Option<Command> {
        self.spec
            .actions
            .get(self.selected)
            .map(|a| self.command(&a.value))
    }

    pub(crate) fn command(&self, arguments: &str) -> Command {
        Command::Invoke {
            name: self.spec.command.clone(),
            arguments: arguments.into(),
        }
    }
}
