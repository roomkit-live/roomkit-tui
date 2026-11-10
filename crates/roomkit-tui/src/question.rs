//! Questions an agent asks the person, one card at a time: each question's
//! options, the person's own words, and the answers sent once the last
//! question is confirmed. The card holds no network or session state.

use std::collections::BTreeSet;
use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use roomkit_tui_protocol::limits::MAX_QUESTION_ANSWER_BYTES;
use roomkit_tui_protocol::{Command, Question, QuestionAnswer, QuestionId, QuestionItem};

use crate::editor::Editor;

/// What a key does to the card.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CardStep {
    /// The card changed, or nothing happened.
    Stay,
    /// The last question was confirmed: send the answers.
    Answer(Command),
    /// The person closes the questions without answering.
    Decline(Command),
}

/// A card of questions on screen. Debug never shows what the person wrote.
pub struct QuestionCard {
    pub(crate) question: Question,
    /// The question shown, by index.
    pub(crate) current: usize,
    /// Per question, the highlighted row: an option, or the last row for
    /// the person's own words.
    pub(crate) cursor: Vec<usize>,
    /// Per question, the options picked, by index.
    pub(crate) picked: Vec<BTreeSet<usize>>,
    /// Per question, the person's own words.
    pub(crate) other: Vec<Editor>,
    /// Whether keys go to the own words of the current question.
    pub(crate) typing: bool,
    /// The answers left, awaiting the host's confirmation.
    pub(crate) sent: bool,
}

impl fmt::Debug for QuestionCard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QuestionCard")
            .field("question", &self.question.id)
            .field("current", &self.current)
            .field("typing", &self.typing)
            .field("sent", &self.sent)
            .finish_non_exhaustive()
    }
}

impl QuestionCard {
    /// A card for `question`; `None` when a terminal cannot show it in full.
    pub(crate) fn new(question: Question) -> Option<Self> {
        if !question.fits() {
            return None;
        }
        let count = question.items.len();
        Some(Self {
            question,
            current: 0,
            cursor: vec![0; count],
            picked: vec![BTreeSet::new(); count],
            other: (0..count).map(|_| Editor::default()).collect(),
            typing: false,
            sent: false,
        })
    }

    /// The questions asked.
    pub fn question(&self) -> &Question {
        &self.question
    }

    /// The question shown.
    pub fn item(&self) -> &QuestionItem {
        &self.question.items[self.current]
    }

    /// Index of the question shown.
    pub fn current(&self) -> usize {
        self.current
    }

    /// The highlighted row of the question shown; the row after its options
    /// is the person's own words.
    pub fn cursor(&self) -> usize {
        self.cursor[self.current]
    }

    /// Whether option `index` of the question shown is picked.
    pub fn is_picked(&self, index: usize) -> bool {
        self.picked[self.current].contains(&index)
    }

    /// The own words written for the question shown.
    pub fn own_words(&self) -> &str {
        self.other[self.current].text()
    }

    /// Whether keys go to the own words.
    pub fn typing(&self) -> bool {
        self.typing
    }

    /// The row of the own words, after the options.
    fn own_row(&self) -> usize {
        self.item().options.len()
    }

    /// Text pasted into the own words, when they are being written.
    pub(crate) fn insert(&mut self, text: &str) {
        if !self.typing || self.sent {
            return;
        }
        let text = text.replace(['\n', '\r', '\t'], " ");
        let editor = &mut self.other[self.current];
        if editor.text().len().saturating_add(text.len()) <= MAX_QUESTION_ANSWER_BYTES {
            editor.insert(&text);
        }
    }

    /// Apply `key` to the card.
    pub(crate) fn key(&mut self, key: KeyEvent) -> CardStep {
        if self.sent {
            return CardStep::Stay;
        }
        if self.typing {
            return self.typing_key(key);
        }
        let rows = self.own_row() + 1;
        let cursor = self.cursor[self.current];
        match key.code {
            KeyCode::Esc => return CardStep::Decline(self.command(None)),
            KeyCode::Up | KeyCode::Char('k') => {
                self.cursor[self.current] = (cursor + rows - 1) % rows;
            }
            KeyCode::Down | KeyCode::Char('j') => self.cursor[self.current] = (cursor + 1) % rows,
            KeyCode::Tab | KeyCode::Right => self.next(),
            KeyCode::BackTab | KeyCode::Left => self.current = self.current.saturating_sub(1),
            KeyCode::Char(' ') => self.toggle(cursor),
            KeyCode::Enter => return self.confirm(cursor),
            KeyCode::Char(digit @ '1'..='9') => {
                let index = (digit as usize) - ('1' as usize);
                if index < rows {
                    self.cursor[self.current] = index;
                    if self.item().multi_select && index < self.own_row() {
                        self.toggle(index);
                    } else {
                        return self.confirm(index);
                    }
                }
            }
            _ => {}
        }
        CardStep::Stay
    }

