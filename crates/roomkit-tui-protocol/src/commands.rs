//! What a host offers the composer: commands, choices, modes and prefixes.

use serde::{Deserialize, Serialize};

/// A host command advertised to the input composer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CommandSpec {
    /// Command name without its leading slash; one whitespace-free word.
    pub name: String,
    /// Short human-readable help.
    pub description: String,
    /// Argument hint, such as `[model]`.
    pub arguments: String,
    /// Optional first-argument choices supplied by the host.
    pub choices: Vec<String>,
}

impl CommandSpec {
    /// A command without arguments or choices.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            arguments: String::new(),
            choices: Vec::new(),
        }
    }

    /// Set the argument hint, such as `[model]`.
    #[must_use]
    pub fn with_arguments(mut self, arguments: impl Into<String>) -> Self {
        self.arguments = arguments.into();
        self
    }

    /// Set the first-argument choices offered by completion.
    #[must_use]
    pub fn with_choices(mut self, choices: Vec<String>) -> Self {
        self.choices = choices;
        self
    }
}

/// An application-owned choice: its display text is separate from its value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Choice {
    /// Opaque argument passed back to the advertised command.
    pub value: String,
    /// Human-readable primary label.
    pub label: String,
    /// Secondary context, such as a date or summary.
    pub description: String,
}

impl Choice {
    /// A value and its label, without secondary text.
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: String::new(),
        }
    }

    /// Set the secondary context.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }
}

/// Host-defined interaction modes. Their meaning and policy belong to the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModeState {
    /// Advertised command that accepts a mode's opaque value.
    pub command: String,
    /// Host-confirmed selected value.
    pub selected: String,
    /// Modes in keyboard cycling order.
    pub choices: Vec<Choice>,
}

impl ModeState {
    /// Modes cycled through `command`, with `selected` confirmed by the host.
    pub fn new(
        command: impl Into<String>,
        selected: impl Into<String>,
        choices: Vec<Choice>,
    ) -> Self {
        Self {
            command: command.into(),
            selected: selected.into(),
            choices,
        }
    }
}

/// A composer prefix invoking an advertised host command, without a model call.
/// The host owns the meaning and execution policy. A leading backslash escapes
/// an advertised prefix into ordinary message text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct InputAction {
    /// One punctuation character, distinct from the terminal command prefix.
    pub prefix: char,
    /// Advertised command receiving the remaining text verbatim.
    pub command: String,
    /// Short input-mode guidance supplied by the application.
    pub description: String,
}

impl InputAction {
    /// A prefix that sends the remaining composer text to `command`.
    pub fn new(prefix: char, command: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            prefix,
            command: command.into(),
            description: description.into(),
        }
    }
}
