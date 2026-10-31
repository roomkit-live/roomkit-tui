//! Requests a client sends to the host.

use serde::{Deserialize, Serialize};

use crate::{
    ActorId, ConversationId, Decision, FormValues, ImageMessage, OperationId, PermissionId,
    RequestId, RunId,
};

/// Requests sent to the host. Hosts validate identities and policy themselves.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Command {
    /// Submit ordinary user text.
    Submit {
        /// Agent selected in the current snapshot.
        agent: ActorId,
        /// User text, not a system instruction.
        text: String,
    },
    /// Submit text and images to an attachment-capable host. Image-only messages
    /// are allowed; the host must validate and store images before accepting.
    SubmitImages {
        /// Agent selected in the current snapshot.
        agent: ActorId,
        /// Bounded message data; boxed to keep ordinary control requests small.
        message: Box<ImageMessage>,
    },
    /// Invoke one of the commands advertised by the host.
    Invoke {
        /// Name without its slash.
        name: String,
        /// Remaining input, preserved for host-owned parsing.
        arguments: String,
    },
    /// Submit an advertised host form without adding its values to chat history.
    SubmitForm {
        /// Advertised application command.
        name: String,
        /// Host-issued form identity/revision.
        form: String,
        /// Values to validate; Debug is redacted.
        values: Box<FormValues>,
    },
    /// Request interruption; completion is reported separately.
    Interrupt {
        /// Execution to stop; never inferred from a global active agent.
        run: RunId,
    },
    /// Answer one human decision.
    Decide {
        /// Pending decision identity.
        permission: PermissionId,
        /// Explicit answer.
        decision: Decision,
    },
    /// A command introduced by a newer client. Hosts reply
    /// [`ErrorCode::Unsupported`](crate::ErrorCode::Unsupported).
    #[serde(other)]
    Unknown,
}

/// A request scoped to one conversation and one client correlation ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Request {
    /// Client correlation ID.
    pub id: RequestId,
    /// Stable operation identity. Retries must keep it and the original payload.
    /// Hosts enforce deduplication within their authorization scope.
    pub operation: OperationId,
    /// Target conversation.
    pub conversation: ConversationId,
    /// Requested operation.
    pub command: Command,
}

impl Request {
    /// A correlated request for one conversation.
    pub fn new(
        id: RequestId,
        operation: impl Into<OperationId>,
        conversation: impl Into<ConversationId>,
        command: Command,
    ) -> Self {
        Self {
            id,
            operation: operation.into(),
            conversation: conversation.into(),
            command,
        }
    }
}
