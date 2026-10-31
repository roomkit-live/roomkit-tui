//! Application-owned forms. Credentials travel only in explicit submissions.

use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};

use crate::Choice;

/// One host-issued form, scoped by the enclosing event's conversation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FormSpec {
    /// Opaque revision/token validated by the host on submission.
    pub id: String,
    /// Advertised application command handling the submission.
    pub command: String,
    /// User-facing heading.
    pub title: String,
    /// Ordered fields; hosts must never populate secret defaults.
    pub fields: Vec<FormField>,
}

impl FormSpec {
    /// A form submitted to `command` with the opaque revision `id`.
    pub fn new(
        id: impl Into<String>,
        command: impl Into<String>,
        title: impl Into<String>,
        fields: Vec<FormField>,
    ) -> Self {
        Self {
            id: id.into(),
            command: command.into(),
            title: title.into(),
            fields,
        }
    }
}

/// An editable value with host-owned semantics and validation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FormField {
    /// Stable key within the form.
    pub name: String,
    /// User-facing name.
    pub label: String,
    /// Guidance, including the meaning of an empty value.
    pub help: String,
    /// Initial non-secret value. Ignored for secret fields.
    pub value: String,
    /// Whether the view requires a nonempty value before submission.
    pub required: bool,
    /// Presentation and input behavior.
    pub kind: FieldKind,
}

impl FormField {
    /// An optional, initially empty field.
    pub fn new(name: impl Into<String>, label: impl Into<String>, kind: FieldKind) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            help: String::new(),
            value: String::new(),
            required: false,
            kind,
        }
    }

    /// Set the guidance shown under the field.
    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = help.into();
        self
    }

    /// Set the initial non-secret value.
    #[must_use]
    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }

    /// Require a nonempty value before submission.
    #[must_use]
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }
}

/// Generic field editors; provider and product concepts belong to the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum FieldKind {
    /// Single line of text.
    Text,
    /// Masked, initially empty text, excluded from composer history.
    Secret,
    /// Text accepting explicit line breaks.
    Multiline,
    /// One of the host's opaque values.
    Choice {
        /// Available values and their labels.
        choices: Vec<Choice>,
    },
    /// An editor introduced by a newer host. Terminals refuse the form rather
    /// than guess how to display or protect its value.
    #[serde(other)]
    Unknown,
}

/// Submitted values. Debug intentionally hides every value, including secrets.
/// Transport serialization includes values: hosts must protect the transport,
/// validate the form token and never log or persist submissions as chat events.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FormValues(BTreeMap<String, String>);

impl FormValues {
    /// No values.
    pub fn new() -> Self {
        Self::default()
    }

    /// The value submitted for a field.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    /// Set a field's value, replacing any previous one.
    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.0.insert(name.into(), value.into());
    }

    /// Fields and values in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    /// Number of submitted fields.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether no field was submitted.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for FormValues {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self(
            iter.into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
        )
    }
}

impl fmt::Debug for FormValues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FormValues")
            .field("fields", &self.0.len())
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for FormField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FormField")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field(
                "value",
                &if matches!(self.kind, FieldKind::Text | FieldKind::Multiline) {
                    &self.value
                } else {
                    "[redacted]"
                },
            )
            .finish_non_exhaustive()
    }
}
