//! What a view attaching to a room is told beside its snapshot: a snapshot
//! holds what is stored, not the run under way nor what its current answer
//! has said so far. Kept under the lock events are published with, so a view
//! reads it at a known place among the events.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use roomkit_tui_protocol::limits::MAX_TEXT_BYTES;
use roomkit_tui_protocol::{ActorId, Event, MessageId, RunId};

/// The answer a run is writing.
#[derive(Clone, Debug)]
struct Draft {
    message: MessageId,
    actor: ActorId,
    text: String,
}

#[derive(Clone, Debug)]
struct LiveRun {
    run: RunId,
    draft: Option<Draft>,
}

/// The runs under way in each room.
#[derive(Clone, Debug, Default)]
pub(crate) struct LiveRuns(Arc<Mutex<BTreeMap<String, Vec<LiveRun>>>>);

impl LiveRuns {
    /// Held while an event is applied and published, or while a view reads
    /// these runs and sets its place among the events.
    pub(crate) fn lock(&self) -> Runs<'_> {
        Runs(self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

pub(crate) struct Runs<'a>(MutexGuard<'a, BTreeMap<String, Vec<LiveRun>>>);

impl Runs<'_> {
    /// Takes `event` into the runs of `room`.
    pub(crate) fn apply(&mut self, room: &str, event: &Event) {
        match event {
            Event::RunStarted { run, .. } => {
                let runs = self.0.entry(room.to_owned()).or_default();
                if !runs.iter().any(|live| live.run == *run) {
                    runs.push(LiveRun {
                        run: run.clone(),
                        draft: None,
                    });
                }
            }
            Event::TextDelta {
                message,
                actor,
                run,
                text,
                ..
            } => self.written(room, run, (message, actor), text),
            Event::MessageFinalized { draft, .. } => self.finalized(room, draft),
            Event::RunFinished { run, .. } => self.finished(room, run),
            _ => {}
        }
    }

    fn written(
        &mut self,
        room: &str,
        run: &RunId,
        (message, actor): (&MessageId, &ActorId),
        text: &str,
    ) {
        let runs = self.0.get_mut(room).into_iter().flatten();
        let Some(live) = runs.into_iter().find(|live| live.run == *run) else {
            return;
        };
        let draft = live.draft.get_or_insert_with(|| Draft {
            message: message.clone(),
            actor: actor.clone(),
            text: String::new(),
        });
        if draft.message != *message {
            draft.message = message.clone();
            draft.text.clear();
        }
        draft.text.push_str(text);
        keep_the_end(&mut draft.text);
    }

    fn finalized(&mut self, room: &str, finished: &MessageId) {
        for live in self.0.get_mut(room).into_iter().flatten() {
            if live.draft.as_ref().is_some_and(|d| d.message == *finished) {
                live.draft = None;
            }
        }
    }

    fn finished(&mut self, room: &str, run: &RunId) {
        if let Some(runs) = self.0.get_mut(room) {
            runs.retain(|live| live.run != *run);
            if runs.is_empty() {
                self.0.remove(room);
            }
        }
    }

    /// What a view attaching to `room` is told after its snapshot: each run
    /// under way, then what its current answer has said so far.
    pub(crate) fn catch_up(&self, room: &str) -> Vec<Event> {
        let mut events = Vec::new();
        for live in self.0.get(room).into_iter().flatten() {
            events.push(Event::RunStarted {
                conversation: room.into(),
                run: live.run.clone(),
                answers: None,
            });
            if let Some(draft) = &live.draft {
                events.push(Event::TextDelta {
                    conversation: room.into(),
                    message: draft.message.clone(),
                    actor: draft.actor.clone(),
                    run: live.run.clone(),
                    text: draft.text.clone(),
                });
            }
        }
        events
    }
}

/// Keeps the last [`MAX_TEXT_BYTES`] of `text`, what one event can carry.
fn keep_the_end(text: &mut String) {
    let Some(mut start) = text.len().checked_sub(MAX_TEXT_BYTES) else {
        return;
    };
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text.drain(..start);
}

#[cfg(test)]
mod tests {
    use super::*;
    use roomkit_tui_protocol::{Message, RunOutcome};

    fn started(run: &str) -> Event {
        Event::RunStarted {
            conversation: "room".into(),
            run: run.into(),
            answers: None,
        }
    }

    fn delta(run: &str, message: &str, text: &str) -> Event {
        Event::TextDelta {
            conversation: "room".into(),
            message: message.into(),
            actor: "agent".into(),
            run: run.into(),
            text: text.into(),
        }
    }

    fn text_of(events: &[Event]) -> Vec<(String, String)> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::TextDelta { message, text, .. } => {
                    Some((message.as_str().to_owned(), text.clone()))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_run_s_current_answer_is_told_whole_until_it_is_final() {
        let live = LiveRuns::default();
        let mut runs = live.lock();
        runs.apply("room", &started("r1"));
        runs.apply("room", &delta("r1", "d0", "Hel"));
        runs.apply("room", &delta("r1", "d0", "lo"));
        let told = runs.catch_up("room");
        assert!(matches!(&told[0], Event::RunStarted { run, .. } if run.as_str() == "r1"));
        assert_eq!(text_of(&told), [("d0".to_owned(), "Hello".to_owned())]);
        assert!(runs.catch_up("other").is_empty());

        // Its answer stored, the next one starts from nothing.
        let message = Message::new("m1", "agent", "Hello");
        runs.apply(
            "room",
            &Event::MessageFinalized {
                conversation: "room".into(),
                draft: "d0".into(),
                message,
            },
        );
        assert!(text_of(&runs.catch_up("room")).is_empty());
        runs.apply("room", &delta("r1", "d1", "Next"));
        assert_eq!(
            text_of(&runs.catch_up("room")),
            [("d1".to_owned(), "Next".to_owned())]
        );
    }

    #[test]
    fn a_finished_run_is_no_longer_told() {
        let live = LiveRuns::default();
        let mut runs = live.lock();
        runs.apply("room", &started("r1"));
        runs.apply(
            "room",
            &Event::RunFinished {
                conversation: "room".into(),
                run: "r1".into(),
                outcome: RunOutcome::Completed,
            },
        );
        assert!(runs.catch_up("room").is_empty());
    }

    /// An answer past what one event carries is told by its end, cut on a
    /// character.
    #[test]
    fn a_long_answer_keeps_its_end_within_one_event() {
        let mut text = "é".repeat(MAX_TEXT_BYTES);
        keep_the_end(&mut text);
        assert!(text.len() <= MAX_TEXT_BYTES && text.len() >= MAX_TEXT_BYTES - 1);
        assert!(text.chars().all(|c| c == 'é'));
    }
}
