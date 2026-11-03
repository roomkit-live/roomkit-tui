//! One accepted message, from admission to the engine's settled delivery.
//! The turn runs on its own task: detaching the view never stops it.

use std::sync::Arc;

use roomkit::model::{EventContent, EventType, InboundMessage};
use roomkit::{ConversationStore, InboundResult, ProcessInboundOptions, RoomKit};
use roomkit_tui_protocol::limits::{
    MAX_IMAGE_BASE64_BYTES, MAX_IMAGE_NAME_BYTES, MAX_IMAGE_PIXELS, MAX_IMAGES, MAX_TEXT_BYTES,
};
use roomkit_tui_protocol::{
    Command, Event, ImageMessage, OperationId, Rejection, Request, RunOutcome,
};

use super::buffered::Buffered;
use super::serve::View;
use super::{AdapterError, CommandHandler, SessionConfig, restore};
use crate::activity::Active;
use crate::execution::{Interrupter, Operation};
use crate::{ConversationChannel, SessionLifecycle, TurnOutcome, cancellation, projection};

/// What a turn started without a new message is told. It is an instruction:
/// never stored, and never shown as anyone's words.
const ANSWER_NOW: &str = "The person cut the turn that was running to get an answer now. \
Answer the messages they sent during that turn.";

/// Validate a submission against this view's agent and transport bounds.
pub(super) fn validate(request: &Request, config: &SessionConfig) -> Result<(), Rejection> {
    let agent = config.agent.as_str();
    match &request.command {
        Command::Submit { agent: to, text }
            if to.as_str() == agent && !text.trim().is_empty() && text.len() <= MAX_TEXT_BYTES =>
        {
            Ok(())
        }
        Command::SubmitImages { agent: to, message }
            if config.can_attach_images && to.as_str() == agent && images_fit(message) =>
        {
            Ok(())
        }
        Command::SubmitImages { .. } => Err(Rejection::invalid(
            "Images unavailable for this agent or attachment limits exceeded",
        )),
        Command::Answer { agent: to } if to.as_str() == agent => Ok(()),
        _ => Err(Rejection::invalid("Invalid agent or message")),
    }
}

fn images_fit(message: &ImageMessage) -> bool {
    message.text.len() <= MAX_TEXT_BYTES
        && !message.images.is_empty()
        && message.images.len() <= MAX_IMAGES
        && message.images.iter().all(|image| {
            image.data.len() <= MAX_IMAGE_BASE64_BYTES
                && image.name.len() <= MAX_IMAGE_NAME_BYTES
                && image.mime_type == "image/png"
                && u64::from(image.width) * u64::from(image.height) <= MAX_IMAGE_PIXELS
        })
}

impl<S: ConversationStore, H: CommandHandler> View<'_, S, H> {
    /// Accept a validated message in the reserved room and start its turn.
    pub fn start_turn(
        &mut self,
        request: &Request,
        content: EventContent,
        reserved: Active,
    ) -> Result<(), AdapterError> {
        self.start(request, content, reserved, false)
    }

    /// Start a turn in the reserved room for what the conversation holds: the
    /// agent reads it there. The host's gate holds as for any turn
    /// (`SessionLifecycle::admit_answer`).
    pub fn start_answer(
        &mut self,
        request: &Request,
        reserved: Active,
    ) -> Result<(), AdapterError> {
        let content = roomkit::model::TextContent::new(ANSWER_NOW).into();
        self.start(request, content, reserved, true)
    }

    fn start(
        &mut self,
        request: &Request,
        content: EventContent,
        reserved: Active,
        answer: bool,
    ) -> Result<(), AdapterError> {
        self.reply(request, None)?;
        let session = &self.session;
        let operation = request.operation.clone();
        let room = session.config.conversation().into();
        let processing =
            Operation::start(session.channel.clone(), room, operation.clone(), reserved);
        let (control, controls) = cancellation::connection();
        let turn = Turn {
            kit: session.kit.clone(),
            channel: session.channel.clone(),
            config: session.config.clone(),
            lifecycle: session.lifecycle.clone(),
            operation,
            controls,
            answer,
        };
        tokio::spawn(turn.run(content, processing));
        self.work = Some(Interrupter::Engine(control));
        Ok(())
    }
}

struct Turn<S: ConversationStore> {
    kit: RoomKit<S>,
    channel: ConversationChannel,
    config: SessionConfig,
    lifecycle: Option<Arc<dyn SessionLifecycle>>,
    operation: OperationId,
    controls: cancellation::Receiver,
    /// The turn answers what the conversation holds: its content is an
    /// instruction, not a person's words.
    answer: bool,
}

/// How a delivery ended, before the host's lifecycle is told.
struct Delivered {
    outcome: RunOutcome,
    failure: Option<&'static str>,
    blocked: bool,
}

impl<S: ConversationStore> Turn<S> {
    async fn run(self, content: EventContent, processing: Operation) {
        let (channel, room) = (self.channel.clone(), self.config.room.to_string());
        // A host callback panic must not leave the client permanently busy.
        let outcome = match tokio::spawn(self.execute(content)).await {
            Ok(outcome) => outcome,
            Err(_) => {
                let text =
                    "The application failed while handling this message; inspect host diagnostics";
                let conversation = room.as_str().into();
                channel.publish(
                    &room,
                    Event::Notice {
                        conversation,
                        text: text.into(),
                    },
                );
                RunOutcome::Failed
            }
        };
        processing.finish(outcome);
    }

