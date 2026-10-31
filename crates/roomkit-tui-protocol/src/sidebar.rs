//! Optional, application-composed information beside the conversation.
use serde::{Deserialize, Serialize};

/// A scoped sidebar. Hosts send only display-safe information, never credentials.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SidebarSpec {
    /// Short heading above the sections.
    pub title: String,
    /// Ordered sections; the terminal accepts at most sixteen.
    pub sections: Vec<SidebarSection>,
}

impl SidebarSpec {
    /// A titled sidebar with ordered sections.
    pub fn new(title: impl Into<String>, sections: Vec<SidebarSection>) -> Self {
        Self {
            title: title.into(),
            sections,
        }
    }
}

/// One named group of information and optional navigation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SidebarSection {
    /// Display heading.
    pub title: String,
    /// Optional action on the heading, such as opening a configuration selector.
    pub action: Option<SidebarAction>,
    /// Host entries or a live projection of generic conversation state.
    pub content: SidebarContent,
}

impl SidebarSection {
    /// A section without a heading action.
    pub fn new(title: impl Into<String>, content: SidebarContent) -> Self {
        Self {
            title: title.into(),
            action: None,
            content,
        }
    }

    /// Set the action invoked from the heading.
    #[must_use]
    pub fn with_action(mut self, action: SidebarAction) -> Self {
        self.action = Some(action);
        self
    }
}

/// Content sources keep streaming model and context updates authoritative.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SidebarContent {
    /// Selected actor, effective provider, model, and reasoning effort.
    Agent,
    /// Latest context occupancy and execution consumption, including freshness.
    Context,
    /// Application-defined entries, independent of a particular tool domain.
    Entries {
        /// Entries in display order.
        entries: Vec<SidebarEntry>,
    },
    /// A stacked bar and its legend, for any proportion: storage, quotas, budget.
    Meter {
        /// Parts in display order; the terminal accepts at most sixteen.
        segments: Vec<MeterSegment>,
        /// Full scale of the bar; absent or zero scales to the sum of the values.
        capacity: Option<u64>,
    },
    /// The context gauge split into host-estimated parts. The terminal adds
    /// the remainder of the latest measurement, so it stays current without
    /// the host resending the sidebar.
    ContextBreakdown {
        /// Parts the host measured or estimated; at most sixteen.
        parts: Vec<MeterSegment>,
        /// Label of the measured remainder, such as `Messages`; at most 256 bytes.
        remainder: String,
    },
    /// Content introduced by a newer host. Terminals omit the section.
    #[serde(other)]
    Unknown,
}

/// Semantic status; colors are selected by the terminal application's theme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SidebarTone {
    /// Secondary or unavailable information.
    Muted,
    /// Active work or navigation.
    Accent,
    /// Successful or connected state.
    Success,
    /// Pending, disabled or degraded state.
    Warning,
    /// Failed state.
    Error,
    /// Ordinary information, also used for tones introduced by a newer host.
    #[default]
    #[serde(other)]
    Neutral,
}

/// One part of a stacked meter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MeterSegment {
    /// Legend label, bounded to 256 bytes by the terminal.
    pub label: String,
    /// Amount in the meter's unit, such as tokens or bytes.
    pub value: u64,
    /// Categorical slot; the terminal's theme picks the actual color.
    pub color: MeterColor,
    /// Whether the value is an estimate, shown with `≈`.
    pub estimated: bool,
}

impl MeterSegment {
    /// A measured part.
    pub fn new(label: impl Into<String>, value: u64, color: MeterColor) -> Self {
        Self {
            label: label.into(),
            value,
            color,
            estimated: false,
        }
    }

    /// Mark the value as an estimate.
    #[must_use]
    pub fn estimated(mut self) -> Self {
        self.estimated = true;
        self
    }
}

/// A categorical color slot. Hosts choose a slot, never a color, so the
/// part stays readable in every theme and without color.
///
/// The wire form is `{"series": n}`, with `n` from 0 to 255, or `"muted"`;
/// any other value, from a newer host, reads as [`MeterColor::Muted`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MeterColor {
    /// One of eight categorical colors; indexes 8 to 255 wrap modulo eight.
    Series(u8),
    /// Secondary parts.
    Muted,
}

impl<'de> Deserialize<'de> for MeterColor {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Series { series: u8 },
            Other(serde::de::IgnoredAny),
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Series { series } => Self::Series(series),
            Wire::Other(_) => Self::Muted,
        })
    }
}

/// A compact item with optional secondary text and a host action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SidebarEntry {
    /// Primary label, bounded to 256 bytes by the terminal.
    pub label: String,
    /// Secondary text, bounded to 1024 bytes by the terminal.
    pub detail: String,
    /// Semantic emphasis for the primary label.
    pub tone: SidebarTone,
    /// Explicit navigation; receiving this entry never executes it.
    pub action: Option<SidebarAction>,
}

impl SidebarEntry {
    /// A neutral entry without detail or action.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: String::new(),
            tone: SidebarTone::Neutral,
            action: None,
        }
    }

    /// Set the secondary text.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    /// Set the semantic emphasis.
    #[must_use]
    pub fn with_tone(mut self, tone: SidebarTone) -> Self {
        self.tone = tone;
        self
    }

    /// Set the explicit navigation action.
    #[must_use]
    pub fn with_action(mut self, action: SidebarAction) -> Self {
        self.action = Some(action);
        self
    }
}

/// An advertised host command, invoked only by explicit user interaction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SidebarAction {
    /// Command name from the current snapshot.
    pub command: String,
    /// Opaque host argument, bounded to 256 bytes by the terminal.
    pub arguments: String,
}

impl SidebarAction {
    /// Invoke `command` with `arguments`.
    pub fn new(command: impl Into<String>, arguments: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            arguments: arguments.into(),
        }
    }
}
