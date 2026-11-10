//! Questions an agent asks the person, in delivery order. Those of another
//! conversation, of a run this view does not follow, or too large to show
//! in full are not shown.

use roomkit_tui_protocol::limits::MAX_PENDING_QUESTIONS;
use roomkit_tui_protocol::{ConversationId, Question, QuestionId};

use super::Session;
use crate::QuestionCard;

impl Session {
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

    /// The questions on screen, to apply a key or a paste to.
    pub(crate) fn question_mut(&mut self) -> Option<&mut QuestionCard> {
        self.questions.first_mut()
    }
}