    async fn execute(mut self, content: EventContent) -> RunOutcome {
        if let Some(lifecycle) = &self.lifecycle {
            let prepared = crate::lifecycle::prepare(
                lifecycle.as_ref(),
                &self.config,
                &self.operation,
                (&content, self.answer),
                &self.channel,
                &mut self.controls,
            );
            if let Some(outcome) = prepared.await {
                return outcome;
            }
        }
        let delivered = self.deliver(content).await;
        if let Some(text) = delivered.failure {
            self.notice(text);
        }
        let stored = crate::approval_history::persist(&self.kit, &self.channel, &self.config).await;
        if stored.is_err() {
            self.notice("Decision accepted, but its history could not be saved. This does not change tool execution.");
        }
        let outcome = match delivered.failure {
            Some(_) => RunOutcome::Failed,
            None => delivered.outcome,
        };
        if let Some(lifecycle) = &self.lifecycle {
            let ended = match outcome {
                _ if delivered.blocked => TurnOutcome::Blocked,
                RunOutcome::Completed => TurnOutcome::Completed,
                RunOutcome::Interrupted => TurnOutcome::Interrupted,
                _ => TurnOutcome::Failed,
            };
            lifecycle
                .finished(&self.config, &self.operation, ended)
                .await;
        }
        outcome
    }

    fn notice(&self, text: &str) {
        let room = self.config.conversation();
        let event = Event::Notice {
            conversation: room.into(),
            text: text.into(),
        };
        self.channel.publish(room, event);
    }

    async fn deliver(&mut self, content: EventContent) -> Delivered {
        let (config, channel) = (&self.config, &self.channel);
        let mut message = InboundMessage::new(channel.id().clone(), &config.sender, content);
        message.addressed_to = Some(vec![config.agent.clone()]);
        let _requested = self
            .answer
            .then(|| channel.answer_requested_by(config.conversation(), &config.sender));
        if self.answer {
            // Never stored, it holds no key to replay under: RoomKit refuses one.
            message.event_type = EventType::Instruction;
        } else {
            let key = [
                config.conversation(),
                channel.id().as_str(),
                &config.sender,
                self.operation.as_str(),
            ];
            message.idempotency_key = Some(serde_json::json!(key).to_string());
        }
        let options = ProcessInboundOptions {
            room_id: Some(config.room.clone()),
            organization_id: config.organization.clone(),
            defer_delivery: true,
        };
        match self.kit.process_inbound(message, options).await {
            Ok(admitted) => self.settle(admitted).await,
            Err(error) => Delivered {
                outcome: RunOutcome::Failed,
                failure: Some(crate::diagnostics::explain(&error.to_string())),
                blocked: false,
            },
        }
    }

    /// Wait for the delivery while interruption stays live, then classify it.
    async fn settle(&mut self, mut admitted: InboundResult) -> Delivered {
        let room = self.config.conversation().to_owned();
        let run = admitted
            .event
            .as_ref()
            .map(|row| projection::run_id(&self.config.agent, &row.id));
        if !admitted.blocked
            && let Some(row) = &admitted.event
        {
            self.channel.row(row);
        }
        let announced = run
            .clone()
            .filter(|_| !self.channel.streams() && !admitted.blocked);
        let buffered = Buffered::start(&self.channel, &room, announced);
        let mut failure = None;
        let mut interrupted = false;
        if let (Some(delivery), Some(run)) = (admitted.delivery.clone(), run) {
            let waited =
                cancellation::wait(delivery, run, &room, &self.channel, &mut self.controls).await;
            interrupted = waited.1;
            match waited.0 {
                Ok(done) => admitted = done,
                Err(error) => failure = Some(crate::diagnostics::explain(&error.to_string())),
            }
        }
        let cancelled = admitted.cancellation_reason.as_deref();
        let mut outcome = if interrupted && cancelled == Some("caller_cancelled") {
            RunOutcome::Interrupted
        } else {
            RunOutcome::Completed
        };
        if let Some(error) = &admitted.error {
            failure = Some(crate::diagnostics::explain(&error.message));
        } else if admitted.blocked {
            failure =
                Some("The engine blocked this message. Check the room state and host policy.");
        } else if cancelled.is_some() && outcome != RunOutcome::Interrupted {
            failure = Some("Execution cancelled. Completed tool actions are retained.");
        }
        if failure.is_some() {
            outcome = RunOutcome::Failed;
        }
        self.publish_usage(&admitted).await;
        buffered.finish(&admitted, outcome);
        Delivered {
            outcome,
            failure,
            blocked: admitted.blocked,
        }
    }

    /// The final record may update an earlier text segment after streaming;
    /// measure only what this view is authorized to see.
    async fn publish_usage(&self, admitted: &InboundResult) {
        let Some(trigger) = &admitted.event else {
            return;
        };
        let Ok(snapshot) = restore::snapshot(&self.kit, &self.channel, &self.config).await else {
            return;
        };
        if let (Some(usage), Some(model)) = (snapshot.usage, snapshot.model) {
            let room = self.config.conversation();
            self.channel.publish(
                room,
                Event::UsageUpdated {
                    conversation: room.into(),
                    run: projection::run_id(&self.config.agent, &trigger.id),
                    provider: model.provider,
                    model: model.model,
                    context: snapshot.context,
                    usage,
                },
            );
        }
    }
}
