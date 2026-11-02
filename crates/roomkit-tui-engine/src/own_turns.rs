//! The host hears a steered message's own turn start (RMK-1232). A message
//! handed to the running turn starts nothing; should that turn no longer
//! read it, RoomKit gives it a turn of its own, which only the stream that
//! carries the turn sees begin. That stream tells the host before its first
//! read: the agent has prepared the turn by then (memory, the
//! `BEFORE_AI_GENERATION` hooks), but its loop has neither asked the model
//! nor called a tool, as a turn advances only as its stream is read.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use roomkit::model::RoomEvent;
use roomkit_tui_protocol::{OperationId, RunId};

use crate::{ConversationChannel, SessionConfig, SessionLifecycle};

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
    /// delivery may begin as soon as it is. The message resent under the
    /// same key keeps the first expectation: only the first is committed.
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
        self.lock().entry(key.to_owned()).or_insert(start);
        Expected {
            turns: self.clone(),
            key: key.to_owned(),
            token,
        }
    }

    /// The expectation of the message `trigger`, taken: the host hears its
    /// turn start once.
    fn take(&self, trigger: &RoomEvent) -> Option<Start> {
        let key = trigger.idempotency_key.as_ref()?;
        self.lock().remove(key)
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Start>> {
        self.starts.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ConversationChannel {
    /// Tell the host the turn `run`, answering `trigger`, starts, when it is
    /// the own turn of a message steered through this channel. The caller
    /// awaits it before the stream's first read. An interruption drops the
    /// host's start, as it drops a turn's preparation: Esc never waits for it.
    pub(crate) async fn start_own_turn(
        &self,
        trigger: Option<&RoomEvent>,
        room: &str,
        run: &RunId,
    ) {
        let Some(trigger) = trigger.filter(|row| &row.source.channel_id == self.id()) else {
            return;
        };
        let Some(start) = self.own_turns.take(trigger) else {
            return;
        };
        // Stopped before it began: no turn starts for the host.
        if self.stopping.contains(room, run) {
            return;
        }
        tokio::select! {
            biased;
            () = self.stopping.stopped(room, run) => {}
            () = start.lifecycle.started(&start.config, &start.operation) => {}
        }
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
        // Only its own expectation: a resent message's keeps the first one.
        if starts.get(&self.key).is_some_and(|s| s.token == self.token) {
            starts.remove(&self.key);
        }
    }
}
