//! Questions an agent asks the person during one of its runs. The host
//! carries them and their answers; what the agent reads of an answer is the
//! host's to say.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::limits::{
    MAX_LABEL_BYTES, MAX_QUESTION_ANSWER_BYTES, MAX_QUESTION_DESCRIPTION_BYTES,
    MAX_QUESTION_HEADER_BYTES, MAX_QUESTION_ITEMS, MAX_QUESTION_OPTIONS, MAX_QUESTION_TEXT_BYTES,
};
use crate::{ActorId, QuestionId, RunId};

/// One option a question offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct QuestionOption {
    /// What the person picks, and what the agent reads back.
    pub label: String,
    /// What picking it means; empty for none.
    #[serde(default)]
    pub description: String,
}

impl QuestionOption {
    /// An option `label`, described by `description`.
    pub fn new(label: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            description: description.into(),
        }
    }
}

/// One question among those an agent asks at once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct QuestionItem {
    /// A short label naming the question, as a tab names it.
    pub header: String,
    /// The question itself.
    pub text: String,
    /// The options offered; the person may also write their own answer.
    pub options: Vec<QuestionOption>,
    /// Whether the person may pick several options.
    #[serde(default)]
    pub multi_select: bool,
}

impl QuestionItem {
    /// The question `text`, named `header`, offering `options` to pick one.
    pub fn new(
        header: impl Into<String>,
        text: impl Into<String>,
        options: Vec<QuestionOption>,
    ) -> Self {
        Self {
            header: header.into(),
            text: text.into(),
            options,
            multi_select: false,
        }
    }

    /// Let the person pick several options.
    #[must_use]
    pub fn with_multi_select(mut self, multi_select: bool) -> Self {
        self.multi_select = multi_select;
        self
    }

    fn fits(&self) -> bool {
        let mut labels = BTreeSet::new();
        !self.text.trim().is_empty()
            && self.text.len() <= MAX_QUESTION_TEXT_BYTES
            && self.header.len() <= MAX_QUESTION_HEADER_BYTES
            && self.options.len() <= MAX_QUESTION_OPTIONS
            && self.options.iter().all(|option| {
                !option.label.trim().is_empty()
                    && option.label.len() <= MAX_LABEL_BYTES
                    && option.description.len() <= MAX_QUESTION_DESCRIPTION_BYTES
                    && labels.insert(option.label.as_str())
            })
    }
}

/// Questions an agent asks the person at once, during its run `run`. The
/// run waits for the answer, or for the person to decline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Question {
    /// Identity used to answer precisely these questions.
    pub id: QuestionId,
    /// The agent asking.
    pub actor: ActorId,
    /// Owning execution.
    pub run: RunId,
    /// The questions, in the order the agent asked them.
    pub items: Vec<QuestionItem>,
}

impl Question {
    /// The questions `items` agent `actor` asks during `run`.
    pub fn new(
        id: impl Into<QuestionId>,
        actor: impl Into<ActorId>,
        run: impl Into<RunId>,
        items: Vec<QuestionItem>,
    ) -> Self {
        Self {
            id: id.into(),
            actor: actor.into(),
            run: run.into(),
            items,
        }
    }

    /// Whether a terminal can show these questions in full: at most
    /// [`MAX_QUESTION_ITEMS`] of them, each with a text, at most
    /// [`MAX_QUESTION_OPTIONS`] distinct options and bounded texts. A host
    /// refuses a larger request rather than show it cut.
    pub fn fits(&self) -> bool {
        self.id.as_str().len() <= MAX_LABEL_BYTES
            && self.run.as_str().len() <= MAX_LABEL_BYTES
            && !self.items.is_empty()
            && self.items.len() <= MAX_QUESTION_ITEMS
            && self.items.iter().all(QuestionItem::fits)
    }

