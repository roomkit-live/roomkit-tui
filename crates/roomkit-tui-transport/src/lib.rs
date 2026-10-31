//! Bounded in-process connections shared by terminal clients and application adapters.

use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::time::Duration;

use roomkit_tui_protocol::{Event, Request};

/// The terminal's side of a bounded, ordered connection.
#[derive(Debug)]
pub struct Client {
    requests: SyncSender<Request>,
    events: Receiver<Box<Event>>,
}

/// The application's side. A disconnected client never implicitly stops a kit.
#[derive(Debug)]
pub struct Host {
    requests: HostRequests,
    events: HostEvents,
}

/// The receiving half of a [`Host`], for a task dedicated to reading requests.
#[derive(Debug)]
pub struct HostRequests(Receiver<Request>);

/// The publishing half of a [`Host`]. Clones publish into the same ordered queue.
#[derive(Clone, Debug)]
pub struct HostEvents(SyncSender<Box<Event>>);

/// Create bounded request and event queues. Capacity must be nonzero.
///
/// The terminal only performs nonblocking sends. A full queue preserves the
/// rejected value so the caller can retry rather than silently lose an action.
/// The host must keep processing control requests while producing events.
pub fn connection(capacity: std::num::NonZeroUsize) -> (Client, Host) {
    let (request_tx, request_rx) = mpsc::sync_channel(capacity.get());
    let (event_tx, event_rx) = mpsc::sync_channel(capacity.get());
    (
        Client {
            requests: request_tx,
            events: event_rx,
        },
        Host {
            requests: HostRequests(request_rx),
            events: HostEvents(event_tx),
        },
    )
}

impl Client {
    /// Submit without blocking, retaining ownership on overload or disconnect.
    pub fn try_send(&self, request: Request) -> Result<(), TrySendError<Request>> {
        self.requests.try_send(request)
    }

    /// Read the next event without blocking.
    pub fn try_recv(&self) -> Result<Event, TryRecvError> {
        self.events.try_recv().map(|event| *event)
    }

    /// Wait for a host event, useful to noninteractive consumers.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Event, mpsc::RecvTimeoutError> {
        self.events.recv_timeout(timeout).map(|event| *event)
    }
}

impl Host {
    /// Publish without blocking. Retain and retry a full queue's event.
    pub fn try_send(&self, event: Event) -> Result<(), TrySendError<Box<Event>>> {
        self.events.try_send(event)
    }

    /// Read a client request without blocking.
    pub fn try_recv(&self) -> Result<Request, TryRecvError> {
        self.requests.try_recv()
    }

    /// Wait for a request with a deadline.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Request, mpsc::RecvTimeoutError> {
        self.requests.recv_timeout(timeout)
    }

    /// Separate reading requests from publishing events, so that a blocking
    /// reader never delays publication.
    pub fn split(self) -> (HostRequests, HostEvents) {
        (self.requests, self.events)
    }
}

impl HostRequests {
    /// Read a client request without blocking.
    pub fn try_recv(&self) -> Result<Request, TryRecvError> {
        self.0.try_recv()
    }

    /// Wait for a request with a deadline.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Request, mpsc::RecvTimeoutError> {
        self.0.recv_timeout(timeout)
    }
}

impl HostEvents {
    /// Publish without blocking. Retain and retry a full queue's event.
    pub fn try_send(&self, event: Event) -> Result<(), TrySendError<Box<Event>>> {
        self.0.try_send(Box::new(event))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "Test fixtures fail on invalid state")]
    use super::*;
    use roomkit_tui_protocol::{Command, ConversationId, RequestId};
    use std::num::NonZeroUsize;

    fn request(id: u64) -> Request {
        Request::new(
            RequestId::new(id),
            format!("operation-{id}"),
            "room",
            Command::Interrupt { run: "run".into() },
        )
    }

    fn notice(text: &str) -> Event {
        Event::Notice {
            conversation: ConversationId::from("room"),
            text: text.into(),
        }
    }

    fn pair(capacity: usize) -> (Client, Host) {
        connection(NonZeroUsize::new(capacity).unwrap())
    }

    #[test]
    fn a_full_queue_returns_the_rejected_value_and_keeps_order() {
        let (client, host) = pair(2);
        client.try_send(request(1)).unwrap();
        client.try_send(request(2)).unwrap();
        let Err(TrySendError::Full(rejected)) = client.try_send(request(3)) else {
            unreachable!("A third request exceeds the capacity");
        };
        assert_eq!(rejected, request(3));
        assert_eq!(host.try_recv().unwrap(), request(1));
        client.try_send(rejected).unwrap();
        assert_eq!(host.try_recv().unwrap(), request(2));
        assert_eq!(host.try_recv().unwrap(), request(3));
        assert!(matches!(host.try_recv(), Err(TryRecvError::Empty)));

        host.try_send(notice("first")).unwrap();
        host.try_send(notice("second")).unwrap();
        let Err(TrySendError::Full(event)) = host.try_send(notice("third")) else {
            unreachable!("A third event exceeds the capacity");
        };
        assert_eq!(*event, notice("third"));
        assert_eq!(client.try_recv().unwrap(), notice("first"));
        assert_eq!(client.try_recv().unwrap(), notice("second"));
    }

    #[test]
    fn each_side_observes_the_other_disconnecting() {
        let (client, host) = pair(1);
        drop(host);
        assert!(matches!(
            client.try_send(request(1)),
            Err(TrySendError::Disconnected(_))
        ));
        assert!(matches!(client.try_recv(), Err(TryRecvError::Disconnected)));

        let (client, host) = pair(1);
        client.try_send(request(1)).unwrap();
        drop(client);
        assert_eq!(
            host.try_recv().unwrap(),
            request(1),
            "Queued requests survive"
        );
        assert!(matches!(host.try_recv(), Err(TryRecvError::Disconnected)));
        assert!(matches!(
            host.try_send(notice("late")),
            Err(TrySendError::Disconnected(_))
        ));
    }

    #[test]
    fn split_halves_read_and_publish_independently() {
        let (client, host) = pair(4);
        let (requests, events) = host.split();
        let reader = std::thread::spawn(move || {
            requests
                .recv_timeout(Duration::from_secs(5))
                .map(|request| request.id)
        });
        let publisher = events.clone();
        publisher.try_send(notice("from a clone")).unwrap();
        events.try_send(notice("from the original")).unwrap();
        client.try_send(request(9)).unwrap();
        assert_eq!(reader.join().unwrap().unwrap(), RequestId::new(9));
        assert_eq!(client.try_recv().unwrap(), notice("from a clone"));
        assert_eq!(client.try_recv().unwrap(), notice("from the original"));
        assert!(matches!(
            client.recv_timeout(Duration::from_millis(1)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
    }
}
