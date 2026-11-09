//! Guards that withdraw pending decisions when a view, a run or a waiting
//! action goes away. A withdrawal is never recorded as a human refusal.

use roomkit_tui_protocol::{Decision, PermissionId};

use super::{Approvals, Scope};

pub(crate) struct View {
    pub(super) broker: Approvals,
    pub(super) scope: Scope,
    pub(super) group: String,
}

impl Drop for View {
    fn drop(&mut self) {
        let mut state = self.broker.lock();
        if let Some(count) = state.views.get_mut(&self.scope) {
            *count -= 1;
            if *count == 0 {
                state.views.remove(&self.scope);
                self.broker.withdraw(&mut state, &self.scope, None);
                let room = &self.scope.0;
                if !state.views.keys().any(|(watched, _)| watched == room) {
                    state.last_runs.remove(room);
                }
            }
        }
        if let Some(count) = state.groups.get_mut(&self.group) {
            *count -= 1;
            if *count == 0 {
                state.groups.remove(&self.group);
                self.broker.withdraw_group(&mut state, &self.group);
            }
        }
    }
}

pub(crate) struct Running {
    pub(super) broker: Approvals,
    pub(super) scope: Scope,
    pub(super) token: u64,
}

impl Running {
    /// The token naming this start of the run in requests.
    pub(crate) fn token(&self) -> u64 {
        self.token
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let mut state = self.broker.lock();
        // The run before it, if still live, receives the scope's requests again.
        if let Some(runs) = state.runs.get_mut(&self.scope) {
            runs.retain(|r| r.token != self.token);
            if runs.is_empty() {
                state.runs.remove(&self.scope);
            }
        }
        self.broker
            .withdraw(&mut state, &self.scope, Some(self.token));
    }
}

pub(crate) struct Waiting {
    pub(super) broker: Approvals,
    pub(super) id: PermissionId,
}

impl Drop for Waiting {
    fn drop(&mut self) {
        let mut state = self.broker.lock();
        let room = state
            .pending
            .get(&self.id)
            .map(super::Pending::published_in);
        self.broker.settle(&mut state, &self.id, Decision::Deny);
        drop(state);
        if let Some(room) = room {
            self.broker.resolved(&room, &self.id);
        }
    }
}
