//! Passive, bounded observation without a runtime or application callbacks.

use std::sync::{Arc, Mutex};

use roomkit_tui_protocol::{
    ActorId, ConversationId, ModeState, OperationId, PermissionId, RunId, RunOutcome,
};

/// The latest accepted operation, including admission before a run exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationObservation {
    /// Identity supplied by the host's operation lifecycle.
    pub id: OperationId,
    /// None while processing; the operation's confirmed outcome after cleanup.
    pub outcome: Option<RunOutcome>,
}

/// The latest execution observed in the displayed conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunObservation {
    /// Host-owned execution identity.
    pub id: RunId,
    /// None until the host confirms that this execution ended.
    pub outcome: Option<RunOutcome>,
}

/// Host-confirmed facts about the displayed session, without transcript content.
/// Operation handling and run outcomes remain separate; presentation and any
/// interpretation of these facts belong to the consuming application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionObservation {
    /// Conversation currently displayed.
    pub conversation: ConversationId,
    /// Display title provided by the host.
    pub title: String,
    /// Participant currently addressed by ordinary submissions.
    pub selected_agent: ActorId,
    /// Opaque interaction mode selected by the host.
    pub mode: Option<ModeState>,
    /// Latest operation when the host supplies its optional lifecycle.
    pub operation: Option<OperationObservation>,
    /// Latest execution, including an active run restored by a snapshot.
    pub run: Option<RunObservation>,
    /// Decisions still open, including answers awaiting host confirmation.
    pub permissions: Vec<PermissionId>,
}

/// A read-only handle to the latest session observation.
///
/// Create explicitly and attach to one session or terminal. Poll with [`Self::latest`]
/// at the consumer's chosen cadence. There is no worker, I/O or event backlog:
/// unread values are replaced. This is current state, not an audit stream.
/// The final observation remains available after the session closes; the host
/// owns the lifetime of both the session and its consumers.
///
/// ```
/// use roomkit_tui::{Session, SessionObserver};
///
/// let observer = SessionObserver::default();
/// let session = Session::default().with_observer(observer.clone());
/// // Apply host events to the session; a separate consumer can read this handle.
/// assert!(observer.latest().is_none()); // No snapshot has arrived yet.
/// ```
#[derive(Clone, Debug, Default)]
pub struct SessionObserver(Arc<Mutex<Option<SessionObservation>>>);

impl SessionObserver {
    /// Read the latest facts, or None before the first valid session snapshot.
    pub fn latest(&self) -> Option<SessionObservation> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn publish(&self, observation: Option<SessionObservation>) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = observation;
    }
}