    fn typing_key(&mut self, key: KeyEvent) -> CardStep {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let editor = &mut self.other[self.current];
        match key.code {
            // Back to the options, keeping what was written.
            KeyCode::Esc => self.typing = false,
            KeyCode::Enter => {
                self.typing = false;
                // One answer to a single choice: own words replace an option.
                if !self.item().multi_select && !self.own_words().trim().is_empty() {
                    self.picked[self.current].clear();
                }
                return self.advance();
            }
            KeyCode::Char('u') if control => editor.set(String::new()),
            KeyCode::Char(c) if !control && !alt => self.insert(&c.to_string()),
            KeyCode::Backspace => editor.backspace(),
            KeyCode::Delete => editor.delete(),
            KeyCode::Left => editor.left(),
            KeyCode::Right => editor.right(),
            KeyCode::Home => editor.home(),
            KeyCode::End => editor.end(),
            _ => {}
        }
        CardStep::Stay
    }

    /// Space: pick the highlighted option, among others when the question
    /// allows several, or start writing on the own-words row.
    fn toggle(&mut self, index: usize) {
        if index >= self.own_row() {
            self.typing = true;
            return;
        }
        let picked = &mut self.picked[self.current];
        if self.question.items[self.current].multi_select {
            if !picked.remove(&index) {
                picked.insert(index);
            }
        } else {
            picked.clear();
            picked.insert(index);
            self.other[self.current].set(String::new());
        }
    }

    /// Enter or a digit on `index`: on a single choice, pick it and go on;
    /// on several, go on with what is picked; on the own-words row, write.
    fn confirm(&mut self, index: usize) -> CardStep {
        if index >= self.own_row() {
            self.typing = true;
            return CardStep::Stay;
        }
        if !self.item().multi_select {
            self.toggle(index);
        }
        self.advance()
    }

    /// The next question, or the answers after the last one.
    fn advance(&mut self) -> CardStep {
        if self.current + 1 < self.question.items.len() {
            self.current += 1;
            return CardStep::Stay;
        }
        CardStep::Answer(self.command(Some(self.answers())))
    }

    fn next(&mut self) {
        if self.current + 1 < self.question.items.len() {
            self.current += 1;
        }
    }