    /// Whether `answers` answer these questions: one per question, each
    /// picking only offered options, one at most unless the question allows
    /// several, with own words within [`MAX_QUESTION_ANSWER_BYTES`]. An
    /// answer picking nothing and writing nothing leaves its question
    /// unanswered.
    pub fn answered_by(&self, answers: &[QuestionAnswer]) -> bool {
        answers.len() == self.items.len()
            && self
                .items
                .iter()
                .zip(answers)
                .all(|(item, answer)| answer.fits(item))
    }
}

/// The person's answer to one question: the options they picked and their
/// own words, either of which may be empty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct QuestionAnswer {
    /// The labels of the options picked, in the order offered.
    #[serde(default)]
    pub selected: Vec<String>,
    /// What the person wrote instead of, or beside, the options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other: Option<String>,
}

impl QuestionAnswer {
    /// An answer picking `selected`.
    pub fn new(selected: Vec<String>) -> Self {
        Self {
            selected,
            other: None,
        }
    }

    /// An answer in the person's own words.
    #[must_use]
    pub fn with_other(mut self, other: impl Into<String>) -> Self {
        self.other = Some(other.into());
        self
    }

    /// Whether neither an option nor own words answer the question.
    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
            && self
                .other
                .as_deref()
                .is_none_or(|text| text.trim().is_empty())
    }

    fn fits(&self, item: &QuestionItem) -> bool {
        let offered: BTreeSet<&str> = item.options.iter().map(|o| o.label.as_str()).collect();
        let picked: BTreeSet<&str> = self.selected.iter().map(String::as_str).collect();
        picked.len() == self.selected.len()
            && picked.is_subset(&offered)
            && (item.multi_select || self.selected.len() <= 1)
            && self
                .other
                .as_ref()
                .is_none_or(|text| text.len() <= MAX_QUESTION_ANSWER_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(multi: bool) -> QuestionItem {
        QuestionItem::new(
            "Database",
            "Which database holds the sessions?",
            vec![
                QuestionOption::new("SQLite", "One file"),
                QuestionOption::new("Postgres", "Shared"),
            ],
        )
        .with_multi_select(multi)
    }

    fn question(items: Vec<QuestionItem>) -> Question {
        Question::new("q1", "agent", "run", items)
    }

    #[test]
    fn a_question_fits_within_its_bounds() {
        assert!(question(vec![item(false)]).fits());
        assert!(!question(Vec::new()).fits(), "no question");
        assert!(!question(vec![item(false); MAX_QUESTION_ITEMS + 1]).fits());
        let mut many = item(false);
        many.options = (0..=MAX_QUESTION_OPTIONS)
            .map(|n| QuestionOption::new(format!("o{n}"), ""))
            .collect();
        assert!(!question(vec![many]).fits());
        let mut twice = item(false);
        twice.options.push(QuestionOption::new("SQLite", "again"));
        assert!(!question(vec![twice]).fits(), "options are distinct");
        let mut blank = item(false);
        blank.text = "  ".into();
        assert!(!question(vec![blank]).fits());
        let open = QuestionItem::new("", "Anything to add?", Vec::new());
        assert!(
            question(vec![open]).fits(),
            "a question may offer no option"
        );
    }

    #[test]
    fn answers_pick_offered_options_once_and_one_unless_several_are_allowed() {
        let single = question(vec![item(false)]);
        let one = QuestionAnswer::new(vec!["SQLite".into()]);
        assert!(single.answered_by(std::slice::from_ref(&one)));
        assert!(!single.answered_by(&[]), "one answer per question");
        let both = QuestionAnswer::new(vec!["SQLite".into(), "Postgres".into()]);
        assert!(!single.answered_by(std::slice::from_ref(&both)));
        assert!(question(vec![item(true)]).answered_by(&[both]));
        let unknown = QuestionAnswer::new(vec!["Redis".into()]);
        assert!(!single.answered_by(&[unknown]));
        let own = QuestionAnswer::default().with_other("DuckDB");
        assert!(single.answered_by(&[own]));
        let long = QuestionAnswer::default().with_other("x".repeat(MAX_QUESTION_ANSWER_BYTES + 1));
        assert!(!single.answered_by(&[long]));
        assert!(QuestionAnswer::default().with_other(" ").is_empty());
    }
}
