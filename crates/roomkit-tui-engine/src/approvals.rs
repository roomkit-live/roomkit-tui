//! Ephemeral decisions tied to a live, authorized view and an exact run.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use roomkit_tui_protocol::{
    ActorId, Decision, Event, Item, Permission, PermissionDecision, PermissionId, Rejection, RunId,
};
use tokio::sync::{Notify, broadcast, oneshot};

/// Decisions recorded but not yet stored before new requests are refused.
const MAX_UNSTORED_RECEIPTS: usize = 512;

type Scope = (String, String);

mod admission;
mod guards;
mod request;

pub(crate) use guards::{Running, View, Waiting};
pub(crate) use request::Details;
pub use request::PermissionRequest;

#[derive(Debug, Default)]
struct State {
    views: BTreeMap<Scope, usize>,
    /// Live runs of each scope, oldest first: a turn delivered for another
    /// channel's message can start while host work still runs.
    runs: BTreeMap<Scope, Vec<Run>>,
    /// Runs started so far: each start gets its own token, as a host may
    /// reuse a run identity once the run has ended.
    started: u64,
    pending: BTreeMap<PermissionId, Pending>,
    sequence: u64,
    /// The last run announced in each room a view watches, and in the room
    /// announced last: a terminal ignores a run that repeats it. A view
    /// attaching to a room nobody watched starts afresh.
    last_runs: BTreeMap<String, RunId>,
    receipts: BTreeMap<String, Vec<Receipt>>,
}

impl State {
    /// The run a request of `scope` belongs to: the one whose start returned
    /// `token` when the request names it, otherwise the most recent one.
    fn run(&self, scope: &Scope, token: Option<u64>) -> Option<&Run> {
        let runs = self.runs.get(scope)?;
        match token {
            Some(token) => runs.iter().find(|r| r.token == token),
            None => runs.last(),
        }
    }

    fn run_mut(&mut self, scope: &Scope, token: u64) -> Option<&mut Run> {
        let runs = self.runs.get_mut(scope)?;
        runs.iter_mut().find(|r| r.token == token)
    }
}

