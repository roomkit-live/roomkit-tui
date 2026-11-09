//! What a view shows of a call between its start and its end.
//!
//! RoomKit records a call's start and its end, nothing between: meanwhile
//! the call waits for its round's decisions, then executes with the round.
//! A view shows a started call of a running turn as running, and as pending
//! while a decision of that turn is unanswered. A refused or expired
//! decision keeps its round pending until each of its calls ends: a call is
//! never shown running when it may not run.

use std::collections::{BTreeMap, BTreeSet};

use roomkit_tui_protocol::{
    ActorId, CallId, ConversationId, Decision, Event, Item, PermissionId, RunId, ToolCall,
    ToolState,
};

/// One view's state of calls in flight. Events go through [`Self::apply`].
#[derive(Debug, Default)]
pub(crate) struct ToolProgress {
    conversation: ConversationId,
    /// Turns between their start and their end.
    running: BTreeSet<RunId>,
    /// Started calls of running turns, as the engine reported them.
    calls: BTreeMap<RunId, BTreeMap<(ActorId, CallId), Item>>,
    /// Unanswered decisions: their turn, and whether a person allowed them.
    decisions: BTreeMap<PermissionId, (RunId, bool)>,
    /// Turns whose current round had a decision refused or expired.
    held: BTreeSet<RunId>,
}

impl ToolProgress {
    /// Start over for a conversation's snapshot. A call a snapshot holds in
    /// flight was seen by no view of this one: its end still reaches it.
    pub fn new(conversation: ConversationId) -> Self {
        Self {
            conversation,
            ..Self::default()
        }
    }

    /// The events a view receives for one engine event: the event itself,
    /// a call's state as the view shows it, and calls whose shown state the
    /// event changed.
    pub fn apply(&mut self, mut event: Event) -> Vec<Event> {
        match &mut event {
            Event::RunStarted { run, .. } => {
                self.running.insert(run.clone());
            }
            Event::RunFinished { run, .. } => {
                let mut events = self.finish(run);
                events.push(event);
                return events;
            }
            Event::Item {
                item: Item::Tool { actor, run, call },
                ..
            } => self.call(actor, run, call),
            Event::Item {
                item: Item::Approval(record),
                ..
            } if record.decision == Decision::Allow => {
                if let Some((_, allowed)) = self.decisions.get_mut(&record.permission.id) {
                    *allowed = true;
                }
            }
            Event::PermissionRequested { permission, .. } => {
                let run = permission.run.clone();
                let first = !self.waiting(&run);
                self.decisions
                    .insert(permission.id.clone(), (run.clone(), false));
                if first {
                    return std::iter::once(event)
                        .chain(self.show(&run, ToolState::Pending))
                        .collect();
                }
            }
            Event::PermissionResolved { permission, .. } => {
                let shown = self.resolved(permission);
                return std::iter::once(event).chain(shown).collect();
            }
            _ => {}
        }
        vec![event]
    }

    fn waiting(&self, run: &RunId) -> bool {
        self.decisions.values().any(|(waiting, _)| waiting == run)
    }

    /// Track a started call of a running turn and show it as the turn's
    /// decisions allow; forget an ended one.
    fn call(&mut self, actor: &ActorId, run: &RunId, call: &mut ToolCall) {
        let key = (actor.clone(), call.id.clone());
        if call.state != ToolState::Pending {
            if let Some(calls) = self.calls.get_mut(run) {
                calls.remove(&key);
                // A refusal holds its round only; the turn's next round runs.
                if calls.is_empty() {
                    self.held.remove(run);
                }
            }
            return;
        }
        if !self.running.contains(run) {
            return;
        }
        let item = Item::Tool {
            actor: actor.clone(),
            run: run.clone(),
            call: call.clone(),
        };
        self.calls.entry(run.clone()).or_default().insert(key, item);
        if !self.waiting(run) && !self.held.contains(run) {
            call.state = ToolState::Running;
        }
    }

    /// Once a turn's last decision is answered, its round runs, unless one
    /// of its decisions was refused or expired.
    fn resolved(&mut self, permission: &PermissionId) -> Vec<Event> {
        let Some((run, allowed)) = self.decisions.remove(permission) else {
            return Vec::new();
        };
        if !allowed {
            self.held.insert(run.clone());
        }
        if self.waiting(&run) || self.held.contains(&run) {
            return Vec::new();
        }
        self.show(&run, ToolState::Running)
    }

    /// A finished turn leaves no call shown running.
    fn finish(&mut self, run: &RunId) -> Vec<Event> {
        self.running.remove(run);
        self.held.remove(run);
        self.decisions.retain(|_, (waiting, _)| waiting != run);
        let held = self.calls.remove(run).unwrap_or_default();
        held.into_values().map(|item| self.item(item)).collect()
    }

    /// The turn's started calls, shown in `state`.
    fn show(&self, run: &RunId, state: ToolState) -> Vec<Event> {
        let Some(calls) = self.calls.get(run) else {
            return Vec::new();
        };
        calls
            .values()
            .cloned()
            .map(|mut item| {
                if let Item::Tool { call, .. } = &mut item {
                    call.state = state;
                }
                self.item(item)
            })
            .collect()
    }

    fn item(&self, item: Item) -> Event {
        Event::Item {
            conversation: self.conversation.clone(),
            item,
        }
    }
}

#[cfg(test)]
mod tests;
