//! One view of a room: its configuration, the application's command policy,
//! and the bounded projection restored from the engine.

mod buffered;
mod requests;
mod restore;
mod serve;
mod steer;
mod turn;

use std::collections::BTreeMap;

use roomkit::model::{ChannelId, EventContent, RoomId};
use roomkit::{ConversationStore, RoomKit};
use roomkit_tui_protocol::limits::MAX_LABEL_BYTES;
use roomkit_tui_protocol::{
    ActorId, CommandSpec, Event, InputAction, ModeState, ModelInfo, Rejection, Request, Snapshot,
    SnapshotError,
};

use crate::ConversationChannel;

/// Host-selected room, agent and authenticated sender for one view.
/// The adapter never obtains these identities from model output.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SessionConfig {
    /// Room to display; created and authorized by the application.
    pub room: RoomId,
    /// Agent that receives ordinary messages.
    pub agent: ChannelId,
    /// Sender identity assigned by the application, never inferred from text.
    pub sender: String,
    /// Optional host-owned display names keyed by stable actor identity.
    pub actor_names: BTreeMap<ActorId, String>,
    /// Optional reason ordinary messages are temporarily unavailable (for setup).
    /// Application commands remain available.
    pub submission_error: Option<String>,
    /// Display title.
    pub title: String,
    /// Effective settings supplied by the host that configured this agent.
    /// The provider must be the one the engine reports, such as
    /// `OpenAIAIProvider` for `roomkit-openai`.
    pub model: Option<ModelInfo>,
    /// Host-confirmed capacity for this configured model, in tokens.
    pub context_capacity: Option<u64>,
    /// Organization scope passed to every RoomKit operation.
    pub organization: Option<String>,
    /// Application commands handled by `run_with_commands`, never by the model.
    pub commands: Vec<CommandSpec>,
    /// Optional prefixes invoking advertised application commands from the composer.
    pub input_actions: Vec<InputAction>,
    /// Host-confirmed image support for this attachment and selected agent.
    pub can_attach_images: bool,
    /// Optional host-owned interaction mode, independent of engine/model state.
    pub mode: Option<ModeState>,
}

impl SessionConfig {
    /// Configure a view. No engine state is changed by this constructor.
    pub fn new(room: RoomId, agent: ChannelId, sender: impl Into<String>) -> Self {
        Self {
            title: room.to_string(),
            room,
            agent,
            sender: sender.into(),
            actor_names: BTreeMap::new(),
            submission_error: None,
            model: None,
            context_capacity: None,
            organization: None,
            commands: Vec::new(),
            input_actions: Vec::new(),
            can_attach_images: false,
            mode: None,
        }
    }

    pub(crate) fn conversation(&self) -> &str {
        self.room.as_str()
    }

    fn advertises(&self, command: &str) -> bool {
        self.commands.iter().any(|spec| spec.name == command)
    }

    /// Keep the mode or title a host event confirms for this view, so that
    /// later snapshots and handlers see it. A change the terminal ignores is
    /// not kept either: a snapshot carrying a mode that does not fit would be
    /// refused whole.
    fn record(&mut self, event: &Event) {
        match event {
            Event::ModeChanged { conversation, mode }
                if conversation.as_str() == self.conversation() && mode.fits(&self.commands) =>
            {
                self.mode = Some(mode.clone());
            }
            Event::TitleChanged {
                conversation,
                title,
            } if conversation.as_str() == self.conversation() && title.len() <= MAX_LABEL_BYTES => {
                self.title.clone_from(title);
            }
            _ => {}
        }
    }
}

/// Result of an application command handled while the engine is idle.
#[derive(Debug)]
#[non_exhaustive]
pub enum CommandResult {
    /// Publish application-owned selectors or notices after acknowledging the request.
    /// A mode or title change for this view also updates its configuration,
    /// unless a terminal would ignore it.
    Events(Vec<Event>),
    /// Submit host-prepared text to the current agent through the normal engine
    /// path, preserving validation, operation identity and cancellation.
    Submit(String),
    /// Run application-owned work while keeping controls and interruption live.
    Execute(crate::CommandExecution),
    /// Validate a replacement attachment, then atomically replace the view.
    /// A replacement whose snapshot a terminal would refuse is rejected as
    /// invalid and the current view is kept.
    Switch(Box<SessionConfig>),
}

/// Application command policy and storage remain outside the generic adapter.
///
/// Its methods return futures the view awaits without blocking its events.
/// Unlike [`crate::SessionLifecycle`], which is shared as a trait object,
/// a handler is owned by one view, so its futures need no boxing.
pub trait CommandHandler: Send + Sync {
    /// Validate and persist uploaded images, returning room content with opaque
    /// host references. Runs only after scope, agent and transport bounds pass.
    /// Defaults to rejecting attachments; errors must be safe for the terminal.
    fn prepare_images(
        &self,
        _text: &str,
        _images: &[roomkit_tui_protocol::ImageUpload],
        _session: &SessionConfig,
    ) -> impl std::future::Future<Output = Result<EventContent, Rejection>> + Send {
        async {
            Err(Rejection::new(
                roomkit_tui_protocol::ErrorCode::Unsupported,
                "This host does not accept image attachments",
            ))
        }
    }

