//! Pure UI state. Model settings and execution completion are host-confirmed.

mod actions;
mod bounds;
mod decisions;
mod events;
mod execution;
mod history;
mod input;
mod labels;
mod measures;
mod runs;
mod send_now;
mod steering;
mod tools;

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;
use std::time::Duration;

use roomkit_tui_protocol::{
    ActorId, BackgroundStatus, CallId, Command, ContextUsage, ConversationId, ImageUpload,
    MessageId, Permission, PermissionId, RequestId, RunId, RunOutcome, Snapshot, TokenUsage,
};

pub(crate) use bounds::bounded;
pub use input::SessionError;
pub use steering::Steering;

use crate::{
    ChoicePicker, CommandMenu, FormState, OperationObservation, PanelState, SessionObserver,
    SidebarState, editor::Editor,
};

type ToolKey = (ActorId, RunId, CallId);

/// How long a new approval card ignores decisions, so that keys typed for the
/// composer, or a click aimed elsewhere, never answer a request unseen.
pub const APPROVAL_ARMING_DELAY: Duration = Duration::from_millis(500);

#[derive(Debug)]
struct ObservedRun {
    id: RunId,
    started_at: Option<Duration>,
    outcome: Option<RunOutcome>,
    duration: Option<Duration>,
}

/// Limits on the terminal's projection, not on the host's durable history.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Limits {
    /// Maximum retained timeline items.
    pub items: NonZeroUsize,
    /// Maximum retained text bytes per message or output.
    pub text_bytes: NonZeroUsize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            items: NonZeroUsize::new(500).unwrap_or(NonZeroUsize::MIN),
            text_bytes: NonZeroUsize::new(roomkit_tui_protocol::limits::MAX_TEXT_BYTES)
                .unwrap_or(NonZeroUsize::MIN),
        }
    }
}

/// The part of the view that receives the keyboard. Exactly one has focus;
/// earlier variants take precedence over later ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Focus {
    /// The oldest pending human decision.
    Permission,
    /// A read-only host panel.
    Panel,
    /// A host form.
    Form,
    /// A host choice selector.
    Picker,
    /// Sidebar navigation, after F8.
    Sidebar,
    /// The message composer and its completion menu.
    Composer,
}

/// Pure UI state. Model settings and execution completion are host-confirmed.
///
/// Drive it with [`Session::apply`] and read it through its accessors; the
/// interactive terminal is one such driver, a headless test is another.
#[derive(Debug)]
pub struct Session {
    pub(crate) sidebar: Option<SidebarState>,
    pub(crate) sidebar_visible: Option<bool>,
    pub(crate) editor: Editor,
    pub(crate) images: Vec<ImageUpload>,
    pub(crate) draft_epoch: u64,
    pub(crate) menu: CommandMenu,
    pub(crate) picker: Option<ChoicePicker>,
    pub(crate) application_info: Option<String>,
    pub(crate) background: Option<BackgroundStatus>,
    pub(crate) exit_warning: Option<String>,
    pub(crate) panel: Option<PanelState>,
    pub(crate) form: Option<FormState>,
    pub(crate) command_prefix: char,
    pub(crate) snapshot: Option<Snapshot>,
    pub(crate) notice: String,
    pub(crate) quit_until: Option<Duration>,
    /// Key whose second press confirms the armed exit.
    pub(crate) quit_key: &'static str,
    pub(crate) expanded: bool,
    pub(crate) scroll: usize,
    pub(crate) permission_scroll: usize,
    pub(crate) permission_choice: usize,
    /// The head permission's card and when a frame first showed it.
    pub(crate) approval_drawn: Option<(PermissionId, Duration)>,
    previous_context: Option<ContextUsage>,
    limits: Limits,
    next_request: u64,
    pending: BTreeMap<RequestId, Command>,
    last_run: Option<RunId>,
    /// Runs started and not finished, earliest first.
    live_runs: Vec<runs::LiveRun>,
    /// The most severe outcome of the runs that finished while another ran
    /// on, since the room was last idle.
    held_outcome: Option<RunOutcome>,
    /// Messages sent while a turn ran, oldest first, and where they went.
    steered: Vec<(MessageId, Steering)>,
    /// Messages a run was started for, oldest first: one handed to a turn
    /// whose own run started before the host said so went to that run.
    answered: Vec<MessageId>,
    interrupting: Option<RunId>,
    /// The message last handed to a turn, and that turn's run.
    handed: Option<(MessageId, RunId)>,
    /// The draft that leaves once the interrupted turn stops (Ctrl+S).
    waiting: Option<send_now::Waiting>,
    /// The waited draft's request and text, until the host answers it.
    flight: Option<(RequestId, String)>,
    deciding: BTreeSet<PermissionId>,
    expanded_tools: BTreeSet<ToolKey>,
    focused_tool: Option<ToolKey>,
    observed_run: Option<ObservedRun>,
    operation: Option<OperationObservation>,
    observer: Option<SessionObserver>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new(Limits::default())
    }
}

impl Session {
    /// Create disconnected UI state with explicit projection limits.
    pub fn new(limits: Limits) -> Self {
        Self {
            sidebar: None,
            sidebar_visible: None,
            editor: Editor::default(),
            images: Vec::new(),
            draft_epoch: 0,
            menu: CommandMenu::default(),
            picker: None,
            application_info: None,
            background: None,
            exit_warning: None,
            panel: None,
            form: None,
            command_prefix: '/',
            snapshot: None,
            notice: String::new(),
            quit_until: None,
            quit_key: "Ctrl+C",
            expanded: false,
            scroll: 0,
            permission_scroll: 0,
            permission_choice: 0,
            approval_drawn: None,
            previous_context: None,
            limits,
            next_request: 0,
            pending: BTreeMap::new(),
            last_run: None,
            live_runs: Vec::new(),
            held_outcome: None,
            steered: Vec::new(),
            answered: Vec::new(),
            interrupting: None,
            handed: None,
            waiting: None,
            flight: None,
            deciding: BTreeSet::new(),
            expanded_tools: BTreeSet::new(),
            focused_tool: None,
            observed_run: None,
            operation: None,
            observer: None,
        }
    }

