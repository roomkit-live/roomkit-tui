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

/// Why a terminal refuses a whole snapshot and keeps the view it shows. An
/// `index` is the position of the refused entry in its list, from 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SnapshotError {
    /// The conversation identity is longer than [`MAX_LABEL_BYTES`].
    Conversation,
    /// More than [`MAX_SNAPSHOT_ENTRIES`] actors, such as every author of a
    /// busy room.
    Actors,
    /// The selected agent is not one of the actors.
    UnknownSelectedAgent,
    /// This command is empty, oversized, badly named, a repeat of an earlier
    /// one, or the first beyond [`MAX_SNAPSHOT_ENTRIES`].
    Command {
        /// Position in [`Snapshot::commands`].
        index: usize,
    },
    /// The interaction modes do not fit the commands; see [`crate::ModeState::fits`].
    Modes,
    /// This composer action is invalid, repeats an earlier prefix, names an
    /// unadvertised command, or is the first beyond [`MAX_INPUT_ACTIONS`].
    InputAction {
        /// Position in [`Snapshot::input_actions`].
        index: usize,
    },
    /// This pending permission is oversized, repeats an earlier identity, is
    /// outside the active run, or is the first beyond [`MAX_PENDING_PERMISSIONS`].
    Permission {
        /// Position in [`Snapshot::permissions`].
        index: usize,
    },
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conversation => write!(
                f,
                "Host snapshot has a conversation identity over {MAX_LABEL_BYTES} bytes"
            ),
            Self::Actors => write!(
                f,
                "Host snapshot has more than {MAX_SNAPSHOT_ENTRIES} actors"
            ),
            Self::UnknownSelectedAgent => {
                f.write_str("Host snapshot has an unknown selected agent")
            }
            Self::Command { index } => write!(
                f,
                "Host snapshot has an invalid, repeated or extra command at index {index}"
            ),
            Self::Modes => f.write_str("Host snapshot has invalid interaction modes"),
            Self::InputAction { index } => {
                write!(
                    f,
                    "Host snapshot has an invalid, repeated or extra composer action at index {index}"
                )
            }
            Self::Permission { index } => write!(
                f,
                "Host snapshot has an invalid, repeated or extra permission at index {index}"
            ),
        }
    }
}

impl std::error::Error for SnapshotError {}

impl Snapshot {
    /// Whether a terminal accepts this snapshot. A terminal refuses the whole
    /// snapshot, shows why and keeps the view it shows. Labels and history are
    /// not checked: a terminal shortens them instead.
    pub fn validate(&self) -> Result<(), SnapshotError> {
        if self.conversation.as_str().len() > MAX_LABEL_BYTES {
            return Err(SnapshotError::Conversation);
        }
        if self.actors.len() > MAX_SNAPSHOT_ENTRIES {
            return Err(SnapshotError::Actors);
        }
        if !self.actors.iter().any(|a| a.id == self.selected_agent) {
            return Err(SnapshotError::UnknownSelectedAgent);
        }
        let names = advertised(&self.commands).map_err(|index| SnapshotError::Command { index })?;
        if self.mode.as_ref().is_some_and(|m| !m.fits(&self.commands)) {
            return Err(SnapshotError::Modes);
        }
        if let Some(index) = refused_action(&self.input_actions, &names) {
            return Err(SnapshotError::InputAction { index });
        }
        if let Some(index) = self.refused_permission() {
            return Err(SnapshotError::Permission { index });
        }
        Ok(())
    }

    fn refused_permission(&self) -> Option<usize> {
        let mut ids = BTreeSet::new();
        first_refused(&self.permissions, MAX_PENDING_PERMISSIONS, |permission| {
            self.active_run.as_ref() == Some(&permission.run)
                && permission.fits()
                && ids.insert(&permission.id)
        })
    }
}

/// The position of the first entry refused: the first beyond `cap`, checked
/// before any entry is read, or else the first `fits` rejects. `fits` sees
/// the entries in order, so it can refuse a repeat.
fn first_refused<'a, T>(
    entries: &'a [T],
    cap: usize,
    mut fits: impl FnMut(&'a T) -> bool,
) -> Option<usize> {
    if entries.len() > cap {
        return Some(cap);
    }
    entries.iter().position(|entry| !fits(entry))
}

/// The names of valid, distinct commands, or the position of the first
/// command refused.
fn advertised(commands: &[CommandSpec]) -> Result<BTreeSet<&str>, usize> {
    let mut names = BTreeSet::new();
    let refused = first_refused(commands, MAX_SNAPSHOT_ENTRIES, |command| {
        command_fits(command) && names.insert(command.name.as_str())
    });
    refused.map_or(Ok(names), Err)
}

fn command_fits(command: &CommandSpec) -> bool {
    !command.name.is_empty()
        && command.name.len() <= MAX_LABEL_BYTES
        && command
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        && command.description.len() <= MAX_COMMAND_DESCRIPTION_BYTES
        && command.arguments.len() <= MAX_LABEL_BYTES
        && command.choices.len() <= MAX_CHOICES
        && command.choices.iter().all(|c| c.len() <= MAX_LABEL_BYTES)
}

/// The position of the first composer action refused. Each prefix is a
/// distinct character that is neither alphanumeric, whitespace, a control nor
/// the `\` escape, and sends to an advertised command.
fn refused_action(actions: &[InputAction], names: &BTreeSet<&str>) -> Option<usize> {
    let mut prefixes = BTreeSet::new();
    first_refused(actions, MAX_INPUT_ACTIONS, |action| {
        !(action.prefix.is_alphanumeric()
            || action.prefix.is_whitespace()
            || action.prefix.is_control()
            || action.prefix == '\\')
            && action.description.len() <= MAX_INPUT_ACTION_DESCRIPTION_BYTES
            && names.contains(action.command.as_str())
            && prefixes.insert(action.prefix)
    })
}
