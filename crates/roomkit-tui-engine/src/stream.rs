//! Engine deliveries to the channel: committed rows, or a streamed answer
//! with its drafts, finalized messages and run boundaries.

use roomkit::model::{ChannelBinding, ChannelOutput, RoomContext, RoomEvent};
use roomkit::{Channel, ChannelDeclaration, ChannelError, DeliveryStream, StreamItem};
use roomkit_tui_protocol::{ActorId, Event, Item, MessageId, RunId, RunOutcome};

use crate::{ConversationChannel, projection};

impl Channel for ConversationChannel {
    fn declaration(&self) -> ChannelDeclaration {
        self.declared().clone()
    }

    async fn deliver(
        &self,
        row: RoomEvent,
        _binding: ChannelBinding,
        _context: RoomContext,
    ) -> Result<ChannelOutput, ChannelError> {
        self.row(&row);
        Ok(ChannelOutput::default())
    }

    async fn deliver_stream(
        &self,
        mut stream: DeliveryStream,
        binding: ChannelBinding,
        context: RoomContext,
    ) -> Result<ChannelOutput, ChannelError> {
        let room = binding.room_id.to_string();
        let run = projection::run_id(stream.responder(), stream.answers());
        let active = self.activity.enter(&room);
        // The stream's trigger is already committed and visible to this channel.
        let trigger = context
            .recent_events
            .iter()
            .find(|r| &r.id == stream.answers());
        if let Some(trigger) = trigger {
            self.row(trigger);
        }
        let conversation = room.as_str().into();
        let started = Event::RunStarted {
            conversation,
            run: run.clone(),
        };
        // Announce the run before the broker records it: a refusal is told
        // once the run exists, and a terminal clears its notice on RunStarted.
        self.publish(&room, started);
        let requester = trigger.and_then(|row| self.requester(row));
        let agent = stream.responder().as_str().to_owned();
        let approvals = self.approvals.start(&room, &agent, run.clone(), requester);
        let mut reader = Reader::new(self, &room, &run, agent.as_str().into());
        while let Some(next) = stream.next().await {
            match next {
                Ok(item) => reader.read(item),
                Err(error) => {
                    reader.fail(&error.to_string());
                    reader.failure = Some(error);
                    break;
                }
            }
        }
        drop((approvals, active));
        reader.finish()
    }
}

impl ConversationChannel {
    /// The person who sent a trigger through this channel, if any. Only they
    /// may answer approvals of the run it starts.
    fn requester(&self, trigger: &RoomEvent) -> Option<String> {
        (&trigger.source.channel_id == self.id())
            .then_some(trigger.source.participant_id.as_ref())
            .flatten()
            .map(ToString::to_string)
    }
}

/// How an agent loop ended, from its final row, and what to tell the person.
pub(crate) fn loop_end(row: &RoomEvent) -> Option<(RunOutcome, Option<&'static str>)> {
    let reason = row.metadata.get("loop_end_reason")?.as_str()?;
    Some(match reason {
        "completed" => (RunOutcome::Completed, None),
        "cancelled" => (RunOutcome::Interrupted, None),
        "max_rounds" => (
            RunOutcome::Failed,
            Some(
                "Tool round limit reached. Completed actions are retained; send a message to continue.",
            ),
        ),
        "truncated" => (
            RunOutcome::Failed,
            Some("The model stopped at its output or context limit. Send a message to continue."),
        ),
        "empty_response" => (
            RunOutcome::Failed,
            Some(
                "The model returned no final answer after using tools. Send a message to continue.",
            ),
        ),
        _ => (
            RunOutcome::Failed,
            Some("The model execution failed after partial work. Completed actions are retained."),
        ),
    })
}

/// Projects one stream into events: drafts, finalized messages and items.
struct Reader<'a> {
    channel: &'a ConversationChannel,
    room: &'a str,
    run: &'a RunId,
    actor: ActorId,
    segment: u64,
    outcome: RunOutcome,
    failure: Option<ChannelError>,
}

impl<'a> Reader<'a> {
    fn new(
        channel: &'a ConversationChannel,
        room: &'a str,
        run: &'a RunId,
        actor: ActorId,
    ) -> Self {
        Self {
            channel,
            room,
            run,
            actor,
            segment: 0,
            outcome: RunOutcome::Completed,
            failure: None,
        }
    }

    fn draft(&self) -> MessageId {
        MessageId::new(format!("draft:{}:{}", self.run, self.segment))
    }

    fn publish(&self, event: Event) {
        self.channel.publish(self.room, event);
    }

    fn read(&mut self, item: StreamItem) {
        match item {
            StreamItem::Text(text) => self.publish(Event::TextDelta {
                conversation: self.room.into(),
                message: self.draft(),
                actor: self.actor.clone(),
                run: self.run.clone(),
                text,
            }),
            StreamItem::Row(row) => self.row(&row),
            _ => {}
        }
    }

    fn row(&mut self, row: &RoomEvent) {
        if let Some(event) = crate::measurements::event(row) {
            self.publish(event);
        }
        if let Some((outcome, notice)) = loop_end(row) {
            self.outcome = outcome;
            if let Some(text) = notice {
                self.notice(text);
            }
        }
        match projection::item(row) {
            Some(Item::Message(message)) => {
                self.publish(Event::MessageFinalized {
                    conversation: self.room.into(),
                    draft: self.draft(),
                    message,
                });
                self.segment += 1;
            }
            Some(item) => self.publish(Event::Item {
                conversation: self.room.into(),
                item,
            }),
            None => {}
        }
    }

    fn notice(&self, text: &str) {
        self.publish(Event::Notice {
            conversation: self.room.into(),
            text: text.into(),
        });
    }

    fn fail(&mut self, error: &str) {
        self.outcome = RunOutcome::Failed;
        self.notice(crate::diagnostics::explain(error));
    }

    /// End the run, unless an accepted interruption reports it once the
    /// engine has settled its cancelled rows.
    fn finish(self) -> Result<ChannelOutput, ChannelError> {
        let deferred = self
            .channel
            .stopping
            .defer(self.room, self.run, self.outcome, self.draft());
        if !deferred {
            self.publish(Event::RunFinished {
                conversation: self.room.into(),
                run: self.run.clone(),
                outcome: self.outcome,
            });
        }
        match self.failure {
            Some(error) => Err(error),
            None => Ok(ChannelOutput::default()),
        }
    }
}
