//! Application-owned branding and semantic terminal colors.

mod theme;

pub use theme::{Color, Theme};

/// Whether the terminal emits color sequences.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ColorMode {
    /// Honor a nonempty `NO_COLOR` and `TERM=dumb` when starting the terminal.
    /// Headless rendering reads no environment and draws this like `Always`.
    #[default]
    Auto,
    /// Use the application's palette regardless of environment preferences.
    Always,
    /// Keep labels and emphasis, without foreground or background colors;
    /// sidebar meter parts are told apart by distinct glyphs instead.
    Never,
}

/// Ownership of mouse text selection and clipboard integration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum SelectionMode {
    /// Leave selection and copy shortcuts to the terminal emulator.
    Native,
    /// Enable tool clicks and request clipboard writes on mouse selection release.
    /// The terminal must support and permit OSC 52; no clipboard contents are read.
    #[default]
    CopyOnSelect,
}

/// Plain-text application identity. Empty logo lines hide the mark entirely.
/// Text is sanitized and clipped by the renderer, just like host messages.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Branding {
    /// Application name shown in the header.
    pub name: String,
    /// Optional short subtitle, shown when space permits.
    pub subtitle: String,
    /// Small terminal mark; at most five lines are displayed on roomy terminals.
    pub logo: Vec<String>,
}

impl Branding {
    /// An application name without subtitle or logo.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            subtitle: String::new(),
            logo: Vec::new(),
        }
    }

    /// Set the short subtitle.
    #[must_use]
    pub fn with_subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = subtitle.into();
        self
    }

    /// Set the terminal mark; at most five lines are displayed.
    #[must_use]
    pub fn with_logo(mut self, logo: Vec<String>) -> Self {
        self.logo = logo;
        self
    }
}

impl Default for Branding {
    fn default() -> Self {
        Self {
            name: "RoomKit".into(),
            subtitle: "A space for conversation".into(),
            logo: vec![
                "╭───────╮".into(),
                "│ ▀▀ ▀▀ │".into(),
                "│ ▄▄ ▄▄ │".into(),
                "╰───────╯".into(),
            ],
        }
    }
}

/// Optional centered composer for an empty, idle conversation.
/// Artwork is plain terminal text, independently configurable by each host.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct WelcomeScreen {
    /// Symbol beside the wordmark; empty hides it. At most eight rows/24 columns.
    pub logo: Vec<String>,
    /// Large lettering; empty uses the application's plain name.
    /// At most eight rows/64 columns are displayed.
    pub wordmark: Vec<String>,
    /// Preferred composer width, clamped to 48–100 display columns.
    pub width: u16,
}

impl Default for WelcomeScreen {
    fn default() -> Self {
        Self {
            logo: Vec::new(),
            wordmark: Vec::new(),
            width: 88,
        }
    }
}

/// Terminal presentation and optional reporting selected by the application,
/// independently of engine configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct UiOptions {
    /// Optional welcome layout for empty, idle conversations. None keeps the timeline view.
    pub welcome: Option<WelcomeScreen>,
    /// Blank rows below the interface, capped at three and removed on tiny terminals.
    pub bottom_margin: u16,
    /// Initial sidebar visibility when the host supplies one; F7 toggles locally.
    pub sidebar_visible: bool,
    /// Preferred sidebar width, clamped to 28–48 columns.
    pub sidebar_width: u16,
    /// Minimum terminal width for a sidebar, with at least 72 conversation columns.
    pub sidebar_min_width: u16,
    /// Optional OSC 7501 reporting, disabled by default and for TERM=dumb.
    /// Interactive entry points write on the terminal thread; pure rendering
    /// never emits status. The application supplies its own stable identity.
    pub terminal_status: Option<crate::TerminalStatus>,
    /// Semantic color palette; every field may be overridden.
    pub theme: Theme,
    /// Application name and terminal logo.
    pub branding: Branding,
    /// Runtime color policy.
    pub color: ColorMode,
    /// Animate active runs and tools. False uses stable activity glyphs.
    pub animations: bool,
    /// Mouse selection and copy behavior.
    pub selection: SelectionMode,
    /// Prefix opening host commands, normally `/` or `$`.
    pub command_prefix: char,
    /// Blank lines between activity and composer, reduced to fit tiny terminals.
    pub activity_gap: u16,
    /// Minimum editable rows in the filled composer, clamped to one through five.
    /// The composer grows with the draft and shrinks to fit small terminals.
    pub input_min_lines: u16,
    /// Filled rows above and below the draft, capped at two and reduced to fit.
    pub input_padding: u16,
    /// Optional hint shown only while the draft is empty; never submitted.
    pub input_placeholder: String,
    /// Short marker at the start of the draft, clipped to four display columns.
    /// An empty string hides it. Plain text only, never terminal controls.
    pub input_prompt: String,
    /// Render message bodies as Markdown. False displays their original syntax.
    pub markdown: bool,
    /// Show the host-identified local author's messages on the input surface,
    /// without an author heading. Other participants retain their names.
    pub highlight_own_messages: bool,
    /// Blank lines between transcript items (capped at three).
    pub message_gap: u16,
    /// Visible detail rows per collapsed tool or activity, clamped to one through eight.
    pub tool_preview_lines: usize,
    /// Replace consecutive calls by one activity heading and its current tool's
    /// preview. F2 reveals the technical timeline. False keeps individual cards.
    pub tool_activity: bool,
}

impl Default for UiOptions {
    fn default() -> Self {
        Self {
            welcome: None,
            bottom_margin: 0,
            sidebar_visible: true,
            sidebar_width: 34,
            sidebar_min_width: 112,
            terminal_status: None,
            theme: Theme::default(),
            branding: Branding::default(),
            color: ColorMode::Auto,
            animations: true,
            selection: SelectionMode::CopyOnSelect,
            command_prefix: '/',
            activity_gap: 1,
            input_min_lines: 1,
            input_padding: 1,
            input_placeholder: String::new(),
            input_prompt: "›".into(),
            markdown: true,
            highlight_own_messages: false,
            message_gap: 1,
            tool_preview_lines: 4,
            tool_activity: false,
        }
    }
}