    /// One answer per question: the labels picked, in the order offered,
    /// and the own words, when any.
    pub(crate) fn answers(&self) -> Vec<QuestionAnswer> {
        self.question
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let selected = self.picked[index]
                    .iter()
                    .map(|&option| item.options[option].label.clone())
                    .collect();
                let answer = QuestionAnswer::new(selected);
                let own = self.other[index].text().trim();
                if own.is_empty() {
                    answer
                } else {
                    answer.with_other(own)
                }
            })
            .collect()
    }

    fn command(&self, answers: Option<Vec<QuestionAnswer>>) -> Command {
        Command::AnswerQuestion {
            question: self.question.id.clone(),
            answers,
        }
    }

    /// The identity of the questions.
    pub fn id(&self) -> &QuestionId {
        &self.question.id
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::panic,
        reason = "fixture failures must fail the test"
    )]

    use roomkit_tui_protocol::QuestionOption;

    use super::*;

    fn press(card: &mut QuestionCard, code: KeyCode) -> CardStep {
        card.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn type_in(card: &mut QuestionCard, text: &str) {
        for c in text.chars() {
            press(card, KeyCode::Char(c));
        }
    }

    fn card(multi: bool) -> QuestionCard {
        let database = QuestionItem::new(
            "Database",
            "Which database?",
            vec![
                QuestionOption::new("SQLite", "One file"),
                QuestionOption::new("Postgres", ""),
            ],
        );
        let checks = QuestionItem::new(
            "Checks",
            "Which checks run?",
            vec![
                QuestionOption::new("Tests", ""),
                QuestionOption::new("Lint", ""),
                QuestionOption::new("Audit", ""),
            ],
        )
        .with_multi_select(multi);
        QuestionCard::new(Question::new("q1", "agent", "run", vec![database, checks])).unwrap()
    }

    fn answers(step: CardStep) -> Vec<QuestionAnswer> {
        let CardStep::Answer(Command::AnswerQuestion {
            answers: Some(answers),
            ..
        }) = step
        else {
            panic!("answers expected: {step:?}");
        };
        answers
    }

    #[test]
    fn a_digit_picks_an_option_and_the_last_answer_sends_them_all() {
        let mut card = card(true);
        assert_eq!(press(&mut card, KeyCode::Char('2')), CardStep::Stay);
        assert_eq!(card.current(), 1, "a single choice goes on once picked");
        press(&mut card, KeyCode::Char('1'));
        press(&mut card, KeyCode::Char('3'));
        press(&mut card, KeyCode::Char('3'));
        press(&mut card, KeyCode::Char('2'));
        let sent = answers(press(&mut card, KeyCode::Enter));
        assert_eq!(sent[0], QuestionAnswer::new(vec!["Postgres".into()]));
        assert_eq!(
            sent[1],
            QuestionAnswer::new(vec!["Tests".into(), "Lint".into()])
        );
        assert!(card.question().answered_by(&sent));
    }

    #[test]
    fn arrows_and_enter_choose_and_tab_moves_between_questions() {
        let mut card = card(false);
        press(&mut card, KeyCode::Down);
        press(&mut card, KeyCode::Up);
        press(&mut card, KeyCode::Up);
        assert_eq!(card.cursor(), 2, "the cursor wraps to the own-words row");
        press(&mut card, KeyCode::Tab);
        assert_eq!(card.current(), 1);
        press(&mut card, KeyCode::BackTab);
        assert_eq!(card.current(), 0);
        press(&mut card, KeyCode::Down);
        assert_eq!(card.cursor(), 0);
        assert_eq!(press(&mut card, KeyCode::Enter), CardStep::Stay);
        assert_eq!(card.current(), 1);
        let sent = answers(press(&mut card, KeyCode::Enter));
        assert_eq!(sent[0], QuestionAnswer::new(vec!["SQLite".into()]));
        assert_eq!(sent[1], QuestionAnswer::new(vec!["Tests".into()]));
    }

    #[test]
    fn own_words_replace_a_single_choice_and_esc_goes_back_to_the_options() {
        let mut card = card(false);
        press(&mut card, KeyCode::Char(' '));
        press(&mut card, KeyCode::Char('3'));
        assert!(card.typing());
        type_in(&mut card, "DuckDB");
        press(&mut card, KeyCode::Esc);
        assert!(!card.typing(), "Esc leaves the words, not the card");
        assert_eq!(card.own_words(), "DuckDB");
        press(&mut card, KeyCode::Char('3'));
        type_in(&mut card, " 1.2");
        assert_eq!(press(&mut card, KeyCode::Enter), CardStep::Stay);
        let sent = answers(press(&mut card, KeyCode::Enter));
        assert_eq!(sent[0], QuestionAnswer::default().with_other("DuckDB 1.2"));
        assert!(card.question().answered_by(&sent));
    }

    #[test]
    fn esc_declines_and_a_sent_card_takes_no_more_keys() {
        let mut card = card(false);
        let CardStep::Decline(Command::AnswerQuestion { question, answers }) =
            press(&mut card, KeyCode::Esc)
        else {
            panic!("a declined card");
        };
        assert_eq!((question.as_str(), answers), ("q1", None));
        card.sent = true;
        assert_eq!(press(&mut card, KeyCode::Char('1')), CardStep::Stay);
        assert_eq!(card.current(), 0);
    }

    #[test]
    fn a_card_too_large_to_show_is_not_made_and_debug_hides_the_words() {
        let items = vec![QuestionItem::new("", " ", Vec::new())];
        assert!(QuestionCard::new(Question::new("q", "agent", "run", items)).is_none());
        let mut card = card(false);
        press(&mut card, KeyCode::Char('3'));
        type_in(&mut card, "private words");
        assert!(!format!("{card:?}").contains("private words"));
    }
}
