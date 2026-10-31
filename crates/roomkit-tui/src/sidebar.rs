//! Bounded sidebar projection and keyboard selection; no application policy.
use roomkit_tui_protocol::{SidebarAction, SidebarContent, SidebarSpec};

/// Local navigation state for application-owned information.
#[derive(Clone, Debug)]
pub struct SidebarState {
    /// Latest validated definition from the current conversation.
    pub(crate) spec: SidebarSpec,
    /// Wrapped content lines skipped from the top.
    pub(crate) scroll: usize,
    /// Whether navigation keys target this sidebar instead of the composer.
    pub(crate) focused: bool,
    /// Index of the selected action in section order.
    pub(crate) selected: usize,
}

impl SidebarState {
    /// The host's sidebar definition.
    pub fn spec(&self) -> &SidebarSpec {
        &self.spec
    }

    /// Whether sidebar navigation has the keyboard.
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Index of the highlighted action.
    pub fn selected(&self) -> usize {
        self.selected
    }

    pub(crate) fn new(spec: SidebarSpec, advertises: impl Fn(&str) -> bool) -> Option<Self> {
        let mut entries = 0;
        let action_valid = |a: &SidebarAction| a.arguments.len() <= 256 && advertises(&a.command);
        if spec.title.len() > 256 || spec.sections.len() > 16 {
            return None;
        }
        for section in &spec.sections {
            if section.title.len() > 256 || !section.action.as_ref().is_none_or(action_valid) {
                return None;
            }
            if let SidebarContent::Entries { entries: items } = &section.content {
                entries += items.len();
                if entries > 256
                    || items.iter().any(|entry| {
                        entry.label.len() > 256
                            || entry.detail.len() > 1024
                            || !entry.action.as_ref().is_none_or(action_valid)
                    })
                {
                    return None;
                }
            }
        }
        Some(Self {
            spec,
            scroll: 0,
            focused: false,
            selected: 0,
        })
    }

    pub(crate) fn actions(&self) -> impl Iterator<Item = &SidebarAction> {
        self.spec.sections.iter().flat_map(|section| {
            section.action.iter().chain(
                match &section.content {
                    SidebarContent::Entries { entries } => entries.as_slice(),
                    _ => &[],
                }
                .iter()
                .filter_map(|entry| entry.action.as_ref()),
            )
        })
    }

    pub(crate) fn navigate(&mut self, backwards: bool, count: usize) {
        let total = self.actions().count();
        self.selected = if self.selected >= total {
            if backwards {
                total.saturating_sub(1)
            } else {
                0
            }
        } else if backwards {
            self.selected.saturating_sub(count)
        } else {
            self.selected
                .saturating_add(count)
                .min(total.saturating_sub(1))
        };
    }
}
