//! The host hears a steered message's own turn start (RMK-1232). A message
//! handed to the running turn starts nothing; should that turn no longer
//! read it, RoomKit gives it a turn of its own, which only the stream that
//! carries the turn sees begin. That stream tells the host, as the view
//! tells it of any turn, before the turn asks the model.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use roomkit::model::RoomEvent;
use roomkit_tui_protocol::OperationId;

use crate::{SessionConfig, SessionLifecycle};

/// What `started` needs for one steered message, and which expectation
/// recorded it.
#[derive(Debug)]
struct Start {
    token: u64,
    lifecycle: Arc<dyn SessionLifecycle>,
    config: SessionConfig,
    operation: OperationId,
}

/// Steered messages under delivery, by idempotency key.
#[derive(Clone, Debug, Default)]
pub(crate) struct OwnTurns {
    starts: Arc<Mutex<BTreeMap<String, Start>>>,
    tokens: Arc<AtomicU64>,
}

impl OwnTurns {
    /// Expect a turn of its own for the message keyed `key`, until the
    /// returned guard drops. Recorded before the message is committed: its
    /// delivery may begin as soon as it is.
    pub fn expect(
        &self,
        key: &str,
        lifecycle: Arc<dyn SessionLifecycle>,
        config: SessionConfig,
        operation: OperationId,
    ) -> Expected {
        let token = self.tokens.fetch_add(1, Ordering::Relaxed);
        let start = Start {
            token,
            lifecycle,
            config,
            operation,
        };
        self.lock().insert(key.to_owned(), start);
        Expected {
            turns: self.clone(),
            key: key.to_owned(),
            token,
        }
    }

    /// Tell the host the turn answering `trigger` starts, when it is a
    /// steered message's own turn. Its turn advances only as its stream is
    /// read, so the caller awaits this before the stream's first read.
    pub async fn start(&self, trigger: Option<&RoomEvent>) {
        let Some(key) = trigger.and_then(|row| row.idempotency_key.as_ref()) else {
            return;
        };
        // Taken: the host hears it once.
        let Some(start) = self.lock().remove(key) else {
            return;
        };
        start
            .lifecycle
            .started(&start.config, &start.operation)
            .await;
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Start>> {
        self.starts.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A steered message expecting a turn of its own; dropping it forgets the
/// message, when no turn of its own took it.
pub(crate) struct Expected {
    turns: OwnTurns,
    key: String,
    token: u64,
}

impl Drop for Expected {
    fn drop(&mut self) {
        let mut starts = self.turns.lock();
        // Never another expectation recorded under the same key since.
        if starts.get(&self.key).is_some_and(|s| s.token == self.token) {
            starts.remove(&self.key);
        }
    }
}
