//! Which rooms are executing. A view reserves a room atomically before it
//! starts work there, so two views can never start two operations at once.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use roomkit_tui_protocol::{MessageId, RunId, RunOutcome};
use tokio::sync::watch;

/// What a reservation starts in a room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Work {
    /// A turn of the agent, for a person's message.
    Turn,
    /// Work of the application's own, outside any turn.
    Host,
}

/// A room's active work: how many operations and deliveries, and whether
/// its reservation is the application's own work.
#[derive(Debug, Default)]
struct Room {
    count: usize,
    hosting: bool,
}

/// Why a room could not be reserved: work is active there, and whether the
/// application's own work holds it, as seen when the reservation was refused.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Held {
    pub hosting: bool,
}

/// Active operations and deliveries per room, plus their total for waiters.
#[derive(Clone, Debug)]
pub(crate) struct Activity {
    rooms: Arc<Mutex<BTreeMap<String, Room>>>,
    total: watch::Sender<usize>,
}

impl Default for Activity {
    fn default() -> Self {
        Self {
            rooms: Arc::default(),
            total: watch::channel(0).0,
        }
    }
}

impl Activity {
    pub fn busy(&self, room: &str) -> bool {
        self.lock().contains_key(room)
    }

    /// Reserve an idle room for `work`; refused, with what holds it, when
    /// anything is active there. Both are decided under one lock.
    pub fn reserve(&self, room: &str, work: Work) -> Result<Active, Held> {
        let mut rooms = self.lock();
        if let Some(held) = rooms.get(room) {
            return Err(Held {
                hosting: held.hosting,
            });
        }
        let hosting = work == Work::Host;
        rooms.insert(room.to_owned(), Room { count: 1, hosting });
        self.total.send_modify(|total| *total += 1);
        drop(rooms);
        Ok(Active {
            activity: self.clone(),
            room: room.to_owned(),
            hosting,
        })
    }

    /// Mark engine-owned work active, whatever else runs in the room.
    pub fn enter(&self, room: &str) -> Active {
        self.lock().entry(room.to_owned()).or_default().count += 1;
        self.total.send_modify(|total| *total += 1);
        Active {
            activity: self.clone(),
            room: room.to_owned(),
            hosting: false,
        }
    }

