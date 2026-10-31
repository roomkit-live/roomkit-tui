//! Host-owned conversations, commands and events for a reusable terminal.
//!
//! This crate performs no I/O and owns no execution loop, provider or tool.
//! Hosts supply capabilities and authoritative state. A terminal projects that
//! state and sends correlated requests; acceptance never implies completion.
//!
//! Every public type is `#[non_exhaustive]`: build structs with their
//! constructors and match enums with a wildcard arm. Each serialized enum that
//! flows to a terminal has an `Unknown` variant (or a neutral default), so an
//! older peer ignores what a newer one adds instead of failing to parse.

mod commands;
mod conversation;
mod event;
mod forms;
mod ids;
mod images;
pub mod limits;
mod measures;
mod panels;
mod permissions;
mod rejection;
mod request;
mod sidebar;

pub use commands::{Choice, CommandSpec, InputAction, ModeState};
pub use conversation::{Actor, Item, Message, Snapshot, ToolCall, ToolState};
pub use event::{Event, RunOutcome};
pub use forms::{FieldKind, FormField, FormSpec, FormValues};
pub use ids::{
    ActorId, CallId, ConversationId, MessageId, OperationId, PermissionId, RequestId, RunId,
};
pub use images::{ImageMessage, ImageUpload};
pub use limits::{MAX_IMAGE_BYTES, MAX_IMAGE_PIXELS, MAX_IMAGES};
pub use measures::{ContextUsage, ModelInfo, TokenUsage};
pub use panels::{BackgroundStatus, PanelSpec};
pub use permissions::{Decision, Permission, PermissionDecision};
pub use rejection::{ErrorCode, Rejection};
pub use request::{Command, Request};
pub use sidebar::{
    MeterColor, MeterSegment, SidebarAction, SidebarContent, SidebarEntry, SidebarSection,
    SidebarSpec, SidebarTone,
};
