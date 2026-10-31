//! Ephemeral decisions tied to a live, authorized view and an exact run.

use std::collections::BTreeMap;
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
        if !details.fits() || timeout.is_zero() {
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
        if !state.views.contains_key(scope) || state.pending.len() >= MAX_PENDING_PERMISSIONS {
            return Registration::Refused;
        }
        if state
            .receipts
            .get(room)
            .is_some_and(|r| r.len() >= MAX_UNSTORED_RECEIPTS)
        {
            drop(state);
            let text = "Approval history cannot be stored; new approvals are refused until it is.";
            self.publish(room, notice(room, text));
            return Registration::Refused;
        }
        let Some(run) = state.runs.get(scope).cloned() else {
            return Registration::NoRun;
        };
        let Some(sequence) = state.sequence.checked_add(1) else {
            return Registration::Refused;
        };
        state.sequence = sequence;
        let id = PermissionId::new(format!("{}:approval:{sequence}", run.id));
        let mut permission = Permission::new(
            id.clone(),
            agent.as_str(),
            run.id.clone(),
            details.title.clone(),
            details.detail.clone(),
        );
        permission.summary.clone_from(&details.summary);
        let (answer, receiver) = oneshot::channel();
        let pending = Pending {
            scope: scope.clone(),
            run,
            answer,
            permission: permission.clone(),
            tool_call_id: details.tool_call_id.clone(),
        };
        state.pending.insert(id.clone(), pending);
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

    /// Remove a request and hand its answer to the waiting action.
    /// Callers publish its resolution once the lock is released.
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
