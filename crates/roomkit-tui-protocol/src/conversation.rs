//! The host's projection of one conversation: participants, settings, history.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    ActorId, CallId, CommandSpec, ContextUsage, ConversationCost, ConversationId, InputAction,
    MessageId, ModeState, ModelInfo, Permission, PermissionDecision, RunId, TokenUsage,
};

/// An identified participant supplied by the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Actor {
    /// Stable identity used in all events.
    pub id: ActorId,
    /// Human-readable name, never used for authorization.
    pub name: String,
}

impl Actor {
    /// A participant and its display name.
    pub fn new(id: impl Into<ActorId>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
        }
    }
}

/// A rendered conversation message. Only the host determines its author.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Message {
    /// Identity used to reconcile streaming and final content.
    pub id: MessageId,
    /// Author identity.
    pub actor: ActorId,
    /// Message body; terminals must treat control characters as untrusted.
    pub text: String,
}

impl Message {
    /// A complete message by one actor.
    pub fn new(
        id: impl Into<MessageId>,
        actor: impl Into<ActorId>,
        text: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            actor: actor.into(),
            text: text.into(),
        }
    }
}

/// Observable tool states; a requested call is not necessarily running.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolState {
    /// Proposed, waiting for scheduling or policy.
    Pending,
    /// Execution has actually begun.
    Running,
    /// The host confirms successful completion.
    Succeeded,
    /// Execution failed, including partial failures.
    Failed,
    /// Execution was refused before it began.
    Denied,
    /// Execution was cancelled; prior effects may remain.
    Cancelled,
    /// A state introduced by a newer host; displayed without a claim.
    #[serde(other)]
    Unknown,
}

impl ToolState {
    /// Whether the host confirmed that the call will not change any more.
    pub fn is_settled(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Denied | Self::Cancelled
        )
    }
}

/// Tool output for a generic or host-registered presentation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ToolCall {
    /// Identity scoped to its run.
    pub id: CallId,
    /// Stable tool name used to select an optional presentation.
    pub name: String,
    /// Actual execution arguments.
    pub arguments: Value,
    /// Optional public description of the intended action, independent of its state.
    /// Plain display text; never an authorization or proof of execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// Latest state confirmed by the host.
    pub state: ToolState,
    /// Structured output, preserved independently from model-facing text.
    pub data: Option<Value>,
    /// Human-readable result or error summary.
    pub text: String,
    /// Actual execution duration if known.
    pub duration_ms: Option<u64>,
}

impl ToolCall {
    /// A call with its arguments and state, before any output.
    pub fn new(
        id: impl Into<CallId>,
        name: impl Into<String>,
        arguments: Value,
        state: ToolState,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
            intent: None,
            state,
            data: None,
            text: String::new(),
            duration_ms: None,
        }
    }

    /// Set the public description of the intended action.
    #[must_use]
    pub fn with_intent(mut self, intent: impl Into<String>) -> Self {
        self.intent = Some(intent.into());
        self
    }

    /// Set the human-readable result or error summary.
    #[must_use]
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self
    }

    /// Set the structured output.
    #[must_use]
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// Set the actual execution duration.
    #[must_use]
    pub fn with_duration_ms(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }
}

/// A model's reasoning as the host hands it. It is shown while the session
/// runs and never stored, so no snapshot holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Thinking {
    /// Identity scoped to its run.
    pub id: CallId,
    /// The agent thinking.
    pub actor: ActorId,
    /// Owning execution.
    pub run: RunId,
    /// What it wrote; terminals must treat control characters as untrusted.
    pub text: String,
}

impl Thinking {
    /// A piece of reasoning by one actor, in one run.
    pub fn new(
        id: impl Into<CallId>,
        actor: impl Into<ActorId>,
        run: impl Into<RunId>,
        text: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            actor: actor.into(),
            run: run.into(),
            text: text.into(),
        }
    }
}

/// An item in the host's ordered conversation projection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Item {
    /// A human decision confirmed by the host, independent of execution success.
    Approval(PermissionDecision),
    /// A complete message.
    Message(Message),
    /// A tool call attributed to one actor and execution.
    Tool {
        /// Executing agent.
        actor: ActorId,
        /// Owning execution, the same identity as its `RunStarted` event.
        run: RunId,
        /// Latest call state.
        call: ToolCall,
    },
    /// A model's reasoning, which a view builds from `Event::ThinkingDelta`.
    Thinking(Thinking),
    /// An item introduced by a newer host. Terminals ignore it.
    #[serde(other)]
    Unknown,
}

