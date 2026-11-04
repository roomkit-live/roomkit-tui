//! A model's reasoning as it comes (RMK-1325): a block of the view's own in the
//! turn's order. Nothing stores it, so the next snapshot drops it.

use std::collections::BTreeSet;
use std::time::Duration;

use roomkit_tui_protocol::{ActorId, CallId, ConversationId, Item, RunId, Thinking};

use super::Session;
use super::bounds::{bound_end_in_place, trim_items};
use super::unread::place;

/// One block's timing, as this view observed it.
#[derive(Debug)]
struct Thought {
    id: CallId,
    actor: ActorId,
    run: RunId,
    started_at: Option<Duration>,
    ended_at: Option<Duration>,
    open: bool,
}

/// The blocks this view follows: those its items still hold.
#[derive(Debug, Default)]
pub(super) struct Thoughts {
    blocks: Vec<Thought>,
    made: u64,
}

/// How a reasoning block reads now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThoughtState {
    /// The model is still writing it, for this long so far when known.
    Open(Option<Duration>),
    /// It ended after this long, when known.
    Ended(Option<Duration>),
}

impl Session {
    /// A piece of `run`'s reasoning goes in its open block, or opens one.
    pub(super) fn on_thinking_delta(
        &mut self,
        conversation: &ConversationId,
        (actor, run): (ActorId, &RunId),
        text: &str,
        at: Option<Duration>,
    ) {
        if !self.is_current(conversation) || !self.is_live(run) {
            return;
        }
        let open = self.open_thought(run);
        // Blanks open nothing, and say no stop passed.
        if open.is_none() && text.trim().is_empty() {
            return;
        }
        // Reasoning opens the model's next answer: it read what it held.
        self.text_comes(run);
        match open {
            Some(id) => self.extend_block(&id, text),
            None => self.open_block((actor, run), text, at),
        }
    }

    fn open_thought(&self, run: &RunId) -> Option<CallId> {
        self.thoughts
            .blocks
            .iter()
            .rev()
            .find(|thought| thought.open && thought.run == *run)
            .map(|thought| thought.id.clone())
    }

    /// Add `text` to the block `id`, which keeps its end.
    fn extend_block(&mut self, id: &CallId, text: &str) {
        let limit = self.limits.text_bytes.get();
        let items = self.snapshot.iter_mut().flat_map(|s| &mut s.items).rev();
        for item in items {
            if let Item::Thinking(block) = item
                && block.id == *id
            {
                block.text.push_str(text);
                bound_end_in_place(&mut block.text, limit);
                return;
            }
        }
    }

    /// Open `run`'s block above what the turn has not read.
    fn open_block(&mut self, (actor, run): (ActorId, &RunId), text: &str, at: Option<Duration>) {
        let limit = self.limits.text_bytes.get();
        let Some(snapshot) = &mut self.snapshot else {
            return;
        };
        self.thoughts.made += 1;
        let id = CallId::new(format!("thinking:{run}:{}", self.thoughts.made));
        self.thoughts.blocks.push(Thought {
            id: id.clone(),
            actor: actor.clone(),
            run: run.clone(),
            started_at: at,
            ended_at: None,
            open: true,
        });
        let mut kept = text.to_owned();
        bound_end_in_place(&mut kept, limit);
        let handed = self.handed.as_ref().filter(|handed| handed.run == *run);
        let block = Item::Thinking(Thinking::new(id, actor, run.clone(), kept));
        snapshot.items.insert(place(handed, &snapshot.items), block);
        if trim_items(&mut snapshot.items, self.limits.items.get()) {
            self.forget_missing_tools();
        }
    }

    /// `run` wrote something else: its reasoning ended.
    pub(super) fn run_thinking_ends(
        &mut self,
        conversation: &ConversationId,
        run: &RunId,
        at: Option<Duration>,
    ) {
        self.end_thoughts(conversation, at, |thought| thought.run == *run);
    }

    /// `actor` finished an answer: its reasoning ended.
    pub(super) fn actor_thinking_ends(
        &mut self,
        conversation: &ConversationId,
        actor: &ActorId,
        at: Option<Duration>,
    ) {
        self.end_thoughts(conversation, at, |thought| thought.actor == *actor);
    }

    fn end_thoughts(
        &mut self,
        conversation: &ConversationId,
        at: Option<Duration>,
        ends: impl Fn(&Thought) -> bool,
    ) {
        if !self.is_current(conversation) {
            return;
        }
        for thought in &mut self.thoughts.blocks {
            if thought.open && ends(thought) {
                thought.open = false;
                thought.ended_at = at;
            }
        }
    }

    /// Forget the timing of the blocks the view's items no longer hold.
    pub(super) fn forget_missing_thoughts(&mut self) {
        let items = self.snapshot.iter().flat_map(|snapshot| &snapshot.items);
        let held: BTreeSet<&CallId> = items
            .filter_map(|item| match item {
                Item::Thinking(block) => Some(&block.id),
                _ => None,
            })
            .collect();
        self.thoughts
            .blocks
            .retain(|thought| held.contains(&thought.id));
    }

    /// Forget every block's timing, as a replacement snapshot drops the blocks.
    pub(super) fn forget_thoughts(&mut self) {
        self.thoughts.blocks.clear();
    }

    /// How the block `block` reads at `now`.
    pub(crate) fn thought_state(&self, block: &Thinking, now: Duration) -> ThoughtState {
        let thought = self
            .thoughts
            .blocks
            .iter()
            .rev()
            .find(|thought| thought.id == block.id && thought.run == block.run);
        match thought {
            Some(thought) if thought.open => {
                ThoughtState::Open(thought.started_at.map(|start| now.saturating_sub(start)))
            }
            Some(thought) => ThoughtState::Ended(
                thought
                    .started_at
                    .zip(thought.ended_at)
                    .map(|(start, end)| end.saturating_sub(start)),
            ),
            None => ThoughtState::Ended(None),
        }
    }
}
