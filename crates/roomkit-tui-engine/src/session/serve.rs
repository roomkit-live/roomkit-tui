//! Serving one view. Host events keep flowing while a request awaits
//! application code, so a slow handler never makes the view fall behind.

use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::mpsc::{RecvTimeoutError, TrySendError};
use std::task::{Context, Poll};
use std::time::Duration;

use roomkit::ConversationStore;
use roomkit_tui_protocol::{ErrorCode, Event, Rejection, Request};
use roomkit_tui_transport::{Host, HostEvents, HostRequests};
use tokio::sync::{broadcast, mpsc, watch};

use super::requests::Step;
use super::{AdapterError, CommandHandler, LocalSession};
use crate::approvals;
use crate::cost::Costs;
use crate::execution::Interrupter;
use crate::progress::ToolProgress;

/// Requests read ahead of the view, and requests held while another awaits
/// application code. Beyond that, requests are refused as overloaded.
const REQUEST_BUFFER: usize = 64;
/// How often the request reader notices that the view has ended.
const READER_POLL: Duration = Duration::from_millis(100);

/// Application code awaited on behalf of one request.
pub(super) type Pending<'h> = Pin<Box<dyn Future<Output = Step> + Send + 'h>>;

/// What a view keeps of a room it left, while work runs there or the view
/// stays in its group: what stops that work when it returns, and what keeps
/// the room's decisions open.
pub(super) struct Parked {
    /// The group it was left in.
    pub group: String,
    pub work: Option<Interrupter>,
    pub steering: Vec<Interrupter>,
    /// Held only: dropped, it withdraws the room's decisions.
    pub _approvals: approvals::View,
}

/// The state of one served view.
pub(super) struct View<'h, S: ConversationStore, H> {
    pub session: LocalSession<S>,
    pub host: HostEvents,
    pub handler: &'h H,
    pub work: Option<Interrupter>,
    /// Turns of messages sent while another turn ran, should RoomKit give
    /// them one: this view may stop them too.
    pub steering: Vec<Interrupter>,
    pub approvals: approvals::View,
    /// The rooms it left and still answers for, by room.
    pub parked: BTreeMap<String, Parked>,
    /// Calls in flight, shown running or waiting for a decision.
    progress: ToolProgress,
    /// The conversation's priced turns, whose cost each usage report updates.
    costs: Costs,
    /// A replacement conversation is being restored. Room events wait in
    /// the receiver meanwhile, then reach the view the switch selects.
    pub switching: bool,
    updates: Option<watch::Receiver<Vec<Event>>>,
    /// Requests that arrived while another awaited application code.
    pub queue: VecDeque<Request>,
}

pub(super) async fn serve<S: ConversationStore, H: CommandHandler>(
    session: LocalSession<S>,
    host: Host,
    handler: &H,
) -> Result<(), AdapterError> {
    let mut broadcast = session.channel.events.subscribe();
    let (snapshot, costs) = session.attachment().await?;
    let (requests, host) = host.split();
    let approvals = session.channel.approvals.attach_in(
        session.config.conversation(),
        session.config.agent.as_str(),
        session.config.group_name(),
    );
    let mut view = View {
        session,
        host,
        handler,
        work: None,
        steering: Vec::new(),
        approvals,
        parked: BTreeMap::new(),
        progress: ToolProgress::default(),
        costs: Costs::default(),
        switching: false,
        updates: None,
        queue: VecDeque::new(),
    };
    view.attached(snapshot, costs)?;
    let mut incoming = read_requests(requests);
    let mut pending: Option<Pending<'_>> = None;
    loop {
        tokio::select! {
            frame = broadcast.recv(), if !view.switching => {
                if !view.forward(frame)? {
                    return Ok(());
                }
            }
            update = application_update(&mut view.updates) => match update {
                Some(events) => view.publish(events)?,
                None => view.updates = None,
            },
            step = poll_pending(&mut pending) => {
                pending = view.complete(step, &mut broadcast)?;
                while pending.is_none() && let Some(request) = view.queue.pop_front() {
                    pending = view.receive(request)?;
                }
            }
            request = incoming.recv() => match request {
                None => return Ok(()),
                Some(request) if pending.is_some() && !super::requests::is_control(&request) => {
                    view.hold(request)?;
                }
                Some(request) => {
                    if let Some(next) = view.receive(request)? {
                        pending = Some(next);
                    }
                }
            },
        }
    }
}

