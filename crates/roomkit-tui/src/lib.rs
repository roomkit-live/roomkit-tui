//! Reusable terminal interactions for an application-owned conversation host.
//!
//! Create a bounded [`connection`], give its [`Host`] endpoint to your own
//! execution loop, and call [`run`] with the [`Client`]. The host chooses every
//! agent, provider, command and tool. Closing the terminal disconnects its
//! transport; ownership and cleanup of the host remain with the application.
//!
//! [`Session`] and [`render`] can be exercised without a real terminal. Tool
//! presentations are optional; unknown tools always have a generic renderer.

pub mod appearance;
mod clipboard;
pub mod content;
mod editor;
mod form;
mod markdown;
mod menu;
mod observation;
mod panel;
mod picker;
pub mod render;
mod selection;
mod session;
mod sidebar;
mod status;
mod terminal;
#[cfg(test)]
mod tests;

pub use appearance::{Branding, Color, ColorMode, SelectionMode, Theme, UiOptions, WelcomeScreen};
pub use form::FormState;
pub use menu::{CommandMenu, MenuEntry};
pub use observation::{OperationObservation, RunObservation, SessionObservation, SessionObserver};
pub use panel::PanelState;
pub use picker::ChoicePicker;
pub use roomkit_tui_transport::{Client, Host, connection};
pub use session::{APPROVAL_ARMING_DELAY, Focus, Limits, Session, SessionError};
pub use sidebar::SidebarState;
pub use status::TerminalStatus;
pub use terminal::{ExitReason, run, run_with_observer, run_with_options};

pub use roomkit_tui_protocol as protocol;
