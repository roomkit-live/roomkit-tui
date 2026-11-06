//! Serving one view. Host events keep flowing while a request awaits
//! application code, so a slow handler never makes the view fall behind.

use std::collections::VecDeque;
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
    let approvals = session
        .channel
        .approvals
        .attach(session.config.conversation(), session.config.agent.as_str());
    let mut view = View {
        session,
        host,
        handler,
        work: None,
        steering: Vec::new(),
        approvals,
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
                pending = view.complete(step)?;
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

    /// Forward one room event. Returns false once the channel is gone.
    fn forward(
        &mut self,
        frame: Result<(String, Event), broadcast::error::RecvError>,
    ) -> Result<bool, AdapterError> {
        match frame {
            Ok((room, mut event)) if room == self.session.config.conversation() => {
                crate::measurements::for_view(&mut event, &self.session.config);
                let cost = self.costs.record(&event, &self.session.config);
                let mut events = self.progress.apply(event);
                events.extend(cost);
                self.send_all(events)?;
                Ok(true)
            }
            Ok(_) => Ok(true),
            Err(broadcast::error::RecvError::Lagged(_)) => Err(AdapterError::Lagged),
            Err(broadcast::error::RecvError::Closed) => Ok(false),
        }
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
    /// step with the mode and title they confirm.
    pub fn publish(&mut self, events: Vec<Event>) -> Result<(), AdapterError> {
        for event in &events {
            self.session.config.record(event);
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
