//! Application-owned background status and read-only detail panels.
use crate::Choice;
use serde::{Deserialize, Serialize};

/// A compact status control invoking an advertised application command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BackgroundStatus {
    /// Short visible status, supplied by the host.
    pub label: String,
    /// Advertised command to invoke on click or F6.
    pub command: String,
    /// Exact host-owned argument for that command.
    pub arguments: String,
}

impl BackgroundStatus {
    /// A status whose activation invokes `command` with `arguments`.
    pub fn new(
        label: impl Into<String>,
        command: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            command: command.into(),
            arguments: arguments.into(),
        }
    }
}

/// Read-only information and explicit host actions, independent of any tool domain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PanelSpec {
    /// Opaque identity; updates cannot reopen a dismissed or replaced panel.
    pub id: String,
    /// Advertised application command receiving action values.
    pub command: String,
    /// Panel heading.
    pub title: String,
    /// Bounded plain text, never interpreted as executable terminal controls.
    pub body: String,
    /// Selectable actions; choosing one sends its exact value to the command.
    pub actions: Vec<Choice>,
    /// Argument sent when the user dismisses the panel, for host-side cleanup.
    pub dismiss: String,
}

impl PanelSpec {
    /// A panel without actions; dismissing it sends an empty argument.
    pub fn new(
        id: impl Into<String>,
        command: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            command: command.into(),
            title: title.into(),
            body: body.into(),
            actions: Vec::new(),
            dismiss: String::new(),
        }
    }

    /// Set the selectable actions.
    #[must_use]
    pub fn with_actions(mut self, actions: Vec<Choice>) -> Self {
        self.actions = actions;
        self
    }

    /// Set the argument sent when the user dismisses the panel.
    #[must_use]
    pub fn with_dismiss(mut self, dismiss: impl Into<String>) -> Self {
        self.dismiss = dismiss.into();
        self
    }
}
