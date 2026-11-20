//! Questions an agent asks the person: a human-input request of the kit
//! (RFC §9.3), shown to the views of its room and answered from them. The
//! application turns the request's arguments into questions; the engine
//! carries them, takes the person's answer back to the kit as the text the
//! agent reads, and withdraws a question once the call that asked it ends.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use roomkit_tui_protocol::limits::MAX_PENDING_QUESTIONS;
use roomkit_tui_protocol::{Event, Item, Question, QuestionAnswer, QuestionId, QuestionItem};

/// What the agent reads when the person closes its questions unanswered.
pub(crate) const DECLINED: &str = "The person closed the question without answering. \
    Continue with your best judgement, say which assumptions you make, and do not ask it again.";

/// Questions for [`ConversationChannel::ask_question`](crate::ConversationChannel::ask_question).
#[derive(Clone, Debug)]
pub struct QuestionRequest {
    pub(crate) id: String,
    pub(crate) items: Vec<QuestionItem>,
    pub(crate) tool_call_id: Option<String>,
}

impl QuestionRequest {
    /// The questions `items` of the kit's human-input request `id`
    /// (`PendingInputEvent::pending_id`), which the person's answer resolves.
    pub fn new(id: impl Into<String>, items: Vec<QuestionItem>) -> Self {
        Self {
            id: id.into(),
            items,
            tool_call_id: None,
        }
    }

    /// The model's call that asks (`PendingInputEvent::tool_call_id`): its
    /// end withdraws the questions from the views, a timeout or a cut turn
    /// included.
    #[must_use]
    pub fn with_tool_call(mut self, id: impl Into<String>) -> Self {
        self.tool_call_id = Some(id.into());
        self
    }
}

/// A question shown, and the call whose end withdraws it.
#[derive(Clone, Debug)]
struct Asked {
    question: Question,
    call: Option<String>,
}

/// Why a question could not be taken to answer it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Untaken {
    /// It is no longer asked: answered, closed or withdrawn.
    Gone,
    /// The answers given do not fit it; it stays asked.
    Unfit,
}

/// The questions waiting for the person, by room.
#[derive(Clone, Debug, Default)]
pub(crate) struct Questions(Arc<Mutex<BTreeMap<String, Vec<Asked>>>>);

impl Questions {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Vec<Asked>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Wait for the answer to `question`, unless as many questions as a
    /// terminal holds already wait in `room`, whoever asks them: checked and
    /// added under one lock, so questions asked at once never pass it.
    pub(crate) fn add(&self, room: &str, question: Question, call: Option<String>) -> bool {
        let mut rooms = self.lock();
        let waiting = rooms.entry(room.to_owned()).or_default();
        if waiting.len() >= MAX_PENDING_QUESTIONS {
            return false;
        }
        waiting.push(Asked { question, call });
        true
    }

    /// The question `id` that `agent` asks in `room`, no longer waiting,
    /// when `fits` accepts it: checked and taken under one lock, so two views
    /// answering at once cannot both take it. As for a decision, only a view
    /// of that room and agent answers it.
    pub(crate) fn take_if(
        &self,
        (room, agent): (&str, &str),
        id: &QuestionId,
        fits: impl FnOnce(&Question) -> bool,
    ) -> Result<Question, Untaken> {
        let mut rooms = self.lock();
        let waiting = rooms.get_mut(room).ok_or(Untaken::Gone)?;
        let at = waiting
            .iter()
            .position(|asked| &asked.question.id == id && asked.question.actor.as_str() == agent)
            .ok_or(Untaken::Gone)?;
        if !fits(&waiting[at].question) {
            return Err(Untaken::Unfit);
        }
        let asked = waiting.remove(at);
        if waiting.is_empty() {
            rooms.remove(room);
        }
        Ok(asked.question)
    }

    /// How many questions wait in `room`, whoever asks them.
    #[cfg(test)]
    pub(crate) fn waiting(&self, room: &str) -> usize {
        self.lock().get(room).map_or(0, Vec::len)
    }