impl Item {
    /// Whether both items have the same scoped identity, so that `other`
    /// replaces `self` in a conversation.
    pub fn same_identity(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Approval(a), Self::Approval(b)) => {
                a.permission.id == b.permission.id
                    && a.permission.run == b.permission.run
                    && a.permission.actor == b.permission.actor
            }
            (Self::Thinking(a), Self::Thinking(b)) => a.id == b.id && a.run == b.run,
            (Self::Message(a), Self::Message(b)) => a.id == b.id && a.actor == b.actor,
            (
                Self::Tool { actor, run, call },
                Self::Tool {
                    actor: other_actor,
                    run: other_run,
                    call: other_call,
                },
            ) => actor == other_actor && run == other_run && call.id == other_call.id,
            _ => false,
        }
    }
}

/// The host's authoritative initial or replacement state for one conversation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Snapshot {
    /// Conversation identity.
    pub conversation: ConversationId,
    /// Display title.
    pub title: String,
    /// Participants, including humans and agents.
    pub actors: Vec<Actor>,
    /// Author of messages submitted through this view, supplied by the host.
    /// Presentation only; never grants authorization. Older hosts omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_actor: Option<ActorId>,
    /// Agent receiving ordinary submitted messages.
    pub selected_agent: ActorId,
    /// Effective settings for that agent, when known.
    pub model: Option<ModelInfo>,
    /// Latest request measurement for the selected model, if still applicable.
    #[serde(default)]
    pub context: Option<ContextUsage>,
    /// Consumption of the latest execution, never a session-wide estimate.
    #[serde(default)]
    pub usage: Option<TokenUsage>,
    /// The conversation's cost at its model's list price; absent when the
    /// host states no price. Older hosts omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<ConversationCost>,
    /// What the work related to the conversation cost, such as the tasks its
    /// agents handed over; a host states it with
    /// [`Event::RelatedCostUpdated`](crate::Event::RelatedCostUpdated). Older
    /// hosts omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related_cost: Option<ConversationCost>,
    /// Host commands available in this conversation.
    pub commands: Vec<CommandSpec>,
    /// Optional application-owned composer actions. Older hosts advertise none.
    #[serde(default)]
    pub input_actions: Vec<InputAction>,
    /// The advertised command a terminal invokes with `next` or `previous`
    /// to move between this conversation and the rooms related to it, on
    /// Alt+Right and Alt+Left with an empty composer. Older hosts omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle_command: Option<String>,
    /// Whether the selected agent and host accept image attachments.
    #[serde(default)]
    pub can_attach_images: bool,
    /// Whether a message sent while the selected agent's turn runs is handed
    /// to that turn rather than refused. Older hosts advertise none.
    #[serde(default)]
    pub can_steer: bool,
    /// Whether the host starts a turn for what the conversation holds
    /// (`Command::Answer`). Older hosts advertise none: the view then never cuts
    /// a turn for a message handed to it.
    #[serde(default)]
    pub can_answer: bool,
    /// Optional interaction mode, changed only after host confirmation.
    #[serde(default)]
    pub mode: Option<ModeState>,
    /// Ordered history; the host remains its durable owner.
    pub items: Vec<Item>,
    /// Current execution, if attached while it is already running. A
    /// snapshot states one live run; its pending permissions belong to it.
    pub active_run: Option<RunId>,
    /// Decisions still waiting when this view is opened or restored.
    pub permissions: Vec<Permission>,
    /// Whether the host currently supports interruption.
    pub can_interrupt: bool,
}

impl Snapshot {
    /// An idle conversation with one selected agent and no history. Add the
    /// selected agent to `actors`: terminals reject a snapshot without it.
    pub fn new(
        conversation: impl Into<ConversationId>,
        title: impl Into<String>,
        selected_agent: Actor,
    ) -> Self {
        Self {
            conversation: conversation.into(),
            title: title.into(),
            selected_agent: selected_agent.id.clone(),
            actors: vec![selected_agent],
            local_actor: None,
            model: None,
            context: None,
            usage: None,
            cost: None,
            related_cost: None,
            commands: Vec::new(),
            input_actions: Vec::new(),
            cycle_command: None,
            can_attach_images: false,
            can_steer: false,
            can_answer: false,
            mode: None,
            items: Vec::new(),
            active_run: None,
            permissions: Vec::new(),
            can_interrupt: false,
        }
    }
}