    /// Wait until nothing is active in any room.
    pub async fn idle(&self) {
        let mut total = self.total.subscribe();
        // The sender lives as long as this value; an error cannot occur.
        let _ = total.wait_for(|total| *total == 0).await;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Room>> {
        self.rooms.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One active operation or delivery; dropping it releases the room.
#[derive(Debug)]
pub(crate) struct Active {
    activity: Activity,
    room: String,
    /// The reservation of the application's own work.
    hosting: bool,
}

impl Drop for Active {
    fn drop(&mut self) {
        let mut rooms = self.activity.lock();
        if let Some(held) = rooms.get_mut(&self.room) {
            held.count -= 1;
            if self.hosting {
                held.hosting = false;
            }
            if held.count == 0 {
                rooms.remove(&self.room);
            }
        }
        drop(rooms);
        self.activity.total.send_modify(|total| *total -= 1);
    }
}

/// The runs whose stream the channel is reading: an interruption reports a
/// run only once its stream has ended.
#[derive(Clone, Debug)]
pub(crate) struct Streams {
    runs: Arc<Mutex<BTreeMap<(String, RunId), usize>>>,
    /// Counts the streams that ended, for those waiting on one.
    ended: watch::Sender<u64>,
}

impl Default for Streams {
    fn default() -> Self {
        Self {
            runs: Arc::default(),
            ended: watch::channel(0).0,
        }
    }
}

impl Streams {
    /// Mark the stream of `run` read until the returned guard drops.
    pub fn open(&self, room: &str, run: &RunId) -> Streaming {
        let key = (room.to_owned(), run.clone());
        *self.lock().entry(key.clone()).or_default() += 1;
        Streaming {
            streams: self.clone(),
            key,
        }
    }

    /// Resolves once no stream of `run` is read.
    pub async fn ended(&self, room: &str, run: &RunId) {
        let mut ended = self.ended.subscribe();
        let key = (room.to_owned(), run.clone());
        while self.lock().contains_key(&key) {
            // The sender lives as long as this value; an error cannot occur.
            let _ = ended.changed().await;
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<(String, RunId), usize>> {
        self.runs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A stream being read; dropping it ends it.
pub(crate) struct Streaming {
    streams: Streams,
    key: (String, RunId),
}

impl Drop for Streaming {
    fn drop(&mut self) {
        let mut runs = self.streams.lock();
        if let Some(count) = runs.get_mut(&self.key) {
            *count -= 1;
            if *count == 0 {
                runs.remove(&self.key);
            }
        }
        drop(runs);
        self.streams.ended.send_modify(|count| *count += 1);
    }
}

/// A stopping run: whether its stream left its end to the interruption, with
/// the outcome it reached and the draft it was writing.
#[derive(Debug, Default)]
struct Stop {
    deferred: Option<RunOutcome>,
    draft: Option<MessageId>,
}

type StoppingRuns = Arc<Mutex<BTreeMap<(String, RunId), Stop>>>;

/// Runs whose interruption was accepted. Their stream defers its terminal
/// event until the interruption has seen it end, recording its draft.
#[derive(Clone, Debug)]
pub(crate) struct Stopping {
    runs: StoppingRuns,
    /// Counts accepted interruptions, for those waiting on one.
    accepted: watch::Sender<u64>,
}

impl Default for Stopping {
    fn default() -> Self {
        Self {
            runs: Arc::default(),
            accepted: watch::channel(0).0,
        }
    }
}

impl Stopping {
    pub fn register(&self, room: &str, run: &RunId) -> StopGuard {
        let key = (room.to_owned(), run.clone());
        self.lock().insert(key.clone(), Stop::default());
        StopGuard {
            runs: self.runs.clone(),
            key,
        }
    }

    /// Whether the interruption of `run` was accepted and it is stopping.
    pub fn contains(&self, room: &str, run: &RunId) -> bool {
        self.lock().contains_key(&(room.to_owned(), run.clone()))
    }

    /// Wake those waiting on a run's stop: its engine accepted the cancel.
    pub fn accepted(&self) {
        self.accepted.send_modify(|count| *count += 1);
    }

    /// Resolves once the interruption of `run` is accepted.
    pub async fn stopped(&self, room: &str, run: &RunId) {
        let mut accepted = self.accepted.subscribe();
        while !self.contains(room, run) {
            // The sender lives as long as this value; an error cannot occur.
            let _ = accepted.changed().await;
        }
    }

    /// Called by a stream as it ends. Returns whether its terminal event is
    /// deferred to the interruption, recording its outcome and draft.
    pub fn defer(&self, room: &str, run: &RunId, outcome: RunOutcome, draft: MessageId) -> bool {
        match self.lock().get_mut(&(room.to_owned(), run.clone())) {
            Some(stop) => {
                stop.deferred = Some(outcome);
                stop.draft = Some(draft);
                true
            }
            None => false,
        }
    }

    /// The draft a cancelled stream ended in, taken once. The deferred end
    /// stays recorded for the interruption to report.
    pub fn take_draft(&self, room: &str, run: &RunId) -> Option<MessageId> {
        self.lock()
            .get_mut(&(room.to_owned(), run.clone()))
            .and_then(|stop| stop.draft.take())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<(String, RunId), Stop>> {
        self.runs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Keeps a stream's terminal event for the interruption, which reports the
/// run once that stream has ended.
pub(crate) struct StopGuard {
    runs: StoppingRuns,
    key: (String, RunId),
}

impl StopGuard {
    /// Stop deferring the stream's end. Returns the outcome it already left
    /// to the interruption; a stream ending afterwards reports itself.
    pub fn release(self) -> Option<RunOutcome> {
        self.runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key)
            .and_then(|stop| stop.deferred)
    }
}

impl Drop for StopGuard {
    fn drop(&mut self) {
        self.runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, reason = "Fixture failures are assertions")]
    use super::*;

    #[tokio::test]
    async fn a_room_is_reserved_once_and_idle_waits_for_every_release() {
        let activity = Activity::default();
        let first = activity.reserve("room", Work::Host).expect("an idle room");
        let held = activity.reserve("room", Work::Turn);
        assert!(held.expect_err("One reservation at a time").hosting);
        let stream = activity.enter("room");
        let other = activity
            .reserve("other", Work::Turn)
            .expect("rooms are independent");
        assert!(
            !activity
                .reserve("other", Work::Turn)
                .expect_err("held")
                .hosting
        );
        drop(first);
        let held = activity.reserve("room", Work::Turn);
        let held = held.expect_err("The delivery still runs");
        assert!(!held.hosting, "The host work ended");
        drop((stream, other));
        assert!(!activity.busy("room"));
        tokio::time::timeout(std::time::Duration::from_secs(1), activity.idle())
            .await
            .expect("idle once everything is released");
    }

    #[tokio::test]
    async fn a_run_s_stream_ends_once_its_last_reader_is_done() {
        let streams = Streams::default();
        let run = RunId::from("agent:turn");
        let within = std::time::Duration::from_secs(1);
        tokio::time::timeout(within, streams.ended("room", &run))
            .await
            .expect("No stream: ended at once");
        let (first, second) = (streams.open("room", &run), streams.open("room", &run));
        drop(first);
        let ended = tokio::time::timeout(within / 10, streams.ended("room", &run)).await;
        assert!(ended.is_err(), "One reader is still reading");
        let waiting = tokio::spawn({
            let streams = streams.clone();
            let run = run.clone();
            async move { streams.ended("room", &run).await }
        });
        drop(second);
        tokio::time::timeout(within, waiting)
            .await
            .expect("Ended with its last reader")
            .expect("The waiter does not panic");
    }

    /// Review finding: a cancelled row taking the stream's draft used to
    /// erase the deferral, so no one reported the end of the run.
    #[test]
    fn taking_the_draft_keeps_the_deferred_end() {
        let stopping = Stopping::default();
        let run = RunId::from("agent:turn");
        let guard = stopping.register("room", &run);
        let draft = MessageId::from("draft:agent:turn:0");
        assert!(stopping.defer("room", &run, RunOutcome::Completed, draft.clone()));
        assert_eq!(stopping.take_draft("room", &run), Some(draft));
        assert_eq!(stopping.take_draft("room", &run), None, "Taken once");
        assert_eq!(guard.release(), Some(RunOutcome::Completed));
        assert!(
            !stopping.defer("room", &run, RunOutcome::Completed, "d".into()),
            "A stream ending after the release reports its own end"
        );
    }
}
