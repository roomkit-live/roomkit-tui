//! Answering a pending request.

use roomkit_tui_protocol::{
    ActorId, Decision, Event, Item, PermissionDecision, PermissionId, Rejection,
};

use super::{Approvals, Pending, Receipt};

impl Approvals {
    /// Answer one pending request. The decision is recorded only once the
    /// waiting action has received it.
    #[cfg(test)]
    pub fn decide(
        &self,
        scope: (&str, &str),
        id: &PermissionId,
        decision: Decision,
        sender: &str,
    ) -> Result<(), Rejection> {
        self.decide_in(scope, scope.0, id, decision, sender)
            .map(drop)
    }

    /// [`Self::decide`] from a view in `group`, which also answers what is
    /// asked in the group's other rooms: the room it was asked in.
    pub fn decide_in(
        &self,
        scope: (&str, &str),
        group: &str,
        id: &PermissionId,
        decision: Decision,
        sender: &str,
    ) -> Result<String, Rejection> {
        let mut state = self.lock();
        let Some(pending) = state.pending.get(id) else {
            return Err(Rejection::unavailable("This approval is no longer pending"));
        };
        may_answer(pending, scope, group, sender)?;
        let (room, published) = (pending.scope.0.clone(), pending.published_in());
        let settled = self.settle(&mut state, id, decision);
        let Some(settled) = settled.filter(|settled| settled.delivered) else {
            drop(state);
            self.resolved(&published, id);
            return Err(Rejection::unavailable("This approval has expired"));
        };
        let decided_by = ActorId::new(format!("person:{sender}"));
        let record = PermissionDecision::new(settled.permission, decision, decided_by);
        let receipt = Receipt {
            decision: record.clone(),
            tool_call_id: settled.tool_call_id,
        };
        state
            .receipts
            .entry(room.clone())
            .or_default()
            .push(receipt);
        drop(state);
        // Publish only explicit, delivered answers. Timeout and withdrawal
        // settle directly and must never look like a human refusal.
        let item = Item::Approval(record);
        self.publish(
            &room,
            Event::Item {
                conversation: room.as_str().into(),
                item,
            },
        );
        self.resolved(&published, id);
        Ok(room)
    }
}

/// Why this view's `sender` may not answer `pending`, if they may not.
fn may_answer(
    pending: &Pending,
    (room, agent): (&str, &str),
    group: &str,
    sender: &str,
) -> Result<(), Rejection> {
    let own = pending.scope == (room.into(), agent.into());
    if !own && pending.group.as_deref() != Some(group) {
        return Err(Rejection::invalid("Approval belongs to another view"));
    }
    if pending
        .run
        .requester
        .as_deref()
        .is_some_and(|requester| requester != sender)
    {
        return Err(Rejection::refused(
            "Only the person who started this request can answer it",
        ));
    }
    Ok(())
}