    /// The questions `agent` asks in `room`, in the order asked.
    pub(crate) fn pending(&self, (room, agent): (&str, &str)) -> Vec<Question> {
        self.lock()
            .get(room)
            .map(|waiting| {
                waiting
                    .iter()
                    .filter(|asked| asked.question.actor.as_str() == agent)
                    .map(|asked| asked.question.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The questions of `room` that `event` ends, no longer waiting: those
    /// of a run that finished, and the one a call of its run that settled
    /// asked; another run's call may reuse its identity.
    pub(crate) fn ended_by(&self, room: &str, event: &Event) -> Vec<QuestionId> {
        let ends = |asked: &Asked| match event {
            Event::RunFinished { run, .. } => &asked.question.run == run,
            Event::Item {
                item: Item::Tool { run, call, .. },
                ..
            } => {
                call.state.is_settled()
                    && &asked.question.run == run
                    && asked.call.as_deref() == Some(call.id.as_str())
            }
            _ => false,
        };
        let mut rooms = self.lock();
        let Some(waiting) = rooms.get_mut(room) else {
            return Vec::new();
        };
        let (ended, kept): (Vec<Asked>, Vec<Asked>) = waiting.drain(..).partition(ends);
        *waiting = kept;
        if waiting.is_empty() {
            rooms.remove(room);
        }
        ended.into_iter().map(|asked| asked.question.id).collect()
    }
}

/// What the asking agent reads of `answers` to `question`: a line per
/// question, its header (else its text) and what the person picked or wrote.
pub(crate) fn answer_text(question: &Question, answers: &[QuestionAnswer]) -> String {
    question
        .items
        .iter()
        .zip(answers)
        .map(|(item, answer)| {
            let name = if item.header.trim().is_empty() {
                item.text.trim()
            } else {
                item.header.trim()
            };
            format!("{name}: {}", answered(answer))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn answered(answer: &QuestionAnswer) -> String {
    let own = answer
        .other
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    match (answer.selected.is_empty(), own) {
        (true, None) => "(no answer)".to_owned(),
        (true, Some(own)) => format!("in their own words: {own}"),
        (false, None) => answer.selected.join(", "),
        (false, Some(own)) => format!("{}; in their own words: {own}", answer.selected.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use roomkit_tui_protocol::{QuestionOption, RunOutcome, ToolCall, ToolState};
    use serde_json::json;

    use super::*;

    fn question(id: &str, run: &str) -> Question {
        let items = vec![
            QuestionItem::new(
                "Database",
                "Which database?",
                vec![
                    QuestionOption::new("SQLite", ""),
                    QuestionOption::new("Postgres", ""),
                ],
            ),
            QuestionItem::new("", "Which port?", Vec::new()),
        ];
        Question::new(id, "agent", run, items)
    }

    #[test]
    fn the_agent_reads_a_line_per_question() {
        let answers = [
            QuestionAnswer::new(vec!["SQLite".into()]),
            QuestionAnswer::default().with_other(" 8443 "),
        ];
        assert_eq!(
            answer_text(&question("q", "r"), &answers),
            "Database: SQLite\nWhich port?: in their own words: 8443"
        );
        let none = [QuestionAnswer::default(), QuestionAnswer::new(vec![])];
        assert_eq!(
            answer_text(&question("q", "r"), &none),
            "Database: (no answer)\nWhich port?: (no answer)"
        );
        let both = [
            QuestionAnswer::new(vec!["SQLite".into()]).with_other("in memory"),
            QuestionAnswer::default(),
        ];
        assert!(
            answer_text(&question("q", "r"), &both)
                .starts_with("Database: SQLite; in their own words: in memory")
        );
    }

    #[test]
    fn a_run_s_end_or_its_call_s_end_withdraws_a_question() {
        let questions = Questions::default();
        assert!(questions.add("room", question("a", "run1"), Some("c1".into())));
        assert!(questions.add("room", question("b", "run2"), None));
        let running = ToolCall::new("c1", "ask_user", json!({}), ToolState::Running);
        let item = |call: ToolCall| Event::Item {
            conversation: "room".into(),
            item: Item::Tool {
                actor: "agent".into(),
                run: "run1".into(),
                call,
            },
        };
        assert!(questions.ended_by("room", &item(running)).is_empty());
        let done = ToolCall::new("c1", "ask_user", json!({}), ToolState::Succeeded);
        assert_eq!(
            questions.ended_by("room", &item(done)),
            [QuestionId::new("a")]
        );
        let finished = Event::RunFinished {
            conversation: "room".into(),
            run: "run2".into(),
            outcome: RunOutcome::Interrupted,
        };
        assert_eq!(
            questions.ended_by("room", &finished),
            [QuestionId::new("b")]
        );
        assert!(questions.pending(("room", "agent")).is_empty());
    }

    /// ENG-9: a call of another run, settled under the identity of the
    /// call that asked, withdrew the question: providers reuse call ids.
    #[test]
    fn a_call_of_another_run_never_withdraws_a_question() {
        let questions = Questions::default();
        assert!(questions.add("room", question("a", "run1"), Some("c1".into())));
        let done = ToolCall::new("c1", "ask_user", json!({}), ToolState::Succeeded);
        let settled = |run: &str| Event::Item {
            conversation: "room".into(),
            item: Item::Tool {
                actor: "agent".into(),
                run: run.into(),
                call: done.clone(),
            },
        };
        assert!(questions.ended_by("room", &settled("run2")).is_empty());
        assert_eq!(
            questions.ended_by("room", &settled("run1")),
            [QuestionId::new("a")]
        );
    }

    #[test]
    fn a_question_is_taken_once_and_only_when_its_answers_fit() {
        let questions = Questions::default();
        assert!(questions.add("room", question("a", "run"), None));
        let id = QuestionId::new("a");
        assert_eq!(
            questions.take_if(("room", "agent"), &id, |_| false),
            Err(Untaken::Unfit)
        );
        assert_eq!(
            questions.pending(("room", "agent")).len(),
            1,
            "an unfit answer leaves it asked"
        );
        assert!(questions.take_if(("room", "agent"), &id, |_| true).is_ok());
        assert_eq!(
            questions.take_if(("room", "agent"), &id, |_| true),
            Err(Untaken::Gone)
        );
        assert_eq!(
            questions.take_if(("other", "agent"), &id, |_| true),
            Err(Untaken::Gone)
        );
    }

    #[test]
    fn only_a_view_of_the_asking_agent_answers_or_sees_a_question() {
        let questions = Questions::default();
        assert!(questions.add("room", question("a", "run"), None));
        let id = QuestionId::new("a");
        assert!(questions.pending(("room", "another")).is_empty());
        assert_eq!(
            questions.take_if(("room", "another"), &id, |_| true),
            Err(Untaken::Gone)
        );
        assert!(questions.take_if(("room", "agent"), &id, |_| true).is_ok());
    }
}
