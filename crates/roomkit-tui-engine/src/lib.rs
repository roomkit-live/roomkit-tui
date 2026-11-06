//! Local integration with an application-owned RoomKit engine.
//!
//! Register [`ConversationChannel`] on the host's kit and attach it to a room.
//! [`LocalSession`] translates that room into the generic client contract.
//! The host owns agents, tools, policy, storage, runtime and engine shutdown.
//! A detached or slow view never stops the channel from reading an execution.

mod activity;
mod approval_history;
mod approvals;
mod cancellation;
mod channel;
mod cost;
mod diagnostics;
mod execution;
mod lifecycle;
mod measurements;
mod own_turns;
mod progress;
mod projection;
mod session;
mod stream;

pub use approvals::PermissionRequest;
pub use channel::ConversationChannel;
pub use cost::{conversation_cost, turn_cost};
pub use diagnostics::Diagnostic;
pub use execution::{CommandExecution, ExecutionContext};
pub use lifecycle::{LifecycleFuture, SessionLifecycle, TurnOutcome};
pub use session::{AdapterError, CommandHandler, CommandResult, LocalSession, SessionConfig};
