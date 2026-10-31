//! A laid-out frame and the click targets it carries. Targets name what they
//! act on, never a screen position alone, so a stale click cannot hit another.

use roomkit_tui_protocol::{ActorId, CallId, ConversationId, RunId};

use super::{SidebarArea, Span};

/// A visible tool disclosure target, scoped to a call rather than its screen row.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolTarget {
    /// Conversation owning the visible call.
    pub conversation: ConversationId,
    /// Visible screen row.
    pub row: usize,
    /// First clickable column, inclusive.
    pub start: usize,
    /// Last clickable column, exclusive.
    pub end: usize,
    /// Tool's owning actor.
    pub actor: ActorId,
    /// Owning execution.
    pub run: RunId,
    /// Call identity within this execution.
    pub call: CallId,
}

/// A visible return-to-latest control, scoped to its conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct BackToBottomTarget {
    /// Conversation displayed when the control was drawn.
    pub conversation: ConversationId,
    /// Visible screen row.
    pub row: usize,
    /// First clickable column, inclusive.
    pub start: usize,
    /// Last clickable column, exclusive.
    pub end: usize,
}

/// A visible application action tied to the view that rendered it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ActionTarget {
    /// Owning conversation.
    pub conversation: ConversationId,
    /// Advertised host command.
    pub command: String,
    /// Opaque host argument.
    pub arguments: String,
    /// Panel identity, or `None` for the background status control.
    pub panel: Option<String>,
    /// Whether this action belongs to the current sidebar.
    pub sidebar: bool,
    /// Visible row.
    pub row: usize,
    /// Inclusive start column.
    pub start: usize,
    /// Exclusive end column.
    pub end: usize,
}

/// A visible choice bound to one exact pending permission.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PermissionTarget {
    /// Conversation which rendered this choice.
    pub conversation: ConversationId,
    /// Exact pending request; stale clicks never authorize the next request.
    pub permission: roomkit_tui_protocol::PermissionId,
    /// One-time answer selected by clicking.
    pub decision: roomkit_tui_protocol::Decision,
    /// Visible row.
    pub row: usize,
    /// Inclusive start column.
    pub start: usize,
    /// Exclusive end column.
    pub end: usize,
}

/// A terminal-sized frame with both plain text and semantic styles.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Frame {
    /// Clickable choices for the exact visible pending permission.
    pub permission_targets: Vec<PermissionTarget>,
    /// Lines without terminal controls, each fitting the requested width.
    pub lines: Vec<String>,
    /// Styled spans with the same text and ordering as `lines`.
    pub spans: Vec<Vec<Span>>,
    /// Composer cursor position as `(column, row)`.
    pub cursor: Option<(u16, u16)>,
    /// Disclosure targets for tools actually present in this viewport.
    pub tool_targets: Vec<ToolTarget>,
    /// Return-to-latest target when browsing history in interactive mouse mode.
    pub back_to_bottom: Option<BackToBottomTarget>,
    /// Host actions actually visible in this frame.
    pub action_targets: Vec<ActionTarget>,
    /// Clamped visible body scroll paired with its panel identity.
    pub panel_scroll: Option<(String, usize)>,
    /// Visible sidebar geometry, absent when hidden or overlaid.
    pub sidebar: Option<SidebarArea>,
    /// Timeline lines actually scrolled back, clamped to the history shown.
    pub scroll: Option<usize>,
    /// Approval detail lines actually scrolled past.
    pub permission_scroll: Option<usize>,
    /// The pending permission whose card this frame shows in full.
    pub approval: Option<roomkit_tui_protocol::PermissionId>,
}