impl<S: ConversationStore, H: CommandHandler> View<'_, S, H> {
    /// Announce a newly attached or replaced conversation to the client.
    pub fn attached(
        &mut self,
        snapshot: roomkit_tui_protocol::Snapshot,
        costs: Costs,
    ) -> Result<(), AdapterError> {
        self.costs = costs;
        if let Some(lifecycle) = &self.session.lifecycle {
            lifecycle.attached(&self.session.config);
        }
        self.progress = ToolProgress::new(snapshot.conversation.clone());
        self.send(Event::Snapshot(snapshot))?;
        self.updates = self.handler.subscribe(self.session.config.clone());
        if let Some(receiver) = &mut self.updates {
            let events = receiver.borrow_and_update().clone();
            self.publish(events)?;
        }
        if let Some(text) = self.session.config.submission_error.clone() {
            let conversation = self.session.config.conversation().into();
            self.send(Event::Notice { conversation, text })?;
        }
        Ok(())
    }

    /// Tell the view, after its snapshot, what the snapshot does not hold:
    /// `catch_up`, the runs under way, and the decisions they wait for.
    pub fn caught_up(&mut self, catch_up: Vec<Event>) -> Result<(), AdapterError> {
        let config = &self.session.config;
        let (room, agent) = (config.conversation(), config.agent.as_str());
        let group = config.group_name();
        let requested = self.session.channel.approvals.requested(room, agent, group);
        let requested = requested
            .into_iter()
            .map(|permission| Event::PermissionRequested {
                conversation: room.into(),
                permission,
            });
        let asked = self.session.channel.questions.pending((room, agent));
        let asked = asked.into_iter().map(|question| Event::QuestionAsked {
            conversation: room.into(),
            question,
        });
        for event in catch_up.into_iter().chain(requested).chain(asked) {
            let events = self.progress.apply(event);
            self.send_all(events)?;
        }
        Ok(())
    }

    /// Forward one room event. Returns false once the channel is gone.
    pub(super) fn forward(
        &mut self,
        frame: Result<(String, Event), broadcast::error::RecvError>,
    ) -> Result<bool, AdapterError> {
        let here = self.session.config.conversation().to_owned();
        match frame {
            Ok((room, mut event)) if room == self.session.config.conversation() => {
                crate::measurements::for_view(&mut event, &self.session.config);
                let cost = self.costs.record(&event, &self.session.config);
                let mut events = self.progress.apply(event);
                events.extend(cost);
                self.send_all(events)?;
                Ok(true)
            }
            // Asked anywhere in the view's group, told wherever it is.
            Ok((room, mut event))
                if room == self.session.config.group_name() && decides(&event) =>
            {
                retarget(&mut event, &here);
                let events = self.progress.apply(event);
                self.send_all(events)?;
                Ok(true)
            }
            // A conversation the view left still waits on the person.
            Ok((room, Event::PermissionRequested { .. })) if self.left_group(&room) => {
                self.send(Event::Notice {
                    conversation: here.as_str().into(),
                    text: "An agent of a conversation you left is waiting for your approval: /resume it to answer".into(),
                })?;
                Ok(true)
            }
            Ok((room, Event::Notice { text, .. })) if self.tells_notices_of(&room) => {
                self.send(Event::Notice {
                    conversation: here.as_str().into(),
                    text,
                })?;
                Ok(true)
            }
            Ok(_) => Ok(true),
            Err(broadcast::error::RecvError::Lagged(_)) => Err(AdapterError::Lagged),
            Err(broadcast::error::RecvError::Closed) => Ok(false),
        }
    }

    /// Whether `group` is another group this view left, whose work runs on.
    fn left_group(&self, group: &str) -> bool {
        group != self.session.config.group_name()
            && self.parked.values().any(|left| left.group == group)
    }

    /// Whether notices published under `room` are for this view.
    fn tells_notices_of(&self, room: &str) -> bool {
        room == crate::approvals::EVERY_VIEW
            || room == crate::approvals::group_notices(self.session.config.group_name())
    }

    /// Keep a request until the pending one completes, within a bound.
    fn hold(&mut self, request: Request) -> Result<(), AdapterError> {
        if self.queue.len() < REQUEST_BUFFER {
            self.queue.push_back(request);
            return Ok(());
        }
        let message = "The host is busy with earlier requests; retry shortly";
        self.reply(
            &request,
            Some(Rejection::new(ErrorCode::Overloaded, message)),
        )
    }

    pub fn send(&self, event: Event) -> Result<(), AdapterError> {
        send(&self.host, event)
    }

    pub fn send_all(&self, events: Vec<Event>) -> Result<(), AdapterError> {
        events.into_iter().try_for_each(|event| self.send(event))
    }

    /// Send events the host published, keeping this view's configuration in
    /// step with the mode and title they confirm. Their measurements and
    /// estimates get the configured capacity, as the engine's own do.
    pub fn publish(&mut self, mut events: Vec<Event>) -> Result<(), AdapterError> {
        for event in &mut events {
            self.session.config.record(event);
            crate::measurements::for_view(event, &self.session.config);
        }
        self.send_all(events)
    }

    pub fn reply(&self, request: &Request, error: Option<Rejection>) -> Result<(), AdapterError> {
        self.send(Event::Reply {
            request: request.id,
            error,
        })
    }
}

