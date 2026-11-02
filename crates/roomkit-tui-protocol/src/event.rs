//! Events a host publishes to its clients.

use serde::{Deserialize, Serialize};

use crate::{
    ActorId, BackgroundStatus, Choice, ContextUsage, ConversationId, FormSpec, Item, Message,
    MessageId, ModeState, ModelInfo, OperationId, PanelSpec, Permission, PermissionId, Rejection,
    RequestId, RunId, SidebarSpec, Snapshot, TokenUsage,
};

/// How an execution actually ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RunOutcome {
    /// Normal completion.
    Completed,
    /// Interrupted with partial effects possibly retained.
    Interrupted,
    /// Failed; details are supplied by notices or tool results.
    Failed,
    /// An outcome introduced by a newer host, displayed without claiming success.
    #[serde(other)]
    Unknown,
}

/// Host events. Event delivery must preserve order within a connection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[allow(
    clippy::large_enum_variant,
    reason = "Keep bootstrap snapshots inline in the bounded event queue; streamed payloads already own their allocations"
)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Event {
    /// Replace optional information beside the conversation. A snapshot clears
    /// this projection; hosts resend it after attaching or switching views.
    SidebarChanged {
        /// View in which the sidebar is valid.
        conversation: ConversationId,
        /// None removes the sidebar, including its navigation actions.
        sidebar: Option<SidebarSpec>,
    },
    /// An accepted operation is being processed, possibly before a run exists.
    /// Hosts opt into this lifecycle and pair each start with a finish, including
    /// admission failures and operations that produce no model execution.
    OperationStarted {
        /// Conversation in which the operation was accepted.
        conversation: ConversationId,
        /// Stable identity from the accepted request, not its connection-local ID.
        operation: OperationId,
    },
    /// Processing and cleanup of an accepted operation have settled.
    /// This covers admission and delivery; individual runs retain their own
    /// outcomes. A completed operation does not override a failed run.
    OperationFinished {
        /// Conversation in which the operation was accepted.
        conversation: ConversationId,
        /// Identity of the matching accepted operation.
        operation: OperationId,
        /// Outcome of handling the operation, independent of its run outcomes.
        outcome: RunOutcome,
    },
    /// Replace a quiet, application-owned welcome caption (for example a version).
    ApplicationInfo {
        /// View in which the caption is valid.
        conversation: ConversationId,
        /// Plain text, at most 256 bytes; None clears the caption.
        text: Option<String>,
    },
    /// Rename the displayed conversation in place, even while it executes.
    /// The history, the draft and any open control are kept; snapshots the
    /// host sends afterwards carry this title.
    TitleChanged {
        /// Conversation being renamed; other views ignore the event.
        conversation: ConversationId,
        /// Plain text, at most 256 bytes. A longer title is ignored and the
        /// current one kept, where a snapshot's title would be shortened.
        title: String,
    },
    /// Replace the host's statement of what exiting stops, such as running
    /// processes. While it is set, detaching also asks for confirmation.
    ///
    /// Send it before the events that announce that work, such as a
    /// background status, and clear it only once the work has stopped. The
    /// terminal reads keys between host events, so a Ctrl+D read after the
    /// announcement but before the warning detaches at once.
    ExitWarning {
        /// View in which the warning is valid.
        conversation: ConversationId,
        /// Plain text, at most 256 bytes; None when exiting stops nothing.
        text: Option<String>,
    },
    /// Replace a scoped, application-owned background status control.
    BackgroundChanged {
        /// Current view conversation, not the scope of an underlying job.
        conversation: ConversationId,
        /// None hides the control.
        status: Option<BackgroundStatus>,
    },
    /// Open a read-only information and action panel.
    Panel {
        /// View in which the panel is valid.
        conversation: ConversationId,
        /// Host-owned panel definition.
        panel: PanelSpec,
    },
    /// Refresh only an already open panel with the same opaque identity.
    PanelUpdated {
        /// View in which the update is valid.
        conversation: ConversationId,
        /// New contents and actions for the existing panel.
        panel: PanelSpec,
    },
    /// Replace the visible conversation from the durable host projection.
    Snapshot(Snapshot),
    /// Confirm the effective interaction mode without replacing the transcript.
    ModeChanged {
        /// Conversation whose policy changed.
        conversation: ConversationId,
        /// Authoritative mode selection and available choices.
        mode: ModeState,
    },
    /// Open a filterable selector for an advertised application command.
    /// Choosing an entry invokes that command with the entry's exact value.
    /// Dismissing the selector has no application side effect.
    Choices {
        /// Conversation in which these choices are valid.
        conversation: ConversationId,
        /// Advertised command receiving the selected argument.
        command: String,
        /// User-facing selector title.
        title: String,
        /// Host-ordered choices; the view bounds its visible catalogue.
        choices: Vec<Choice>,
    },
    /// Open an application form. Cancellation is local and has no side effects.
    Form {
        /// Conversation in which the form is valid.
        conversation: ConversationId,
        /// Host-owned definition; never include credentials in initial values.
        form: FormSpec,
    },
    /// A request was accepted or rejected; it does not announce run completion.
    Reply {
        /// Correlated client request.
        request: RequestId,
        /// Why the request was refused; `None` means accepted.
        error: Option<Rejection>,
    },
    /// Append text to an identified in-progress message.
    TextDelta {
        /// Target conversation.
        conversation: ConversationId,
        /// Message to extend or create.
        message: MessageId,
        /// Author.
        actor: ActorId,
        /// Owning execution.
        run: RunId,
        /// New text only.
        text: String,
    },
    /// Replace a streamed draft with its durable host-assigned message identity.
    MessageFinalized {
        /// Target conversation.
        conversation: ConversationId,
        /// Temporary message identity used by the preceding deltas.
        draft: MessageId,
        /// Stored message after the host applied its hooks and policies.
        message: Message,
    },
    /// Insert or replace an item by its scoped identity.
    Item {
        /// Target conversation.
        conversation: ConversationId,
        /// Authoritative content.
        item: Item,
    },
    /// An execution has started. Several can be live at once in one
    /// conversation, such as host work and an agent turn for another
    /// channel's message; each lasts until its own `RunFinished`.
    RunStarted {
        /// Target conversation.
        conversation: ConversationId,
        /// New execution identity.
        run: RunId,
        /// The message the execution answers, when it answers one. A message
        /// handed to a running turn gets a turn of its own when that turn no
        /// longer reads it. Older hosts send none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answers: Option<MessageId>,
    },
    /// An execution has actually stopped.
    RunFinished {
        /// Target conversation.
        conversation: ConversationId,
        /// Completed execution identity.
        run: RunId,
        /// Actual outcome.
        outcome: RunOutcome,
    },
    /// The host needs a human decision.
    PermissionRequested {
        /// Target conversation.
        conversation: ConversationId,
        /// Operation to approve or refuse.
        permission: Permission,
    },
    /// A permission was settled or withdrawn by the host.
    PermissionResolved {
        /// Target conversation.
        conversation: ConversationId,
        /// Settled decision.
        permission: PermissionId,
    },
    /// The host confirmed a model change; stale context must be cleared.
    ModelChanged {
        /// Target conversation.
        conversation: ConversationId,
        /// New effective model.
        model: ModelInfo,
    },
    /// Per-request context, associated with its actual model and execution.
    ContextUpdated {
        /// Target conversation.
        conversation: ConversationId,
        /// Owning execution.
        run: RunId,
        /// Provider for this measurement.
        provider: String,
        /// Model for this measurement.
        model: String,
        /// Context measurement.
        usage: ContextUsage,
    },
    /// Replace measurements for one execution and its actual model.
    UsageUpdated {
        /// Target conversation.
        conversation: ConversationId,
        /// Owning execution.
        run: RunId,
        /// Provider identity.
        provider: String,
        /// Configured model identity.
        model: String,
        /// Last generation's measured occupancy; absent when it was not measured.
        context: Option<ContextUsage>,
        /// Execution consumption, replacing any earlier report for this run.
        usage: TokenUsage,
    },
    /// The host handed a message to a run already under way, which reads it
    /// at its next step instead of starting a turn of its own. Should the run
    /// no longer read it, a later `RunStarted` names the message in `answers`.
    MessageSteered {
        /// Target conversation.
        conversation: ConversationId,
        /// The message handed over.
        message: MessageId,
        /// The run it was handed to.
        run: RunId,
    },
    /// A host diagnostic suitable for the user, with credentials removed.
    Notice {
        /// View in which the diagnostic is relevant.
        conversation: ConversationId,
        /// Human-readable diagnostic.
        text: String,
    },
    /// An event introduced by a newer host. Terminals ignore it.
    #[serde(other)]
    Unknown,
}
