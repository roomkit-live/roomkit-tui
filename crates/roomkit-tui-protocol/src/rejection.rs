//! Why a host refused a request: a machine-readable code and display text.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Stable classification of a refused request. Clients branch on the code and
/// display the message; they never parse the message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorCode {
    /// The request is malformed, out of bounds or scoped to another conversation.
    Invalid,
    /// Another execution is active in this conversation; retry after it ends.
    Busy,
    /// The command, permission, run or conversation is no longer available.
    Unavailable,
    /// The host refused the request under its own policy.
    Refused,
    /// The host is temporarily overloaded; retry later.
    Overloaded,
    /// This host does not support the requested command.
    Unsupported,
    /// The host failed while handling the request.
    Failed,
    /// A code introduced by a newer peer.
    #[serde(other)]
    Other,
}

/// A refused request. The message is safe to display: hosts never include
/// credentials, raw provider bodies or private paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Rejection {
    /// Machine-readable classification.
    pub code: ErrorCode,
    /// Human-readable explanation and recovery guidance.
    pub message: String,
}

impl Rejection {
    /// A refusal with an explicit classification.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// The request is malformed or scoped to another conversation.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Invalid, message)
    }

    /// Another execution is active.
    pub fn busy(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Busy, message)
    }

    /// The target no longer exists or is not available to this view.
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unavailable, message)
    }

    /// The host refused the request under its policy.
    pub fn refused(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Refused, message)
    }
}

/// Plain text from application code is a policy refusal.
impl From<String> for Rejection {
    fn from(message: String) -> Self {
        Self::refused(message)
    }
}

/// Plain text from application code is a policy refusal.
impl From<&str> for Rejection {
    fn from(message: &str) -> Self {
        Self::refused(message)
    }
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Rejection {}