    /// Use another prefix to invoke host commands, normally `/` or `$`.
    #[must_use]
    pub fn with_command_prefix(mut self, prefix: char) -> Self {
        self.command_prefix = prefix;
        self
    }

    /// Attach an optional, passive observer. Existing state is published immediately.
    /// The handle retains only the latest value and never calls application code.
    #[must_use]
    pub fn with_observer(mut self, observer: SessionObserver) -> Self {
        observer.publish(self.observation());
        self.observer = Some(observer);
        self
    }

    /// The part of the view that receives the keyboard.
    pub fn focus(&self) -> Focus {
        if !self.permissions().is_empty() {
            Focus::Permission
        } else if self.panel.is_some() {
            Focus::Panel
        } else if self.form.is_some() {
            Focus::Form
        } else if self.picker.is_some() {
            Focus::Picker
        } else if self.sidebar.as_ref().is_some_and(|s| s.focused) {
            Focus::Sidebar
        } else {
            Focus::Composer
        }
    }

    /// Current authoritative conversation projection.
    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot.as_ref()
    }

    /// Outstanding human decisions in delivery order.
    pub fn permissions(&self) -> &[Permission] {
        self.snapshot
            .as_ref()
            .map_or(&[], |snapshot| snapshot.permissions.as_slice())
    }

    /// Most recent request occupancy for the current model.
    pub fn context(&self) -> Option<&ContextUsage> {
        self.snapshot.as_ref()?.context.as_ref()
    }

    /// Provider-reported consumption of the latest execution.
    pub fn usage(&self) -> Option<&TokenUsage> {
        self.snapshot.as_ref()?.usage.as_ref()
    }

    /// The current bounded, user-visible diagnostic; empty when there is none.
    pub fn notice(&self) -> &str {
        &self.notice
    }

    /// The composer's text.
    pub fn draft(&self) -> &str {
        self.editor.text()
    }

    /// Replace the composer's text and place the caret at its end.
    pub fn set_draft(&mut self, text: impl Into<String>) {
        self.editor.set(text.into());
        self.refresh_menu();
    }

    /// Insert text at the caret, as typing or pasting would.
    pub fn insert(&mut self, text: &str) {
        self.editor.insert(text);
        self.refresh_menu();
    }

    /// Images waiting in the composer; bytes are sent only on explicit submission.
    pub fn images(&self) -> &[ImageUpload] {
        &self.images
    }

    /// Host-supplied command and argument completions beside the composer.
    pub fn menu(&self) -> &CommandMenu {
        &self.menu
    }

    /// The host's open choice selector.
    pub fn picker(&self) -> Option<&ChoicePicker> {
        self.picker.as_ref()
    }

    /// The host's open read-only panel.
    pub fn panel(&self) -> Option<&PanelState> {
        self.panel.as_ref()
    }

    /// The host's open form.
    pub fn form(&self) -> Option<&FormState> {
        self.form.as_ref()
    }

    /// The host's sidebar, whether or not it is visible.
    pub fn sidebar(&self) -> Option<&SidebarState> {
        self.sidebar.as_ref()
    }

    /// The host's caption on the welcome screen.
    pub fn application_info(&self) -> Option<&str> {
        self.application_info.as_deref()
    }

    /// What exiting stops, as stated by the host; detaching then asks first.
    pub fn exit_warning(&self) -> Option<&str> {
        self.exit_warning.as_deref()
    }

    /// The host's background work status.
    pub fn background(&self) -> Option<&BackgroundStatus> {
        self.background.as_ref()
    }

    /// Whether an interruption has been submitted but not settled.
    pub fn interrupting(&self) -> bool {
        self.interrupting.is_some()
    }

    /// Whether an answer to this permission is awaiting host settlement.
    pub fn deciding(&self, id: &PermissionId) -> bool {
        self.deciding.contains(id)
    }

    /// Whether the oldest pending decision accepts answers at `now`: a frame
    /// synchronized through [`Self::sync`] has shown its card for at least
    /// [`APPROVAL_ARMING_DELAY`]. A card never drawn is never armed.
    pub fn approval_armed(&self, now: Duration) -> bool {
        let Some(head) = self.permissions().first() else {
            return true;
        };
        self.approval_drawn.as_ref().is_some_and(|(id, drawn)| {
            *id == head.id && now.saturating_sub(*drawn) >= APPROVAL_ARMING_DELAY
        })
    }

    /// Resolve a name for display, falling back to the opaque identity.
    pub fn actor_name<'a>(&'a self, id: &'a ActorId) -> &'a str {
        self.snapshot
            .as_ref()
            .and_then(|s| s.actors.iter().find(|a| &a.id == id))
            .map_or(id.as_str(), |a| a.name.as_str())
    }

    pub(crate) fn expire_quit_hint(&mut self, now: Duration) -> bool {
        if self.quit_until.is_some_and(|deadline| now >= deadline) {
            self.quit_until = None;
            return true;
        }
        false
    }

    fn advertises(&self, command: &str) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|s| s.commands.iter().any(|c| c.name == command))
    }

    pub(crate) fn is_current(&self, id: &ConversationId) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| &snapshot.conversation == id)
    }
}
