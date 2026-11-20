//! Questions an agent asks the person, in delivery order. Those of another
//! conversation, of a run this view does not follow, or too large to show
//! in full are not shown.

use std::time::Duration;

use roomkit_tui_protocol::limits::MAX_PENDING_QUESTIONS;
use roomkit_tui_protocol::{ConversationId, Question, QuestionId, RunId};

use super::{APPROVAL_ARMING_DELAY, Session};
use crate::QuestionCard;

impl Session {
    /// The questions on screen: the oldest an agent asks.
    pub fn question(&self) -> Option<&QuestionCard> {
        self.questions.first()
    }

    /// Whether the questions on screen take answers at `now`: a frame
    /// synchronized through [`Self::sync`] has shown them for at least
    /// [`APPROVAL_ARMING_DELAY`], as for a decision, so that typing under way
    /// when they appear answers nothing.
    pub fn question_armed(&self, now: Duration) -> bool {
        let Some(head) = self.questions.first() else {
            return true;
        };
        self.question_drawn.as_ref().is_some_and(|(id, drawn)| {
            id == head.id() && now.saturating_sub(*drawn) >= APPROVAL_ARMING_DELAY
        })
    }

    pub(super) fn on_question_asked(&mut self, conversation: &ConversationId, question: Question) {
        if !self.is_current(conversation) || !self.is_live(&question.run) {
            return;
        }
        if self.questions.iter().any(|card| *card.id() == question.id) {
            return;
        }
        if self.questions.len() >= MAX_PENDING_QUESTIONS {
            self.notice = "The agent asked more questions than this view keeps; \
                           they were not shown"
                .into();
            return;
        }
        match QuestionCard::new(question) {
            Some(card) => self.questions.push(card),
            None => {
                self.notice = "The agent asked questions this view cannot show in full; \
                               they were not shown"
                    .into();
            }
        }
    }

    pub(super) fn on_question_resolved(&mut self, conversation: &ConversationId, id: &QuestionId) {
        if self.is_current(conversation) {
            self.questions.retain(|card| card.id() != id);
        }
    }

    /// `run` ended, or the view stopped following it: nothing it asked can
    /// be answered any more, a card already sent included.
    pub(super) fn forget_questions(&mut self, run: &RunId) {
        self.questions.retain(|card| card.question().run != *run);
    }

    /// The questions on screen, to apply a key or a paste to.
    pub(crate) fn question_mut(&mut self) -> Option<&mut QuestionCard> {
        self.questions.first_mut()
    }
}
