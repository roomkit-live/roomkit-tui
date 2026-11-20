//! How a message's turn ended, judged the same for every turn a view
//! delivers, its own or a steered message's: the outcome the views and the
//! host are told, and the failure the views are told once.

use roomkit::InboundResult;
use roomkit_tui_protocol::{Event, RunId, RunOutcome};

use crate::cancellation::{self, Receiver};
use crate::{ConversationChannel, TurnOutcome};

/// What the views are told of a message the engine refused.
const BLOCKED: &str = "The engine blocked this message. Check the room state and host policy.";
/// What the views are told of a delivery cut by anything but the person.
const CANCELLED: &str = "Execution cancelled. Completed tool actions are retained.";

/// A message's delivery, waited until its end.
#[derive(Debug, Default)]
pub(super) struct Settled {
    /// The delivery's end, or the message's commit when waiting failed.
    pub result: InboundResult,
    /// Why waiting for the end failed.
    lost: Option<String>,
    /// The engine accepted the person's interruption of the run.
    interrupted: bool,
}

impl Settled {
    /// `admitted`, whose delivery nobody waits for.
    pub(super) fn now(admitted: InboundResult) -> Self {
        Self {
            result: admitted,
            ..Self::default()
        }
    }

    /// Wait for the delivery of `admitted`, the person able to interrupt
    /// its `run` in `room` through `controls` meanwhile.
    pub(super) async fn wait(
        admitted: InboundResult,
        run: RunId,
        room: &str,
        channel: &ConversationChannel,
        controls: &mut Receiver,
    ) -> Self {
        let Some(delivery) = admitted.delivery.clone() else {
            return Self::now(admitted);
        };
        let (result, interrupted) =
            cancellation::wait(delivery, run, room, channel, controls).await;
        let (result, lost) = match result {
            Ok(done) => (done, None),
            Err(error) => (admitted, Some(error.to_string())),
        };
        Self {
            result,
            lost,
            interrupted,
        }
    }
}

/// Explain `error` to the views of `room`. A refusal past the model's
/// context window is also told with its `run`, before the run ends; a
/// streamed run tells it from its stream.
pub(super) fn explained(
    channel: &ConversationChannel,
    room: &str,
    run: Option<&RunId>,
    error: &str,
) -> String {
    let diagnostic = crate::diagnostics::Diagnostic::classify(error);
    if diagnostic.exceeds_context()
        && !channel.streams()
        && let Some(run) = run
    {
        let conversation = room.into();
        let run = run.clone();
        channel.publish(room, Event::ContextExceeded { conversation, run });
    }
    crate::diagnostics::told(&diagnostic, error)
}

/// How one message's turn ended.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Verdict {
    pub outcome: RunOutcome,
    /// Why it failed, as the views are told.
    failure: Option<String>,
    /// The engine refused the message.
    blocked: bool,
    /// The run's stream already told the views why it failed.
    told: bool,
}

impl Verdict {
    /// A message the engine never took, for the reason `failure` says.
    pub(super) fn failed(failure: String) -> Self {
        Self {
            outcome: RunOutcome::Failed,
            failure: Some(failure),
            blocked: false,
            told: false,
        }
    }

    /// How the delivery `settled` ended. Only a cut the person asked for,
    /// and the engine accepted, ends a turn interrupted: any other cut, as
    /// the kit's close, fails it. `explain` words an engine or provider
    /// error.
    pub(super) fn judge(settled: &Settled, explain: impl FnOnce(&str) -> String) -> Self {
        let result = &settled.result;
        let cancelled = result.cancellation_reason.as_deref();
        let stopped = settled.interrupted && cancelled == Some("caller_cancelled");
        let failure = if let Some(error) = &result.error {
            Some(explain(&error.message))
        } else if result.blocked {
            Some(BLOCKED.to_owned())
        } else if cancelled.is_some() && !stopped {
            Some(CANCELLED.to_owned())
        } else {
            settled.lost.as_deref().map(explain)
        };
        let outcome = match (&failure, stopped) {
            (Some(_), _) => RunOutcome::Failed,
            (None, true) => RunOutcome::Interrupted,
            (None, false) => RunOutcome::Completed,
        };
        Self {
            outcome,
            failure,
            blocked: result.blocked,
            told: false,
        }
    }

    /// Note whether the stream of `run` in `room` already told the views
    /// why it failed; the turn asking is the last to need it.
    pub(super) fn heard_from(
        mut self,
        channel: &ConversationChannel,
        room: &str,
        run: Option<&RunId>,
    ) -> Self {
        self.told =
            self.failure.is_some() && run.is_some_and(|run| channel.take_failure_told(room, run));
        self
    }

    /// Tell the views of `room` why the turn failed, unless its stream did.
    pub(super) fn tell(&self, channel: &ConversationChannel, room: &str) {
        let Some(text) = self.failure.as_ref().filter(|_| !self.told) else {
            return;
        };
        let event = Event::Notice {
            conversation: room.into(),
            text: text.clone(),
        };
        channel.publish(room, event);
    }

    /// How the host is told the turn ended.
    pub(super) fn ended(&self) -> TurnOutcome {
        match self.outcome {
            _ if self.blocked => TurnOutcome::Blocked,
            RunOutcome::Completed => TurnOutcome::Completed,
            RunOutcome::Interrupted => TurnOutcome::Interrupted,
            _ => TurnOutcome::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use roomkit::model::ErrorRecord;

    use super::*;

    /// A delivery cut for `reason`, the person's interruption accepted or not.
    fn cut(reason: Option<&str>, interrupted: bool) -> Settled {
        let result = InboundResult {
            cancellation_reason: reason.map(str::to_owned),
            ..Default::default()
        };
        Settled {
            result,
            lost: None,
            interrupted,
        }
    }

    fn judged(settled: &Settled) -> (RunOutcome, Option<String>) {
        let verdict = Verdict::judge(settled, str::to_owned);
        (verdict.outcome, verdict.failure)
    }

    #[test]
    fn only_the_person_s_accepted_interruption_ends_a_turn_interrupted() {
        let stopped = cut(Some("caller_cancelled"), true);
        assert_eq!(judged(&stopped), (RunOutcome::Interrupted, None));
        let closed = (RunOutcome::Failed, Some(CANCELLED.to_owned()));
        assert_eq!(judged(&cut(Some("kit_closed"), true)), closed);
        assert_eq!(judged(&cut(Some("kit_closed"), false)), closed);
        let ran = cut(None, true);
        assert_eq!(judged(&ran), (RunOutcome::Completed, None));
    }

    #[test]
    fn an_error_wins_over_a_block_a_cut_and_a_lost_wait() {
        let mut settled = cut(Some("caller_cancelled"), true);
        settled.result.blocked = true;
        settled.result.error = Some(ErrorRecord::new("ProviderError", "connect error"));
        settled.lost = Some("lost".to_owned());
        let verdict = Verdict::judge(&settled, |e| format!("said {e}"));
        assert_eq!(verdict.failure.as_deref(), Some("said connect error"));
        assert_eq!(verdict.ended(), TurnOutcome::Blocked);
        let mut lost = Settled::now(InboundResult::default());
        lost.lost = Some("lost".to_owned());
        assert_eq!(judged(&lost), (RunOutcome::Failed, Some("lost".to_owned())));
    }
}
