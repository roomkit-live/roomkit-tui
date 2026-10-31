//! What a host asks a person to decide.

use std::time::Duration;

use roomkit_tui_protocol::limits::{
    MAX_PERMISSION_DETAIL_BYTES, MAX_PERMISSION_SUMMARY_BYTES, MAX_PERMISSION_TITLE_BYTES,
};

/// A human decision the host needs before an action. The person sees every
/// field in full: oversized requests are refused, never shown truncated.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PermissionRequest {
    /// Short title of the action, at most 256 bytes.
    pub title: String,
    /// The concrete action the person approves, at most 64 KiB.
    pub detail: String,
    /// Concise label kept in history instead of the title, at most 512 bytes.
    pub summary: Option<String>,
    /// How long to wait before refusing.
    pub timeout: Duration,
}

impl PermissionRequest {
    /// A request refused after `timeout` without an answer.
    pub fn new(title: impl Into<String>, detail: impl Into<String>, timeout: Duration) -> Self {
        Self {
            title: title.into(),
            detail: detail.into(),
            summary: None,
            timeout,
        }
    }

    /// Set the concise label kept in history.
    #[must_use]
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }
}

#[derive(Default)]
pub(crate) struct Details {
    pub title: String,
    pub detail: String,
    pub summary: Option<String>,
    pub tool_call_id: Option<String>,
}

impl Details {
    pub(super) fn fits(&self) -> bool {
        self.title.len() <= MAX_PERMISSION_TITLE_BYTES
            && self.detail.len() <= MAX_PERMISSION_DETAIL_BYTES
            && self
                .summary
                .as_ref()
                .is_none_or(|s| s.len() <= MAX_PERMISSION_SUMMARY_BYTES)
    }
}
