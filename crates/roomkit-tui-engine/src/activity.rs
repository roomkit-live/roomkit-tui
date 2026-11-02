//! Which rooms are executing. A view reserves a room atomically before it
//! starts work there, so two views can never start two operations at once.

use std::collections::{BTreeMap, BTreeSet};
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

/// Active operations and deliveries per room, plus their total for waiters.
#[derive(Clone, Debug)]
pub(crate) struct Activity {
    rooms: Arc<Mutex<BTreeMap<String, usize>>>,
    /// Rooms whose reservation is the application's own work.
    hosting: Arc<Mutex<BTreeSet<String>>>,
    total: watch::Sender<usize>,
}

impl Default for Activity {
    fn default() -> Self {
        Self {
            rooms: Arc::default(),
            hosting: Arc::default(),
            total: watch::channel(0).0,
        }
    }
}

impl Activity {
    pub fn busy(&self, room: &str) -> bool {
        self.lock().contains_key(room)
    }

    /// Reserve an idle room for `work`; `None` when anything is active there.
    pub fn reserve(&self, room: &str, work: Work) -> Option<Active> {
        let mut rooms = self.lock();
        if rooms.contains_key(room) {
            return None;
        }
        rooms.insert(room.to_owned(), 1);
        self.total.send_modify(|total| *total += 1);
        drop(rooms);
        let hosting = work == Work::Host;
        if hosting {
            self.hosts().insert(room.to_owned());
        }
        Some(Active {
            activity: self.clone(),
            room: room.to_owned(),
            hosting,
        })
    }

    /// Whether the application's own work holds `room`.
    pub fn hosting(&self, room: &str) -> bool {
        self.hosts().contains(room)
    }

    /// Mark engine-owned work active, whatever else runs in the room.
    pub fn enter(&self, room: &str) -> Active {
        *self.lock().entry(room.to_owned()).or_default() += 1;
        self.total.send_modify(|total| *total += 1);
        self.active(room)
    }

    /// Wait until nothing is active in any room.
    pub async fn idle(&self) {
        let mut total = self.total.subscribe();
        // The sender lives as long as this value; an error cannot occur.
        let _ = total.wait_for(|total| *total == 0).await;
    }

    fn active(&self, room: &str) -> Active {
        Active {
            activity: self.clone(),
            room: room.to_owned(),
            hosting: false,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, usize>> {
        self.rooms.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn hosts(&self) -> std::sync::MutexGuard<'_, BTreeSet<String>> {
        self.hosting.lock().unwrap_or_else(PoisonError::into_inner)
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
        if self.hosting {
            self.activity.hosts().remove(&self.room);
        }
        let mut rooms = self.activity.lock();
        if let Some(count) = rooms.get_mut(&self.room) {
            *count -= 1;
            if *count == 0 {
                rooms.remove(&self.room);
            }
        }
        drop(rooms);
        self.activity.total.send_modify(|total| *total -= 1);
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
/// event until delivery is joined, recording the draft it was writing.
#[derive(Clone, Debug, Default)]
pub(crate) struct Stopping(StoppingRuns);

impl Stopping {
    pub fn register(&self, room: &str, run: &RunId) -> StopGuard {
        let key = (room.to_owned(), run.clone());
        self.lock().insert(key.clone(), Stop::default());
        StopGuard {
            runs: self.0.clone(),
            key,
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
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Keeps a stream's terminal event deferred until delivery is joined.
pub(crate) struct StopGuard {
    runs: StoppingRuns,
    key: (String, RunId),
}

impl StopGuard {
    /// The outcome the stream reached if it ended after the interruption and
    /// left its terminal event to it; `None` when it had finished on its own.
    pub fn deferred(&self) -> Option<RunOutcome> {
        self.runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&self.key)
            .and_then(|stop| stop.deferred)
    }
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
        assert!(
            activity.reserve("room", Work::Turn).is_none(),
            "One reservation at a time"
        );
        assert!(activity.hosting("room"));
        let stream = activity.enter("room");
        let other = activity
            .reserve("other", Work::Turn)
            .expect("rooms are independent");
        assert!(!activity.hosting("other"));
        drop(first);
        assert!(!activity.hosting("room"), "The host work ended");
        assert!(activity.busy("room"), "The delivery still runs");
        drop((stream, other));
        assert!(!activity.busy("room"));
        tokio::time::timeout(std::time::Duration::from_secs(1), activity.idle())
            .await
            .expect("idle once everything is released");
    }

    /// Review finding: a cancelled row taking the stream's draft used to
    /// erase the deferral, so no one reported the end of the run.
    #[test]
    fn taking_the_draft_keeps_the_deferred_end() {
        let stopping = Stopping::default();
        let run = RunId::from("agent:turn");
        let guard = stopping.register("room", &run);
        assert_eq!(guard.deferred(), None);
        let draft = MessageId::from("draft:agent:turn:0");
        assert!(stopping.defer("room", &run, RunOutcome::Completed, draft.clone()));
        assert_eq!(stopping.take_draft("room", &run), Some(draft));
        assert_eq!(stopping.take_draft("room", &run), None, "Taken once");
        assert_eq!(guard.deferred(), Some(RunOutcome::Completed));
        assert_eq!(guard.release(), Some(RunOutcome::Completed));
        assert!(
            !stopping.defer("room", &run, RunOutcome::Completed, "d".into()),
            "A stream ending after the release reports its own end"
        );
    }
}
