//! The person's answer to a question an agent asks, taken back to the kit's
//! human-input request.

use roomkit::{ConversationStore, RejectInputOptions, RoomKit};
use roomkit_tui_protocol::{Event, Question, QuestionAnswer, QuestionId, Rejection, Request};

use super::serve::{View, send};
use super::{AdapterError, CommandHandler};
use crate::questions::{DECLINED, Untaken, answer_text};

impl<S: ConversationStore, H: CommandHandler> View<'_, S, H> {
    /// Take the person's answers to `id`, or their refusal, to the kit's
    /// human-input request, from a separate task; the question leaves every
    /// view of the room at once, and the reply says whether the asking call
    /// read it.
    pub(super) fn answer_question(
        &self,
        request: &Request,
        id: QuestionId,
        answers: Option<Vec<QuestionAnswer>>,
    ) -> Result<(), AdapterError> {
        let question = match self.take_question(&id, answers.as_deref()) {
            Ok(question) => question,
            Err(error) => return self.reply(request, Some(error)),
        };
        let (kit, channel) = (self.session.kit.clone(), self.session.channel.clone());
        let room = self.session.config.conversation().to_owned();
        let (host, reply_to) = (self.host.clone(), request.id);
        tokio::spawn(async move {
            let error = settle_question(&kit, &question, &id, answers).await;
            let conversation = room.as_str().into();
            let resolved = Event::QuestionResolved {
                conversation,
                question: id,
            };
            channel.publish(&room, resolved);
            let reply = Event::Reply {
                request: reply_to,
                error,
            };
            let _ = send(&host, reply);
        });
        Ok(())
    }

    /// The question `id` this view's agent asks, no longer waiting, when
    /// `answers` fit it; no answers close it.
    fn take_question(
        &self,
        id: &QuestionId,
        answers: Option<&[QuestionAnswer]>,
    ) -> Result<Question, Rejection> {
        let config = &self.session.config;
        let scope = (config.conversation(), config.agent.as_str());
        let fits =
            |question: &Question| answers.is_none_or(|answers| question.answered_by(answers));
        let questions = &self.session.channel.questions;
        questions
            .take_if(scope, id, fits)
            .map_err(|untaken| match untaken {
                Untaken::Gone => Rejection::invalid("This question is no longer asked"),
                Untaken::Unfit => Rejection::invalid("These answers do not fit the question"),
            })
    }
}

/// Hand the kit's human-input request `id` the person's `answers` to
/// `question`, or their refusal: why the reply refuses, when the asking call
/// did not read them.
async fn settle_question<S: ConversationStore>(
    kit: &RoomKit<S>,
    question: &Question,
    id: &QuestionId,
    answers: Option<Vec<QuestionAnswer>>,
) -> Option<Rejection> {
    let settled = match answers {
        Some(answers) => {
            let text = answer_text(question, &answers);
            kit.answer_input(id.as_str(), &text).await
        }
        None => {
            let reason = DECLINED.to_owned();
            kit.reject_input(id.as_str(), RejectInputOptions { reason })
                .await
        }
    };
    match settled {
        Ok(true) => None,
        Ok(false) => Some(Rejection::unavailable(
            "The question was settled otherwise first",
        )),
        Err(_) => Some(Rejection::unavailable("The engine is closing")),
    }
}
