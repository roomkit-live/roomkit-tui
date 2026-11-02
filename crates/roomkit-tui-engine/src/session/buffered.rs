//! Run boundaries a turn reports itself when the channel is not streaming.

use roomkit::InboundResult;
use roomkit_tui_protocol::{Event, RunId, RunOutcome};

use crate::ConversationChannel;

/// A run announced by the turn itself when the channel is not streaming.
/// Dropped unfinished, as when the turn panics, it reports a failure.
pub(super) struct Buffered<'a> {
    channel: &'a ConversationChannel,
    room: &'a str,
    run: Option<RunId>,
}

impl<'a> Buffered<'a> {
    pub(super) fn start(
        channel: &'a ConversationChannel,
        room: &'a str,
        run: Option<RunId>,
    ) -> Self {
        if let Some(run) = &run {
            let started = Event::RunStarted {
                conversation: room.into(),
                run: run.clone(),
                answers: None,
            };
            channel.publish(room, started);
        }
        Self { channel, room, run }
    }

    /// End the run with the agent loop's own verdict, which a buffered
    /// delivery reports only in its final row. Without a stream, nothing
    /// else reports an interrupted run.
    pub(super) fn finish(mut self, admitted: &InboundResult, outcome: RunOutcome) {
        let Some(run) = self.run.take() else {
            return;
        };
        let mut outcome = outcome;
        let verdict = admitted
            .response_events
            .iter()
            .rev()
            .find_map(crate::stream::loop_end);
        if let Some((ended, notice)) = verdict.filter(|_| outcome == RunOutcome::Completed) {
            outcome = ended;
            if let Some(text) = notice {
                let event = Event::Notice {
                    conversation: self.room.into(),
                    text: text.into(),
                };
                self.channel.publish(self.room, event);
            }
        }
        self.end(run, outcome);
    }

    fn end(&self, run: RunId, outcome: RunOutcome) {
        let event = Event::RunFinished {
            conversation: self.room.into(),
            run,
            outcome,
        };
        self.channel.publish(self.room, event);
    }
}

impl Drop for Buffered<'_> {
    fn drop(&mut self) {
        if let Some(run) = self.run.take() {
            self.end(run, RunOutcome::Failed);
        }
    }
}
