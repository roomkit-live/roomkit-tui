//! Admitting a request: it waits only with a live view and a run, and is
//! refused, with a notice, when the person could never see it.

use std::time::Duration;

use roomkit_tui_protocol::limits::MAX_PENDING_PERMISSIONS;
use roomkit_tui_protocol::{Decision, Event, Permission};
use tokio::sync::oneshot;

use super::{
    Approvals, Details, MAX_UNSTORED_RECEIPTS, Pending, Run, Scope, State, Waiting, notice,
};

impl Approvals {
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

    pub(super) fn register(&self, scope: &Scope, details: &Details) -> Registration {
        let mut state = self.lock();
        let (room, agent) = scope;
        if !state.views.contains_key(scope) {
            return Registration::Refused;
        }
        // A refusal is told once the run has started: a terminal clears its
        // notice when the run starts.
        let Some(run) = state.run(scope, details.run).cloned() else {
            // A named run is registered before its work starts: gone, it ended.
            return match details.run {
                Some(_) => Registration::Refused,
                None => Registration::NoRun,
            };
        };
        if run.stopped {
            return Registration::Refused;
        }
        if let Some(reason) = refusal(&state, room) {
            return self.refuse(&mut state, scope, run.token, reason);
        }
        let Some(sequence) = state.sequence.checked_add(1) else {
            return Registration::Refused;
        };
        state.sequence = sequence;
        let Some(permission) = details.permission(&run.id, agent, sequence) else {
            let reason = "An approval was too large to show in full; it was refused.";
            return self.refuse(&mut state, scope, run.token, reason);
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
    fn refuse(
        &self,
        state: &mut State,
        scope: &Scope,
        token: u64,
        reason: &'static str,
    ) -> Registration {
        let first = state
            .run_mut(scope, token)
            .is_some_and(|run| run.announced.insert(reason));
        if first {
            self.publish(&scope.0, notice(&scope.0, reason));
        }
        Registration::Refused
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

pub(super) enum Registration {
    Refused,
    NoRun,
    Waiting(Waiting, oneshot::Receiver<Decision>),
}