pub(super) fn send(host: &HostEvents, event: Event) -> Result<(), AdapterError> {
    match host.try_send(event) {
        // The request reader observes this normal detach. A late record or
        // diagnostic can race with the client's exit.
        Ok(()) | Err(TrySendError::Disconnected(_)) => Ok(()),
        Err(TrySendError::Full(_)) => Err(AdapterError::Lagged),
    }
}

/// Read client requests on a blocking thread, without polling the view.
fn read_requests(requests: HostRequests) -> mpsc::Receiver<Request> {
    let (sender, receiver) = mpsc::channel(REQUEST_BUFFER);
    tokio::task::spawn_blocking(move || {
        loop {
            let delivered = match requests.recv_timeout(READER_POLL) {
                Ok(request) => sender.blocking_send(request).is_ok(),
                Err(RecvTimeoutError::Timeout) => !sender.is_closed(),
                Err(RecvTimeoutError::Disconnected) => false,
            };
            if !delivered {
                return;
            }
        }
    });
    receiver
}

async fn application_update(
    updates: &mut Option<watch::Receiver<Vec<Event>>>,
) -> Option<Vec<Event>> {
    match updates {
        Some(receiver) => {
            receiver.changed().await.ok()?;
            Some(receiver.borrow_and_update().clone())
        }
        None => std::future::pending().await,
    }
}

async fn poll_pending(pending: &mut Option<Pending<'_>>) -> Step {
    match pending {
        Some(future) => future.as_mut().await,
        None => std::future::pending().await,
    }
}

/// Resolves to `None` when application code panics, so that a host bug is
/// reported to the client instead of ending the view.
pub(super) struct CatchUnwind<F>(pub Pin<Box<F>>);

impl<F: Future> Future for CatchUnwind<F> {
    type Output = Option<F::Output>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let inner = self.0.as_mut();
        match std::panic::catch_unwind(AssertUnwindSafe(|| inner.poll(context))) {
            Ok(Poll::Ready(value)) => Poll::Ready(Some(value)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(_) => Poll::Ready(None),
        }
    }
}

/// Whether `event` is a decision being asked or settled.
pub(super) fn decides(event: &Event) -> bool {
    matches!(
        event,
        Event::PermissionRequested { .. } | Event::PermissionResolved { .. }
    )
}

/// Sets `event`, a decision asked or settled in the view's group, to the
/// conversation the view shows.
fn retarget(event: &mut Event, here: &str) {
    if let Event::PermissionRequested { conversation, .. }
    | Event::PermissionResolved { conversation, .. } = event
    {
        *conversation = here.into();
    }
}
