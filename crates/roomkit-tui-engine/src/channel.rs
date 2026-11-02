//! The RoomKit transport channel shared by local views of a room.

use roomkit::ToolCallEvent;
use roomkit::model::{ChannelId, RoomEvent, RoomId};
use roomkit_tui_protocol::{Decision, Event, Item, RunId};
use tokio::sync::broadcast;

use crate::activity::{Active, Activity, Held, Stopping, Work};
use crate::approvals::{Approvals, Details};
use crate::{PermissionRequest, projection};

/// Events retained for a slow view before it must reattach.
const EVENT_BUFFER: usize = 1_024;

/// A RoomKit transport shared by independent local client connections.
///
/// The application registers and attaches this channel. Stream reading belongs
/// to the engine, not to a view: disconnects and queue overflow never drop the
/// engine's delivery stream. Events are bounded; lagging clients must reattach.
/// Streaming exposes text before broadcast hooks judge the final row. Use
/// [`Self::buffered`] when the application's policy requires pre-display review.
#[derive(Clone, Debug)]
pub struct ConversationChannel {
    id: ChannelId,
    /// Whether the engine streams answers to it as they are written.
    streaming: bool,
    pub(crate) events: broadcast::Sender<(String, Event)>,
    pub(crate) activity: Activity,
    pub(crate) stopping: Stopping,
    pub(crate) approvals: Approvals,
}

impl ConversationChannel {
    /// Create a streaming transport, without registering it or starting tasks.
    pub fn new(id: ChannelId) -> Self {
        Self::with_streaming(id, true)
    }

    /// Create a transport that delivers committed rows only, for rooms with
    /// output moderation hooks. Approvals are unavailable in this mode.
    pub fn buffered(id: ChannelId) -> Self {
        Self::with_streaming(id, false)
    }

    fn with_streaming(id: ChannelId, streaming: bool) -> Self {
        let events = broadcast::channel(EVENT_BUFFER).0;
        Self {
            id,
            streaming,
            approvals: Approvals::new(events.clone()),
            events,
            activity: Activity::default(),
            stopping: Stopping::default(),
        }
    }

    /// Identity used when the application registers and attaches this channel.
    pub fn id(&self) -> &ChannelId {
        &self.id
    }

    /// Wait for a one-time decision from an attached view of this room and
    /// agent, in its most recent run. The host awaits this before executing
    /// the concrete action, for example in a tool hook. Work a `CommandExecution` runs
    /// asks through [`ExecutionContext::ask_permission`](crate::ExecutionContext::ask_permission),
    /// which keeps the request in that run.
    /// Streaming delivery must be enabled. Missing views, oversized details,
    /// cancellation, timeout and disconnect all refuse the action. A request
    /// too large to show in full, beyond the pending limit, or made while
    /// approval history cannot be stored, is refused with a notice to the
    /// views, once per run and reason. This does not grant filesystem, shell
    /// or engine permissions by itself.
    pub async fn ask_permission(
        &self,
        room: &RoomId,
        agent: &ChannelId,
        request: PermissionRequest,
    ) -> Decision {
        self.ask(room.as_str(), agent.as_str(), request, None, None)
            .await
    }

    /// Ask before a concrete engine tool call. The confirmed decision is kept
    /// beside that call in the session history. A call outside a room is
    /// denied: no view can answer it.
    pub async fn ask_tool_permission(
        &self,
        event: &ToolCallEvent,
        request: PermissionRequest,
    ) -> Decision {
        // A call outside a room has no view to ask.
        let Some(room) = &event.room_id else {
            return Decision::Deny;
        };
        let call = Some(event.tool_call_id.clone());
        let (room, agent) = (room.as_str(), event.channel_id.as_str());
        self.ask(room, agent, request, call, None).await
    }

    /// Ask in the run started with `token` when given, otherwise in the
    /// scope's most recent run.
    pub(crate) async fn ask(
        &self,
        room: &str,
        agent: &str,
        request: PermissionRequest,
        tool_call_id: Option<String>,
        token: Option<u64>,
    ) -> Decision {
        if !self.streams() {
            return Decision::Deny;
        }
        let details = Details {
            title: request.title,
            detail: request.detail,
            summary: request.summary,
            tool_call_id,
            run: token,
        };
        self.approvals
            .ask(room, agent, details, request.timeout)
            .await
    }

    /// Wait until no operation or delivery is active in any room, so their
    /// decisions are stored. Call this before closing the engine.
    pub async fn idle(&self) {
        self.activity.idle().await;
    }

    pub(crate) fn streams(&self) -> bool {
        self.streaming
    }

    pub(crate) fn publish(&self, room: &str, event: Event) {
        if let Event::RunStarted { run, .. } = &event {
            self.approvals.announced(room, run);
        }
        // No subscribers is normal: the engine keeps executing and persisting.
        let _ = self.events.send((room.into(), event));
    }

    /// The last run announced in `room`, while a view watches it.
    pub(crate) fn last_run(&self, room: &str) -> Option<RunId> {
        self.approvals.last_run(room)
    }

    pub(crate) fn busy(&self, room: &str) -> bool {
        self.activity.busy(room)
    }

    pub(crate) fn reserve(&self, room: &str, work: Work) -> Result<Active, Held> {
        self.activity.reserve(room, work)
    }

    /// Publish a committed row: measurements, then its item. A cancelled
    /// answer replaces the draft its interrupted stream was writing.
    pub(crate) fn row(&self, row: &RoomEvent) {
        let room = row.room_id.as_str();
        if let Some(event) = crate::measurements::event(row) {
            self.publish(room, event);
        }
        let Some(item) = projection::item(row) else {
            return;
        };
        if let Item::Message(message) = &item
            && let Some(draft) = self.cancelled_draft(row)
        {
            let conversation = room.into();
            let message = message.clone();
            let event = Event::MessageFinalized {
                conversation,
                draft,
                message,
            };
            self.publish(room, event);
            return;
        }
        let conversation = room.into();
        self.publish(room, Event::Item { conversation, item });
    }

    fn cancelled_draft(&self, row: &RoomEvent) -> Option<roomkit_tui_protocol::MessageId> {
        let cancelled = row
            .metadata
            .get("cancelled")
            .and_then(serde_json::Value::as_bool);
        let trigger = row
            .responds_to
            .as_ref()
            .filter(|_| cancelled == Some(true))?;
        let run: RunId = projection::run_id(&row.source.channel_id, trigger);
        self.stopping.take_draft(row.room_id.as_str(), &run)
    }
}
