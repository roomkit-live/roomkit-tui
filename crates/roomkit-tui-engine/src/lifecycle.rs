//! Application lifecycle callbacks, independent of the presentation event stream.

use std::{fmt::Debug, future::Future, pin::Pin, time::Duration};

use roomkit::model::EventContent;
use roomkit_tui_protocol::{Event, OperationId, Rejection, RunId, RunOutcome};

use crate::{ConversationChannel, SessionConfig};

/// A host callback whose resources remain owned by the application.
pub type LifecycleFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Definitive outcome of an admitted logical request, across all model/tool rounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TurnOutcome {
    /// Delivery completed successfully.
    Completed,
    /// Delivery or generation failed.
    Failed,
    /// Explicit interruption was accepted.
    Interrupted,
    /// The engine refused admission.
    Blocked,
}

/// Optional host lifecycle. Callbacks execute on the host, never in the renderer.
///
/// Detaching a client does not end its accepted turn. The application owns
/// session shutdown; there is deliberately no session-end callback on detach.
/// Implementations must bound their work. Dropping a preparation/start future
/// on interruption must clean up any resources it launched.
pub trait SessionLifecycle: Debug + Send + Sync {
    /// A view was successfully authorized. May be called again on reattachment;
    /// the host decides whether this starts a new session execution.
    fn attached(&self, _session: &SessionConfig) {}

    /// Check a validated input before it enters the engine/history. Errors are
    /// displayed to the user; return only safe, actionable explanations.
    fn admit<'a>(
        &'a self,
        _session: &'a SessionConfig,
        _operation: &'a OperationId,
        _content: &'a EventContent,
    ) -> LifecycleFuture<'a, Result<(), Rejection>> {
        Box::pin(async { Ok(()) })
    }

    /// Check a turn that answers what the conversation holds, with no new
    /// input (`Command::Answer`). `content` is what the agent is told, never
    /// stored or shown. By default it goes through [`Self::admit`], so a host's
    /// gate holds for every turn; a host that tells a person's words apart from
    /// a turn can refine it, and must still refuse what it would not run, and do
    /// what its `admit` prepares for `started` and `finished`.
    fn admit_answer<'a>(
        &'a self,
        session: &'a SessionConfig,
        operation: &'a OperationId,
        content: &'a EventContent,
    ) -> LifecycleFuture<'a, Result<(), Rejection>> {
        self.admit(session, operation, content)
    }

    /// Once per admitted logical turn, before engine processing. A matching
    /// `finished` follows on normal completion, failure or explicit interruption.
    /// A message handed to the running turn of an agent that steers its
    /// room's turns is admitted and finished like any message, but starts no
    /// turn: no `started`. Should that turn no longer read it, RoomKit gives
    /// it a turn of its own, and `started` comes once the agent has prepared
    /// that turn (memory, `BEFORE_AI_GENERATION` hooks), before its loop asks
    /// the model or calls a tool. An interruption drops it then, as it drops
    /// a preparation, and the turn ends `Interrupted`; its first model
    /// request may still go out, and its answer is dropped.
    fn started<'a>(
        &'a self,
        _session: &'a SessionConfig,
        _operation: &'a OperationId,
    ) -> LifecycleFuture<'a, ()> {
        Box::pin(async {})
    }

    /// After delivery and cancellation cleanup, before operation completion.
    /// Also called when admission is refused/interrupted without `started`, so
    /// the host can release preparation state without announcing a started turn.
    /// Observational: it cannot retroactively cancel effects or request a retry.
    fn finished<'a>(
        &'a self,
        _session: &'a SessionConfig,
        _operation: &'a OperationId,
        _outcome: TurnOutcome,
    ) -> LifecycleFuture<'a, ()> {
        Box::pin(async {})
    }
}

/// How long a preparation runs before the views are shown it as a run.
const SHOWN_AFTER: Duration = Duration::from_millis(80);

/// Announce preparation only when it lasts long enough to be useful. Fast or
/// disabled callbacks do not manufacture an extra visible model run.
pub(crate) async fn prepare(
    lifecycle: &dyn SessionLifecycle,
    session: &SessionConfig,
    operation: &OperationId,
    (content, answer): (&EventContent, bool),
    channel: &ConversationChannel,
    controls: &mut crate::cancellation::Receiver,
) -> Option<RunOutcome> {
    let run = crate::projection::bounded_run_id(format!("prepare:{operation}"));
    let mut shown = Shown {
        channel,
        room: session.room.as_str(),
        run: None,
    };
    let result = {
        let admitted = admission(lifecycle, session, operation, (content, answer));
        tokio::pin!(admitted);
        tokio::select! {
            result = &mut admitted => Some(result),
            () = tokio::time::sleep(SHOWN_AFTER) => {
                shown.announce(run.clone());
                crate::cancellation::prepare(&mut admitted, &run, controls).await
            }
        }
    };
    // Preparation is dropped before finalization, its resources cleaned.
    let outcome = concluded(lifecycle, session, operation, channel, result).await;
    shown.finish(outcome);
    (outcome != RunOutcome::Completed).then_some(outcome)
}

/// How a preparation ended, `None` when interrupted: the host hears of a
/// refusal or an interruption, and the views of a refusal's reason.
async fn concluded(
    lifecycle: &dyn SessionLifecycle,
    session: &SessionConfig,
    operation: &OperationId,
    channel: &ConversationChannel,
    result: Option<Result<(), Rejection>>,
) -> RunOutcome {
    let (ended, outcome) = match result {
        Some(Ok(())) => return RunOutcome::Completed,
        Some(Err(rejection)) => {
            let (room, text) = (session.room.as_str(), rejection.message);
            let conversation = room.into();
            channel.publish(room, Event::Notice { conversation, text });
            (TurnOutcome::Blocked, RunOutcome::Failed)
        }
        None => (TurnOutcome::Interrupted, RunOutcome::Interrupted),
    };
    lifecycle.finished(session, operation, ended).await;
    outcome
}

/// The host's admission of the turn, then its start; a turn with no new
/// message passes the host's gate for those.
async fn admission(
    lifecycle: &dyn SessionLifecycle,
    session: &SessionConfig,
    operation: &OperationId,
    (content, answer): (&EventContent, bool),
) -> Result<(), Rejection> {
    if answer {
        lifecycle.admit_answer(session, operation, content).await?;
    } else {
        lifecycle.admit(session, operation, content).await?;
    }
    lifecycle.started(session, operation).await;
    Ok(())
}

/// A preparation shown as a run. Dropped unfinished, as when the host's
/// callback panics, it reports a failure, so no view keeps the run going.
struct Shown<'a> {
    channel: &'a ConversationChannel,
    room: &'a str,
    run: Option<RunId>,
}

impl Shown<'_> {
    fn announce(&mut self, run: RunId) {
        let conversation = self.room.into();
        let started = Event::RunStarted {
            conversation,
            run: run.clone(),
            answers: None,
        };
        self.channel.publish(self.room, started);
        self.run = Some(run);
    }

    /// End the run shown, if the preparation was.
    fn finish(mut self, outcome: RunOutcome) {
        if let Some(run) = self.run.take() {
            self.end(run, outcome);
        }
    }

    fn end(&self, run: RunId, outcome: RunOutcome) {
        let conversation = self.room.into();
        let event = Event::RunFinished {
            conversation,
            run,
            outcome,
        };
        self.channel.publish(self.room, event);
    }
}

impl Drop for Shown<'_> {
    fn drop(&mut self) {
        if let Some(run) = self.run.take() {
            self.end(run, RunOutcome::Failed);
        }
    }
}
