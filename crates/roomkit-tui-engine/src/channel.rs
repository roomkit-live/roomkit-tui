//! The RoomKit transport channel shared by local views of a room.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use roomkit::ToolCallEvent;
use roomkit::model::{ChannelId, RoomEvent, RoomId};
use roomkit_tui_protocol::{Decision, Event, Item, RunId};
use tokio::sync::broadcast;

use crate::activity::{Active, Activity, Held, Stopping, Streams, Work};
use crate::approvals::{Approvals, Details};
use crate::live_runs::LiveRuns;
use crate::own_turns::OwnTurns;
use crate::questions::{QuestionRequest, Questions};
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
    pub(crate) live_streams: Streams,
    /// The runs under way in each room, for a view that attaches to one.
    pub(crate) live_runs: LiveRuns,
    pub(crate) approvals: Approvals,
    /// One writer of decisions at a time: two would store the same row.
    pub(crate) storing_decisions: Arc<tokio::sync::Mutex<()>>,
    pub(crate) own_turns: OwnTurns,
    /// Who asked for the answer turn under way in a room: its trigger is never
    /// stored, so its stream cannot read them from the room.
    answers: Arc<Mutex<BTreeMap<String, String>>>,
    /// The questions waiting for the person in each room.
    pub(crate) questions: Questions,
    /// The latest run of each room whose stream told why it failed, so that
    /// the turn that delivered it does not tell it again.
    failures_told: Arc<Mutex<BTreeMap<String, RunId>>>,
}

/// The answer turn of a room being delivered; dropping it forgets who asked.
pub(crate) struct AnswerRequest {
    answers: Arc<Mutex<BTreeMap<String, String>>>,
    room: String,
}

impl Drop for AnswerRequest {
    fn drop(&mut self) {
        lock(&self.answers).remove(&self.room);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ConversationChannel {
    /// `sender` asks for an answer turn in `room`, which they alone may approve
    /// the tools of, until the returned guard drops.
    pub(crate) fn answer_requested_by(&self, room: &str, sender: &str) -> AnswerRequest {
        lock(&self.answers).insert(room.to_owned(), sender.to_owned());
        AnswerRequest {
            answers: self.answers.clone(),
            room: room.to_owned(),
        }
    }

    /// Who asked for the answer turn under way in `room`.
    pub(crate) fn answer_requester(&self, room: &str) -> Option<String> {
        lock(&self.answers).get(room).cloned()
    }

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
            live_streams: Streams::default(),
            live_runs: LiveRuns::default(),
            storing_decisions: Arc::default(),
            own_turns: OwnTurns::default(),
            answers: Arc::default(),
            failures_told: Arc::default(),
            questions: Questions::default(),
        }
    }

    /// Show the views of `room` the questions of `request`, which `agent`
    /// asks in its latest run there: a view's answer resolves the kit's
    /// human-input request with the text the agent reads, and a person who
    /// closes them rejects it. Call it from an `ON_USER_INPUT_REQUIRED`
    /// hook. `false` when no view can show them: no run of the room is
    /// watched, or they exceed what a terminal shows in full; the request
    /// then waits for an answer from elsewhere, or its timeout.
    pub fn ask_question(&self, room: &RoomId, agent: &ChannelId, request: QuestionRequest) -> bool {
        let Some(run) = self.last_run(room.as_str()) else {
            return false;
        };
        let question =
            roomkit_tui_protocol::Question::new(request.id, agent.as_str(), run, request.items);
        if !question.fits() {
            return false;
        }
        self.questions
            .add(room.as_str(), question.clone(), request.tool_call_id);
        let conversation = room.as_str().into();
        self.publish(
            room.as_str(),
            Event::QuestionAsked {
                conversation,
                question,
            },
        );
        true
    }

    /// The stream of `run` in `room` told why it failed.
    pub(crate) fn failure_told(&self, room: &str, run: &RunId) {
        lock(&self.failures_told).insert(room.to_owned(), run.clone());
    }

    /// Whether the stream of `run` in `room` told why it failed; the turn
    /// asking is the last to need it.
    pub(crate) fn take_failure_told(&self, room: &str, run: &RunId) -> bool {
        let mut told = lock(&self.failures_told);
        let was = told.get(room) == Some(run);
        if was {
            told.remove(room);
        }
        was
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
            group: request.group,
            room: room.to_owned(),
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
        // Under one lock: a view that reads the runs under way knows which
        // of the events it is then sent they already count.
        let mut runs = self.live_runs.lock();
        let withdrawn = self.questions.ended_by(room, &event);
        runs.apply(room, &event);
        if let Event::RunStarted { run, .. } = &event {
            self.approvals.announced(room, run);
        }
        // No subscribers is normal: the engine keeps executing and persisting.
        let _ = self.events.send((room.into(), event));
        for question in withdrawn {
            let conversation = room.into();
            let _ = self.events.send((
                room.into(),
                Event::QuestionResolved {
                    conversation,
                    question,
                },
            ));
        }
    }

    /// The last run announced in `room`, while a view watches it.
    pub(crate) fn last_run(&self, room: &str) -> Option<RunId> {
        self.approvals.last_run(room)
    }

    pub(crate) fn busy(&self, room: &str) -> bool {
        self.activity.busy(room)
    }

    /// Whether work runs in `room`: a turn, a delivery or the host's own.
    pub fn is_busy(&self, room: &RoomId) -> bool {
        self.busy(room.as_str())
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
