//! A message sent while the agent's turn runs (RMK-1161). The agent steers
//! its room's turns, so RoomKit hands the message to that turn, which reads
//! it at its next step. Should the turn no longer read its queue, the
//! message waits a turn of its own after it, which this view can stop and
//! the host hears start (RMK-1232). Only a turn of this view's agent,
//! streamed to this console and not being interrupted, takes a message this
//! way; anything else in the room keeps the message busy, as before.

use std::sync::Arc;

use roomkit::model::{EventContent, InboundMessage, TextContent};
use roomkit::{ConversationStore, InboundResult, ProcessInboundOptions, RoomKit};
use roomkit_tui_protocol::{Command, Event, MessageId, OperationId, Rejection, Request, RunId};

use super::serve::View;
use super::{AdapterError, CommandHandler, SessionConfig};
use crate::activity::Active;
use crate::cancellation::{self, Receiver};
use crate::execution::Interrupter;
use crate::{ConversationChannel, SessionLifecycle, TurnOutcome, projection};

/// The turns of steered messages this view can stop at once: each lasts
/// until its message's delivery ends.
const MAX_STEERING: usize = 16;

impl<S: ConversationStore, H: CommandHandler> View<'_, S, H> {
    /// The turn `request` may go to, refused a reservation because a turn
    /// holds the room: for a message to an agent that steers its room's
    /// turns, that agent's run streamed to this console, unless it is being
    /// interrupted.
    pub(super) fn steering_target(&self, request: &Request) -> Option<RunId> {
        let (channel, config) = (&self.session.channel, &self.session.config);
        if !matches!(request.command, Command::Submit { .. }) || !config.can_steer {
            return None;
        }
        let room = config.conversation();
        let run = channel.approvals.live_run(room, config.agent.as_str())?;
        // Being stopped, it reads nothing more.
        (!channel.stopping.contains(room, &run)).then_some(run)
    }

    /// Accept the validated message for the turn `run`.
    pub(super) fn steer(&mut self, request: &Request, run: RunId) -> Result<(), AdapterError> {
        let Command::Submit { text, .. } = &request.command else {
            return Ok(());
        };
        self.steering.retain(Interrupter::is_live);
        if self.steering.len() >= MAX_STEERING {
            // Each steered message keeps its own turn stoppable.
            let error = Rejection::busy("Too many messages are waiting for the running turn");
            return self.reply(request, Some(error));
        }
        // Held before the reply: `idle` waits for the message from then on.
        let active = self
            .session
            .channel
            .activity
            .enter(self.session.config.conversation());
        let (control, controls) = cancellation::connection();
        self.steering.push(Interrupter::Engine(control));
        self.reply(request, None)?;
        let steer = Steer {
            kit: self.session.kit.clone(),
            channel: self.session.channel.clone(),
            config: self.session.config.clone(),
            lifecycle: self.session.lifecycle.clone(),
            operation: request.operation.clone(),
            turn: run,
            controls,
            _active: active,
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
    /// The turn the message is handed to.
    turn: RunId,
    /// Interruptions of the message's own turn, should it get one.
    controls: Receiver,
    _active: Active,
}

impl<S: ConversationStore> Steer<S> {
    /// Admit the message as the host admits every message, deliver it, and
    /// tell the host how it ended. Handed to the running turn, it starts no
    /// turn: no `started`. Given a turn of its own, the stream carrying that
    /// turn calls `started` before its first read (`own_turns`).
    async fn run(mut self, text: String) {
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

    async fn deliver(&mut self, content: EventContent) -> TurnOutcome {
        let key = self.idempotency_key();
        // Its own turn, should it get one, starts as any turn.
        let _own_turn = self.lifecycle.clone().map(|lifecycle| {
            let (config, operation) = (self.config.clone(), self.operation.clone());
            self.channel
                .own_turns
                .expect(&key, lifecycle, config, operation)
        });
        let (channel, config) = (&self.channel, &self.config);
        let mut message = InboundMessage::new(channel.id().clone(), &config.sender, content);
        message.addressed_to = Some(vec![config.agent.clone()]);
        message.idempotency_key = Some(key);
        let options = ProcessInboundOptions {
            room_id: Some(config.room.clone()),
            organization_id: config.organization.clone(),
            defer_delivery: true,
        };
        let outcome = match self.kit.process_inbound(message, options).await {
            Ok(admitted) => self.settle(admitted).await,
            Err(error) => {
                self.notice(crate::diagnostics::explain(&error.to_string()));
                TurnOutcome::Failed
            }
        };
        // Decisions taken in the message's own turn are kept, as a turn's.
        let stored = crate::approval_history::persist(&self.kit, &self.channel, &self.config);
        if stored.await.is_err() {
            self.notice("Decision accepted, but its history could not be saved. This does not change tool execution.");
        }
        outcome
    }

    /// Show the committed message, say which turn it went to, and wait for
    /// its delivery, which a turn of its own may extend.
    async fn settle(&mut self, admitted: InboundResult) -> TurnOutcome {
        if admitted.blocked {
            self.notice("The engine blocked this message. Check the room state and host policy.");
            return TurnOutcome::Blocked;
        }
        let Some(row) = &admitted.event else {
            return TurnOutcome::Completed;
        };
        self.channel.row(row);
        let room = self.config.conversation();
        let event = Event::MessageSteered {
            conversation: room.into(),
            message: MessageId::new(row.id.to_string()),
            run: self.turn.clone(),
        };
        self.channel.publish(room, event);
        let Some(delivery) = admitted.delivery else {
            return TurnOutcome::Completed;
        };
        // The run a turn of its own would be, which Esc may stop.
        let own = projection::run_id(&self.config.agent, &row.id);
        let room = room.to_owned();
        let (done, interrupted) =
            cancellation::wait(delivery, own, &room, &self.channel, &mut self.controls).await;
        let failure = match done {
            Ok(done) => done.error.map(|error| error.message),
            Err(error) => Some(error.to_string()),
        };
        match failure {
            Some(failure) => {
                self.notice(crate::diagnostics::explain(&failure));
                TurnOutcome::Failed
            }
            None if interrupted => TurnOutcome::Interrupted,
            None => TurnOutcome::Completed,
        }
    }

    /// The message's key in the room, from the operation it was sent as.
    fn idempotency_key(&self) -> String {
        let config = &self.config;
        let key = [
            config.conversation(),
            self.channel.id().as_str(),
            &config.sender,
            self.operation.as_str(),
        ];
        serde_json::json!(key).to_string()
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
