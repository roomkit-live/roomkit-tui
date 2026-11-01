//! What a terminal accepts as a whole snapshot. Hosts check the same rules
//! before sending one, so a view never stays on a conversation the host has
//! already left.

use std::collections::BTreeSet;
use std::fmt;

use crate::limits::{
    MAX_CHOICES, MAX_COMMAND_DESCRIPTION_BYTES, MAX_INPUT_ACTION_DESCRIPTION_BYTES,
    MAX_INPUT_ACTIONS, MAX_LABEL_BYTES, MAX_PENDING_PERMISSIONS, MAX_SNAPSHOT_ENTRIES,
};
use crate::{CommandSpec, InputAction, Snapshot};

/// Why a terminal refuses a whole snapshot and keeps the view it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SnapshotError {
    /// The conversation identity or the actors exceed the view's bounds.
    Bounds,
    /// The selected agent is not one of the actors.
    UnknownSelectedAgent,
    /// A command is empty, oversized, badly named or advertised twice.
    Commands,
    /// The interaction modes do not fit the commands; see [`crate::ModeState::fits`].
    Modes,
    /// A composer action is invalid, repeated or names an unadvertised command.
    InputActions,
    /// A pending permission is oversized, repeated or outside the active run.
    Permissions,
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Bounds => "Host snapshot exceeds the view's bounds",
            Self::UnknownSelectedAgent => "Host snapshot has an unknown selected agent",
            Self::Commands => "Host snapshot has invalid or duplicate commands",
            Self::Modes => "Host snapshot has invalid interaction modes",
            Self::InputActions => "Host snapshot has invalid composer actions",
            Self::Permissions => "Host snapshot has invalid or duplicate permissions",
        })
    }
}

impl std::error::Error for SnapshotError {}

impl Snapshot {
    /// Whether a terminal accepts this snapshot. A refused snapshot changes
    /// nothing on screen. Labels and history are not checked: a terminal
    /// shortens them instead.
    pub fn validate(&self) -> Result<(), SnapshotError> {
        if self.conversation.as_str().len() > MAX_LABEL_BYTES
            || self.actors.len() > MAX_SNAPSHOT_ENTRIES
        {
            return Err(SnapshotError::Bounds);
        }
        if !self.actors.iter().any(|a| a.id == self.selected_agent) {
            return Err(SnapshotError::UnknownSelectedAgent);
        }
        let names = advertised(&self.commands).ok_or(SnapshotError::Commands)?;
        if self.mode.as_ref().is_some_and(|m| !m.fits(&self.commands)) {
            return Err(SnapshotError::Modes);
        }
        if !input_actions_fit(&self.input_actions, &names) {
            return Err(SnapshotError::InputActions);
        }
        if !self.permissions_fit() {
            return Err(SnapshotError::Permissions);
        }
        Ok(())
    }

    fn permissions_fit(&self) -> bool {
        let mut ids = BTreeSet::new();
        self.permissions.len() <= MAX_PENDING_PERMISSIONS
            && self.permissions.iter().all(|permission| {
                self.active_run.as_ref() == Some(&permission.run)
                    && permission.fits()
                    && ids.insert(&permission.id)
            })
    }
}

/// The names of valid, distinct commands; `None` when any command is invalid.
fn advertised(commands: &[CommandSpec]) -> Option<BTreeSet<&str>> {
    if commands.len() > MAX_SNAPSHOT_ENTRIES {
        return None;
    }
    let mut names = BTreeSet::new();
    for command in commands {
        let valid = !command.name.is_empty()
            && command.name.len() <= MAX_LABEL_BYTES
            && command
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            && command.description.len() <= MAX_COMMAND_DESCRIPTION_BYTES
            && command.arguments.len() <= MAX_LABEL_BYTES
            && command.choices.len() <= MAX_CHOICES
            && command.choices.iter().all(|c| c.len() <= MAX_LABEL_BYTES)
            && names.insert(command.name.as_str());
        if !valid {
            return None;
        }
    }
    Some(names)
}

/// Each prefix is one distinct punctuation character, other than the escape,
/// sending to an advertised command.
fn input_actions_fit(actions: &[InputAction], names: &BTreeSet<&str>) -> bool {
    let mut prefixes = BTreeSet::new();
    actions.len() <= MAX_INPUT_ACTIONS
        && actions.iter().all(|action| {
            !(action.prefix.is_alphanumeric()
                || action.prefix.is_whitespace()
                || action.prefix.is_control()
                || action.prefix == '\\')
                && action.description.len() <= MAX_INPUT_ACTION_DESCRIPTION_BYTES
                && names.contains(action.command.as_str())
                && prefixes.insert(action.prefix)
        })
}