    /// Subscribe to coalesced application state for this attachment. The host
    /// owns scoping and must stop observation when the receiver is dropped.
    /// Values describe current state, not durable/transient model events.
    /// A mode or title change for this view also updates its configuration,
    /// so later snapshots and handlers see it, unless a terminal would ignore it.
    fn subscribe(
        &self,
        _session: SessionConfig,
    ) -> Option<tokio::sync::watch::Receiver<Vec<Event>>> {
        None
    }

    /// Allow this particular advertised control command during execution.
    /// Session switching is still refused while busy; rename the view with
    /// [`Event::TitleChanged`] instead. Defaults to false.
    fn allow_while_busy(&self, _name: &str) -> bool {
        false
    }

    /// Handle an advertised command in the current conversation. Errors must be
    /// safe to display; the adapter rejects busy and incorrectly scoped requests
    /// before calling this handler, except explicitly opted-in busy controls.
    fn handle(
        &self,
        request: Request,
        session: SessionConfig,
    ) -> impl std::future::Future<Output = Result<CommandResult, Rejection>> + Send;
}

struct NoCommands;

impl CommandHandler for NoCommands {
    async fn handle(&self, _: Request, _: SessionConfig) -> Result<CommandResult, Rejection> {
        Err(Rejection::new(
            roomkit_tui_protocol::ErrorCode::Unsupported,
            "No application command handler was registered",
        ))
    }
}

/// Failures of the client attachment, separate from a model/tool failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AdapterError {
    /// The caller may inspect the source locally; never send it unredacted to a view.
    #[error("RoomKit operation failed")]
    Engine(#[from] roomkit::RoomKitError),
    /// The host selected an invalid or inaccessible binding.
    #[error("The configured conversation or agent is not available to this view")]
    InvalidBinding,
    /// The view's snapshot would be refused by a terminal. The cause is
    /// usually the configuration, such as a command with a space in its name,
    /// but can be the room, such as more actors than a view shows. Nothing was
    /// sent to the view.
    #[error("This view would be refused by the terminal: {0}")]
    InvalidSnapshot(#[from] SnapshotError),
    /// The initial adapter only restores quiescent conversations.
    #[error("Wait for the active response to finish before attaching this view")]
    Busy,
    /// A bounded event queue overflowed; state must be reloaded.
    #[error("The view fell behind; reattach to reload the conversation")]
    Lagged,
    /// The view was detached; execution remains owned by the application.
    #[error("The client disconnected")]
    Disconnected,
}

/// Local bridge to an existing kit; independent of the terminal library.
///
/// It serves one selected agent, messages and restoration of an idle
/// conversation. Applications can register commands and switch idle views
/// through `run_with_commands`. Provider metadata supplies context
/// measurements; scoped interruption uses the active delivery handle.
/// Approval brokerage is opt-in through the host's tool hooks. It never
/// closes a shared engine.
#[derive(Debug)]
pub struct LocalSession<S: ConversationStore> {
    kit: RoomKit<S>,
    channel: ConversationChannel,
    config: SessionConfig,
    lifecycle: Option<std::sync::Arc<dyn crate::SessionLifecycle>>,
}

impl<S: ConversationStore> LocalSession<S> {
    /// Bind an application-configured kit to its registered transport channel.
    pub fn new(kit: RoomKit<S>, channel: ConversationChannel, config: SessionConfig) -> Self {
        Self {
            kit,
            channel,
            config,
            lifecycle: None,
        }
    }

    /// Install application callbacks for admission and complete logical turns.
    /// The application, not this view, owns session shutdown and hook policy.
    #[must_use]
    pub fn with_lifecycle(
        mut self,
        lifecycle: std::sync::Arc<dyn crate::SessionLifecycle>,
    ) -> Self {
        self.lifecycle = Some(lifecycle);
        self
    }

    /// Reload a bounded projection from the engine, respecting visibility and
    /// read access. The engine remains the only durable conversation store.
    /// Fails with [`AdapterError::InvalidSnapshot`] when a terminal would
    /// refuse the result, so attaching and switching never send one.
    pub async fn snapshot(&self) -> Result<Snapshot, AdapterError> {
        if self.channel.busy(self.config.conversation()) {
            return Err(AdapterError::Busy);
        }
        let snapshot = restore::snapshot(&self.kit, &self.channel, &self.config).await?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    /// Stores the decisions this view's room collected, for work that runs
    /// outside a turn; `false` when they could not be stored.
    pub(crate) fn history_writer(&self) -> crate::execution::Persist {
        let (kit, channel) = (self.kit.clone(), self.channel.clone());
        let config = self.config.clone();
        Box::new(move || {
            Box::pin(async move {
                crate::approval_history::persist(&kit, &channel, &config)
                    .await
                    .is_ok()
            })
        })
    }

    /// Serve one bounded connection until it detaches or needs resynchronizing.
    /// Input acceptance is not proof of commit or execution success. Work already
    /// accepted runs independently of this future and of the client's lifetime.
    pub async fn run(self, host: roomkit_tui_transport::Host) -> Result<(), AdapterError> {
        self.run_with_commands(host, NoCommands).await
    }

    /// Serve the view with application-owned commands and idle view switching.
    /// A failed replacement leaves the original attachment intact. Commands
    /// never enter model history or implicitly interrupt an accepted execution.
    pub async fn run_with_commands(
        self,
        host: roomkit_tui_transport::Host,
        handler: impl CommandHandler,
    ) -> Result<(), AdapterError> {
        serve::serve(self, host, &handler).await
    }
}
