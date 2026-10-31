//! Guards that withdraw pending decisions when a view, a run or a waiting
//! action goes away. A withdrawal is never recorded as a human refusal.

use roomkit_tui_protocol::{Decision, PermissionId, RunId};

use super::{Approvals, Scope};

pub(crate) struct View {
    pub(super) broker: Approvals,
    pub(super) scope: Scope,
}

impl Drop for View {
    fn drop(&mut self) {
        let mut state = self.broker.lock();
        if let Some(count) = state.views.get_mut(&self.scope) {
            *count -= 1;
            if *count == 0 {
                state.views.remove(&self.scope);
                self.broker.withdraw(&mut state, &self.scope, None);
            }
        }
    }
}

pub(crate) struct Running {
    pub(super) broker: Approvals,
    pub(super) scope: Scope,
    pub(super) run: RunId,
}

impl Drop for Running {
    fn drop(&mut self) {
        let mut state = self.broker.lock();
        if state
            .runs
            .get(&self.scope)
            .is_some_and(|r| r.id == self.run)
        {
            state.runs.remove(&self.scope);
        }
        self.broker
            .withdraw(&mut state, &self.scope, Some(&self.run));
    }
}

pub(crate) struct Waiting {
    pub(super) broker: Approvals,
    pub(super) id: PermissionId,
}

impl Drop for Waiting {
    fn drop(&mut self) {
        let mut state = self.broker.lock();
        let room = state.pending.get(&self.id).map(|p| p.scope.0.clone());
        self.broker.settle(&mut state, &self.id, Decision::Deny);
        drop(state);
        if let Some(room) = room {
            self.broker.resolved(&room, &self.id);
        }
    }
}
