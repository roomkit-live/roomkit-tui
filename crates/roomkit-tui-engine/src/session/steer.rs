//! A message sent while the agent's turn runs (RMK-1161). The agent steers
//! its room's turns, so RoomKit hands the message to that turn, which reads
//! it at its next step; when the turn no longer reads it, the message waits
//! a turn of its own after. Either way no second turn runs beside the first,
//! and the view starts no operation of its own.

use std::sync::Arc;

use roomkit::model::{EventContent, InboundMessage, TextContent};
use roomkit::{ConversationStore, InboundResult, ProcessInboundOptions, RoomKit};
use roomkit_tui_protocol::{Command, Event, MessageId, OperationId, Rejection, Request};

use super::serve::View;
use super::{AdapterError, CommandHandler, SessionConfig};
use crate::{ConversationChannel, SessionLifecycle, TurnOutcome};

impl<S: ConversationStore, H: CommandHandler> View<'_, S, H> {
    /// Whether `request`, refused a reservation because a turn holds the
    /// room, may go to that turn: a message, for an agent that steers its
    /// room's turns.
    pub(super) fn steers(&self, request: &Request) -> bool {
        matches!(request.command, Command::Submit { .. }) && self.session.config.can_steer
    }

    /// Accept the validated message and hand it to the room's running turn.
    pub(super) fn steer(&mut self, request: &Request) -> Result<(), AdapterError> {
        let Command::Submit { text, .. } = &request.command else {
            return Ok(());
        };
        self.reply(request, None)?;
        let steer = Steer {
            kit: self.session.kit.clone(),
            channel: self.session.channel.clone(),
            config: self.session.config.clone(),
            lifecycle: self.session.lifecycle.clone(),
            operation: request.operation.clone(),
        };
        tokio::spawn(steer.run(text.clone()));
        Ok(())
    }
}

struct Steer<S: ConversationStore> {
    kit: RoomKit<S>,
    channel: ConversationChannel,
    config: SessionConfig,
    lifecycle: Option<Arc<dyn SessionLifecycle>>,
    operation: OperationId,
}

impl<S: ConversationStore> Steer<S> {
    /// Admit the message as the host admits every message, deliver it, and
    /// tell the host how it ended. It starts no turn: no `started`.
    async fn run(self, text: String) {
        // `idle` waits for the message's delivery, its own turn included.
        let _active = self.channel.activity.enter(self.config.conversation());
        let content = EventContent::from(TextContent::new(text));
        let outcome = match self.admit(&content).await {
            Ok(()) => self.deliver(content).await,
            Err(refusal) => {
                self.notice(&refusal.message);
                TurnOutcome::Blocked
            }
        };
        if let Some(lifecycle) = &self.lifecycle {
            lifecycle
                .finished(&self.config, &self.operation, outcome)
                .await;
        }
    }

    async fn admit(&self, content: &EventContent) -> Result<(), Rejection> {
        match &self.lifecycle {
            Some(lifecycle) => {
                lifecycle
                    .admit(&self.config, &self.operation, content)
                    .await
            }
            None => Ok(()),
        }
    }

    async fn deliver(&self, content: EventContent) -> TurnOutcome {
        let (channel, config) = (&self.channel, &self.config);
        let room = config.conversation();
        let mut message = InboundMessage::new(channel.id().clone(), &config.sender, content);
        message.addressed_to = Some(vec![config.agent.clone()]);
        let key = [
            room,
            channel.id().as_str(),
            &config.sender,
            self.operation.as_str(),
        ];
        message.idempotency_key = Some(serde_json::json!(key).to_string());
        let options = ProcessInboundOptions {
            room_id: Some(config.room.clone()),
            organization_id: config.organization.clone(),
            defer_delivery: true,
        };
        match self.kit.process_inbound(message, options).await {
            Ok(admitted) => self.settle(admitted).await,
            Err(error) => {
                self.notice(crate::diagnostics::explain(&error.to_string()));
                TurnOutcome::Failed
            }
        }
    }

    /// Show the committed message, say which run it went to, and report a
    /// delivery that failed.
    async fn settle(&self, admitted: InboundResult) -> TurnOutcome {
        if admitted.blocked {
            self.notice("The engine blocked this message. Check the room state and host policy.");
            return TurnOutcome::Blocked;
        }
        let room = self.config.conversation();
        if let Some(row) = &admitted.event {
            self.channel.row(row);
            // The run under way is the agent's turn: no work of the
            // application's held the room. Should that turn no longer read
            // the message, its own turn's start names it.
            let agent = self.config.agent.as_str();
            if let Some(run) = self.channel.approvals.live_run(room, agent) {
                let event = Event::MessageSteered {
                    conversation: room.into(),
                    message: MessageId::new(row.id.to_string()),
                    run,
                };
                self.channel.publish(room, event);
            }
        }
        let Some(delivery) = admitted.delivery else {
            return TurnOutcome::Completed;
        };
        let failure = match delivery.wait().await {
            Ok(done) => done.error.map(|error| error.message),
            Err(error) => Some(error.to_string()),
        };
        match failure {
            Some(failure) => {
                self.notice(crate::diagnostics::explain(&failure));
                TurnOutcome::Failed
            }
            None => TurnOutcome::Completed,
        }
    }

    fn notice(&self, text: &str) {
        let room = self.config.conversation();
        let event = Event::Notice {
            conversation: room.into(),
            text: text.into(),
        };
        self.channel.publish(room, event);
    }
}