#[derive(Clone, Debug)]
struct Run {
    id: RunId,
    /// This start of the run, unique for the broker's lifetime.
    token: u64,
    /// The person who started the run through a view; only they may answer.
    requester: Option<String>,
    /// Refusals already announced during this run, said once each.
    announced: BTreeSet<&'static str>,
    /// The person interrupted the run: it may ask nothing more.
    stopped: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Receipt {
    pub decision: PermissionDecision,
    pub tool_call_id: Option<String>,
}

#[derive(Debug)]
struct Pending {
    scope: Scope,
    run: Run,
    answer: oneshot::Sender<Decision>,
    permission: Permission,
    tool_call_id: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct Approvals {
    state: Arc<Mutex<State>>,
    ready: Arc<Notify>,
    events: broadcast::Sender<(String, Event)>,
}

impl Approvals {
    pub fn new(events: broadcast::Sender<(String, Event)>) -> Self {
        Self {
            state: Arc::default(),
            ready: Arc::default(),
            events,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn publish(&self, room: &str, event: Event) {
        let _ = self.events.send((room.into(), event));
    }

    pub fn attach(&self, room: &str, agent: &str) -> View {
        let scope = (room.into(), agent.into());
        *self.lock().views.entry(scope.clone()).or_default() += 1;
        View {
            broker: self.clone(),
            scope,
        }
    }

    pub fn start(&self, room: &str, agent: &str, run: RunId, requester: Option<String>) -> Running {
        let scope: Scope = (room.into(), agent.into());
        let mut state = self.lock();
        state.started += 1;
        let token = state.started;
        let record = Run {
            id: run,
            token,
            requester,
            announced: BTreeSet::new(),
            stopped: false,
        };
        state.runs.entry(scope.clone()).or_default().push(record);
        drop(state);
        self.ready.notify_waiters();
        Running {
            broker: self.clone(),
            scope,
            token,
        }
    }

    /// Answer one pending request. The decision is recorded only once the
    /// waiting action has received it.
    pub fn decide(
        &self,
        (room, agent): (&str, &str),
        id: &PermissionId,
        decision: Decision,
        sender: &str,
    ) -> Result<(), Rejection> {
        let mut state = self.lock();
        let Some(pending) = state.pending.get(id) else {
            return Err(Rejection::unavailable("This approval is no longer pending"));
        };
        if pending.scope != (room.into(), agent.into()) {
            return Err(Rejection::invalid("Approval belongs to another view"));
        }
        if pending
            .run
            .requester
            .as_deref()
            .is_some_and(|r| r != sender)
        {
            return Err(Rejection::refused(
                "Only the person who started this request can answer it",
            ));
        }
        let settled = self.settle(&mut state, id, decision);
        let Some(settled) = settled.filter(|settled| settled.delivered) else {
            drop(state);
            self.resolved(room, id);
            return Err(Rejection::unavailable("This approval has expired"));
        };
        let decided_by = ActorId::new(format!("person:{sender}"));
        let record = PermissionDecision::new(settled.permission, decision, decided_by);
        let receipt = Receipt {
            decision: record.clone(),
            tool_call_id: settled.tool_call_id,
        };
        state.receipts.entry(room.into()).or_default().push(receipt);
        drop(state);
        // Publish only explicit, delivered answers. Timeout and withdrawal
        // settle directly and must never look like a human refusal.
        let item = Item::Approval(record);
        self.publish(
            room,
            Event::Item {
                conversation: room.into(),
                item,
            },
        );
        self.resolved(room, id);
        Ok(())
    }

    pub fn receipts(&self, room: &str) -> Vec<Receipt> {
        self.lock().receipts.get(room).cloned().unwrap_or_default()
    }

    pub fn recorded(&self, room: &str, id: &PermissionId) {
        let mut state = self.lock();
        if let Some(receipts) = state.receipts.get_mut(room) {
            receipts.retain(|r| &r.decision.permission.id != id);
            if receipts.is_empty() {
                state.receipts.remove(room);
            }
        }
    }

    /// Remove a request and hand its answer to the waiting action. Callers
    /// publish its resolution; a broadcast send never waits on a subscriber.
    fn settle(&self, state: &mut State, id: &PermissionId, decision: Decision) -> Option<Settled> {
        let pending = state.pending.remove(id)?;
        Some(Settled {
            delivered: pending.answer.send(decision).is_ok(),
            permission: pending.permission,
            tool_call_id: pending.tool_call_id,
        })
    }

    fn resolved(&self, room: &str, id: &PermissionId) {
        let permission = id.clone();
        self.publish(
            room,
            Event::PermissionResolved {
                conversation: room.into(),
                permission,
            },
        );
    }

    /// Note the run just announced in `room`. A view about to attach may
    /// already receive it, so it is kept even before one does; other rooms
    /// no view watches are forgotten, which keeps one entry per watched room.
    pub(crate) fn announced(&self, room: &str, run: &RunId) {
        let mut state = self.lock();
        let watched: BTreeSet<String> = state.views.keys().map(|(r, _)| r.clone()).collect();
        state
            .last_runs
            .retain(|kept, _| kept == room || watched.contains(kept));
        state.last_runs.insert(room.to_owned(), run.clone());
    }

    /// The last run announced in `room`, while a view watches it.
    pub(crate) fn last_run(&self, room: &str) -> Option<RunId> {
        self.lock().last_runs.get(room).cloned()
    }

    /// The person interrupted the run started with `token` in
    /// `(room, agent)`: deny its pending requests, and every request it asks
    /// from now on.
    pub(crate) fn stop_run(&self, scope: &(String, String), token: u64) {
        let mut state = self.lock();
        if let Some(record) = state.run_mut(scope, token) {
            record.stopped = true;
        }
        self.withdraw(&mut state, scope, Some(token));
    }

    /// Deny the pending requests of `scope`, or of its run started with `token`.
    fn withdraw(&self, state: &mut State, scope: &Scope, token: Option<u64>) {
        let ids: Vec<_> = state
            .pending
            .iter()
            .filter(|(_, p)| &p.scope == scope && token.is_none_or(|t| t == p.run.token))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.settle(state, &id, Decision::Deny);
            self.resolved(&scope.0, &id);
        }
        self.ready.notify_waiters();
    }
}

/// A removed request: whether its action received the answer, and what a
/// receipt keeps of it.
struct Settled {
    delivered: bool,
    permission: Permission,
    tool_call_id: Option<String>,
}

fn notice(room: &str, text: &str) -> Event {
    Event::Notice {
        conversation: room.into(),
        text: text.into(),
    }
}

#[cfg(test)]
mod tests;
