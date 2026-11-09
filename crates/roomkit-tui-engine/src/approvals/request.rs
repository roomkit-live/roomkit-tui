//! What a host asks a person to decide.

use std::time::Duration;

use roomkit_tui_protocol::{Permission, RunId};

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
    /// The group of views that may answer it, by its name: any view showing
    /// a room of that group, wherever the request was asked.
    pub group: Option<String>,
}

impl PermissionRequest {
    /// A request refused after `timeout` without an answer.
    pub fn new(title: impl Into<String>, detail: impl Into<String>, timeout: Duration) -> Self {
        Self {
            title: title.into(),
            detail: detail.into(),
            summary: None,
            timeout,
            group: None,
        }
    }

    /// Let any view of `group` answer it.
    #[must_use]
    pub fn with_group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
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
    /// The run the request belongs to, by the token its start returned; the
    /// scope's most recent run if none.
    pub run: Option<u64>,
    pub group: Option<String>,
    /// The room the request is asked in.
    pub room: String,
}

impl Details {
    /// The `sequence`-th request of `run`, or `None` when a terminal could
    /// not show it in full and would hide it until it expires.
    pub(super) fn permission(&self, run: &RunId, agent: &str, sequence: u64) -> Option<Permission> {
        let id = crate::projection::permission_id(run, sequence);
        let mut permission = Permission::new(
            id,
            agent,
            run.clone(),
            self.title.clone(),
            self.detail.clone(),
        );
        permission.summary.clone_from(&self.summary);
        if self.group.is_some() {
            permission = permission.with_origin(self.room.as_str());
        }
        permission.fits().then_some(permission)
    }
}
