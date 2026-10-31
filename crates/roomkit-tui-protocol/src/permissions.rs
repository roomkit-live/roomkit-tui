//! One-time human decisions requested by the host.

use serde::{Deserialize, Serialize};

use crate::{ActorId, PermissionId, RunId};

/// A human decision. Approval is scoped to this one request. Unknown values
/// fail to parse, so a newer client can never be read as an approval.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Decision {
    /// Allow this operation under the host's policy.
    Allow,
    /// Refuse this operation.
    Deny,
}

/// A pending human decision emitted by the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Permission {
    /// Identity used to answer precisely this decision.
    pub id: PermissionId,
    /// Agent asking for the operation.
    pub actor: ActorId,
    /// Owning execution.
    pub run: RunId,
    /// Short title describing the operation.
    pub title: String,
    /// Concrete details the person is approving.
    pub detail: String,
    /// Concise description for the confirmation in history; defaults to the title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

impl Permission {
    /// A decision requested by `actor` during `run`.
    pub fn new(
        id: impl Into<PermissionId>,
        actor: impl Into<ActorId>,
        run: impl Into<RunId>,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            actor: actor.into(),
            run: run.into(),
            title: title.into(),
            detail: detail.into(),
            summary: None,
        }
    }

    /// Set the concise description kept in history.
    #[must_use]
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }
}

/// A confirmed, one-time human decision. Withdrawals are not human decisions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PermissionDecision {
    /// The exact request that was reviewed.
    pub permission: Permission,
    /// Explicit answer accepted by the host.
    pub decision: Decision,
    /// Human identity assigned by the authenticated host.
    pub decided_by: ActorId,
}

impl PermissionDecision {
    /// Record `decision` on `permission`, made by `decided_by`.
    pub fn new(permission: Permission, decision: Decision, decided_by: impl Into<ActorId>) -> Self {
        Self {
            permission,
            decision,
            decided_by: decided_by.into(),
        }
    }
}
