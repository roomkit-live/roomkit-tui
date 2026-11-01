//! Ephemeral decisions tied to a live, authorized view and an exact run.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use roomkit_tui_protocol::limits::MAX_PENDING_PERMISSIONS;
use roomkit_tui_protocol::{
    ActorId, Decision, Event, Item, Permission, PermissionDecision, PermissionId, Rejection, RunId,
};
use tokio::sync::{Notify, broadcast, oneshot};

/// Decisions recorded but not yet stored before new requests are refused.
const MAX_UNSTORED_RECEIPTS: usize = 512;

type Scope = (String, String);

mod guards;
mod request;

pub(crate) use guards::{Running, View, Waiting};
pub(crate) use request::Details;
pub use request::PermissionRequest;

#[derive(Debug, Default)]
struct State {
    views: BTreeMap<Scope, usize>,
    runs: BTreeMap<Scope, Run>,
    pending: BTreeMap<PermissionId, Pending>,
    sequence: u64,
    receipts: BTreeMap<String, Vec<Receipt>>,
}

#[derive(Clone, Debug)]
struct Run {
    id: RunId,
    /// The person who started the run through a view; only they may answer.
    requester: Option<String>,
    /// Refusals already announced during this run, said once each.
    announced: BTreeSet<&'static str>,
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
        let scope = (room.into(), agent.into());
        let record = Run {
            id: run.clone(),
            requester,
            announced: BTreeSet::new(),
        };
        self.lock().runs.insert(scope.clone(), record);
        self.ready.notify_waiters();
        Running {
            broker: self.clone(),
            scope,
            run,
        }
    }

    /// Wait for one decision. Missing views, oversized details, timeout and
    /// cancellation all refuse.
    pub async fn ask(
        &self,
        room: &str,
        agent: &str,
        details: Details,
        timeout: Duration,
    ) -> Decision {
        if timeout.is_zero() {
            return Decision::Deny;
        }
        let scope = (room.to_owned(), agent.to_owned());
        let wait = async {
            loop {
                // Engine generation can reach its hook before delivery starts.
                let ready = self.ready.notified();
                tokio::pin!(ready);
                ready.as_mut().enable();
                match self.register(&scope, &details) {
                    Registration::Refused => return Decision::Deny,
                    Registration::Waiting(_guard, answer) => {
                        return answer.await.unwrap_or(Decision::Deny);
                    }
                    Registration::NoRun => ready.await,
                }
            }
        };
        tokio::time::timeout(timeout, wait)
            .await
            .unwrap_or(Decision::Deny)
    }

    fn register(&self, scope: &Scope, details: &Details) -> Registration {
        let mut state = self.lock();
        let (room, agent) = scope;
        if !state.views.contains_key(scope) {
            return Registration::Refused;
        }
        // A refusal is told once the run has started: a terminal clears its
        // notice when the run starts.
        let Some(run) = state.runs.get(scope).cloned() else {
            return Registration::NoRun;
        };
        if let Some(reason) = refusal(&state, room) {
            return self.refuse(&mut state, scope, reason);
        }
        let Some(sequence) = state.sequence.checked_add(1) else {
            return Registration::Refused;
        };
        state.sequence = sequence;
        let Some(permission) = details.permission(&run.id, agent, sequence) else {
            let reason = "An approval was too large to show in full; it was refused.";
            return self.refuse(&mut state, scope, reason);
        };
        let pending = (run, permission, details.tool_call_id.clone());
        self.enqueue(&mut state, scope, pending)
    }

    /// Hold `permission` until it is answered, and show it to the views.
    fn enqueue(
        &self,
        state: &mut State,
        scope: &Scope,
        (run, permission, tool_call_id): (Run, Permission, Option<String>),
    ) -> Registration {
        let id = permission.id.clone();
        let (answer, receiver) = oneshot::channel();
        let pending = Pending {
            scope: scope.clone(),
            run,
            answer,
            permission: permission.clone(),
            tool_call_id,
        };
        state.pending.insert(id.clone(), pending);
        let room = &scope.0;
        let conversation = room.as_str().into();
        self.publish(
            room,
            Event::PermissionRequested {
                conversation,
                permission,
            },
        );
        let guard = Waiting {
            broker: self.clone(),
            id,
        };
        Registration::Waiting(guard, receiver)
    }

    /// Refuse a request the person will never see, and tell them why, once
    /// per run and reason: the action reports a denial they did not make, and
    /// a host asking again must not flood the view.
    fn refuse(&self, state: &mut State, scope: &Scope, reason: &'static str) -> Registration {
        let first = state
            .runs
            .get_mut(scope)
            .is_some_and(|run| run.announced.insert(reason));
        if first {
            self.publish(&scope.0, notice(&scope.0, reason));
        }
        Registration::Refused
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

    fn withdraw(&self, state: &mut State, scope: &Scope, run: Option<&RunId>) {
        let ids: Vec<_> = state
            .pending
            .iter()
            .filter(|(_, p)| &p.scope == scope && run.is_none_or(|run| run == &p.run.id))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.settle(state, &id, Decision::Deny);
            self.resolved(&scope.0, &id);
        }
        self.ready.notify_waiters();
    }
}

/// Why no request of `room` can wait now, although a view and a run are
/// there. The pending limit is shared by every room of the channel.
fn refusal(state: &State, room: &str) -> Option<&'static str> {
    if state.pending.len() >= MAX_PENDING_PERMISSIONS {
        Some("Too many approvals are waiting; a new one was refused.")
    } else if state
        .receipts
        .get(room)
        .is_some_and(|r| r.len() >= MAX_UNSTORED_RECEIPTS)
    {
        Some("Approval history cannot be stored; new approvals are refused until it is.")
    } else {
        None
    }
}

enum Registration {
    Refused,
    NoRun,
    Waiting(Waiting, oneshot::Receiver<Decision>),
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
