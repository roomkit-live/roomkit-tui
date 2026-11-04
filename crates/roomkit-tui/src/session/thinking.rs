//! A model's reasoning as it comes (RMK-1325): a block of the view's own in the
//! turn's order. Nothing stores it, so the next snapshot drops it.

use std::time::Duration;

use roomkit_tui_protocol::{ActorId, CallId, ConversationId, Item, RunId, Thinking};

use super::Session;
use super::bounds::{bound_in_place, bounded, trim_items};
use super::steering::remember;
use super::unread::place;

/// One block's timing, as this view observed it.
#[derive(Debug)]
struct Thought {
    id: CallId,
    run: RunId,
    started_at: Option<Duration>,
    ended_at: Option<Duration>,
    open: bool,
}

/// The blocks this view follows. Beyond the bound of `remember` the oldest
/// forgets its timing and reads as finished.
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
    pub(super) fn on_thinking_delta(
        &mut self,
        conversation: &ConversationId,
        (actor, run): (ActorId, &RunId),
        text: &str,
        at: Option<Duration>,
    ) {
        if !self.is_current(conversation)
            || (self.last_run.as_ref() != Some(run) && !self.is_live(run))
        {
            return;
        }
        // Reasoning opens the model's next answer: it read what it held.
        self.text_comes(run);
        let limit = self.limits.text_bytes.get();
        let open = self
            .thoughts
            .blocks
            .iter()
            .rev()
            .find(|thought| thought.open && thought.run == *run)
            .map(|thought| thought.id.clone());
        let Some(snapshot) = &mut self.snapshot else {
            return;
        };
        if let Some(id) = open {
            let block = snapshot.items.iter_mut().rev().find_map(|item| match item {
                Item::Thinking(block) if block.id == id && block.run == *run => Some(block),
                _ => None,
            });
            if let Some(block) = block {
                if block.text.len() < limit {
                    block.text.push_str(text);
                    bound_in_place(&mut block.text, limit);
                }
                return;
            }
        }
        self.thoughts.made += 1;
        let id = CallId::new(format!("thinking:{run}:{}", self.thoughts.made));
        remember(
            &mut self.thoughts.blocks,
            Thought {
                id: id.clone(),
                run: run.clone(),
                started_at: at,
                ended_at: None,
                open: true,
            },
        );
        let handed = self.handed.as_ref().filter(|handed| handed.run == *run);
        let block = Item::Thinking(Thinking::new(id, actor, run.clone(), bounded(text, limit)));
        snapshot.items.insert(place(handed, &snapshot.items), block);
        if trim_items(&mut snapshot.items, self.limits.items.get()) {
            self.forget_missing_tools();
        }
    }

    /// `run`, or any run when it is not named, wrote something else: its
    /// reasoning ended.
    pub(super) fn thinking_ends(
        &mut self,
        conversation: &ConversationId,
        run: Option<&RunId>,
        at: Option<Duration>,
    ) {
        if !self.is_current(conversation) {
            return;
        }
        for thought in &mut self.thoughts.blocks {
            if thought.open && run.is_none_or(|run| thought.run == *run) {
                thought.open = false;
                thought.ended_at = at;
            }
        }
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
