//! Requests for host actions: decisions, modes, panels and background work.
//! Each is built only while its target is still on screen.

use roomkit_tui_protocol::{Command, Decision};

use super::{Focus, Session};

impl Session {
    /// Request the host's background panel without changing the input draft.
    pub fn background_command(&self) -> Option<Command> {
        let background = self.background.as_ref()?;
        Some(Command::Invoke {
            name: background.command.clone(),
            arguments: background.arguments.clone(),
        })
    }

    /// Resolve a click only while its view and exact action remain current.
    pub fn action_command(&self, target: &crate::render::ActionTarget) -> Option<Command> {
        if !self.permissions().is_empty() || !self.is_current(&target.conversation) {
            return None;
        }
        let valid = if target.sidebar {
            // Clicks reach the sidebar whether or not it has the keyboard.
            matches!(self.focus(), Focus::Composer | Focus::Sidebar)
                && self.sidebar.as_ref().is_some_and(|s| {
                    s.actions()
                        .any(|a| a.command == target.command && a.arguments == target.arguments)
                })
        } else if let Some(id) = &target.panel {
            self.panel.as_ref().is_some_and(|p| {
                p.spec.id == *id
                    && p.spec.command == target.command
                    && p.spec.actions.iter().any(|a| a.value == target.arguments)
            })
        } else {
            self.panel.is_none()
                && self
                    .background
                    .as_ref()
                    .is_some_and(|b| b.command == target.command && b.arguments == target.arguments)
        };
        valid.then(|| Command::Invoke {
            name: target.command.clone(),
            arguments: target.arguments.clone(),
        })
    }

    /// Build an answer to the first pending decision, without implicit approval.
    pub fn decide(&self, decision: Decision) -> Option<Command> {
        self.permissions().first().map(|p| Command::Decide {
            permission: p.id.clone(),
            decision,
        })
    }

    /// The command that moves to the next related room, or the previous one,
    /// when the host offers one and nothing is being written.
    pub fn cycle_room(&self, next: bool) -> Option<Command> {
        let name = self.snapshot.as_ref()?.cycle_command.clone()?;
        if !self.editor.text().is_empty()
            || self.pending.values().any(
                |command| matches!(command, Command::Invoke { name: sent, .. } if *sent == name),
            )
        {
            return None;
        }
        let arguments = if next { "next" } else { "previous" };
        Some(Command::Invoke {
            name,
            arguments: arguments.into(),
        })
    }

    /// Cycle the host's modes without changing the local policy optimistically.
    /// Repeated keys wait for the first request to settle; drafts are preserved.
    pub fn cycle_mode(&self) -> Option<Command> {
        let mode = self.snapshot.as_ref()?.mode.as_ref()?;
        if mode.choices.len() < 2
            || self.pending.values().any(
                |command| matches!(command, Command::Invoke { name, .. } if name == &mode.command),
            )
        {
            return None;
        }
        let index = mode
            .choices
            .iter()
            .position(|choice| choice.value == mode.selected)?;
        Some(Command::Invoke {
            name: mode.command.clone(),
            arguments: mode.choices[(index + 1) % mode.choices.len()].value.clone(),
        })
    }
}
