//! Application lifecycle callbacks, independent of the presentation event stream.

use std::{fmt::Debug, future::Future, pin::Pin};

use roomkit::model::EventContent;
use roomkit_tui_protocol::OperationId;

use crate::SessionConfig;

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
    ) -> LifecycleFuture<'a, Result<(), roomkit_tui_protocol::Rejection>> {
        Box::pin(async { Ok(()) })
    }

    /// Once per admitted logical turn, before engine processing. A matching
    /// `finished` follows on normal completion, failure or explicit interruption.
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

/// Announce preparation only when it lasts long enough to be useful. Fast or
/// disabled callbacks do not manufacture an extra visible model run.
pub(crate) async fn prepare(
    lifecycle: &dyn SessionLifecycle,
    session: &SessionConfig,
    operation: &OperationId,
    content: &EventContent,
    channel: &crate::ConversationChannel,
    controls: &mut crate::cancellation::Receiver,
) -> Option<roomkit_tui_protocol::RunOutcome> {
    use roomkit_tui_protocol::{Event, RunOutcome};
    let run = crate::projection::bounded_run_id(format!("prepare:{operation}"));
    let mut announced = false;
    let result = {
        let future = async {
            lifecycle.admit(session, operation, content).await?;
            lifecycle.started(session, operation).await;
            Ok::<_, roomkit_tui_protocol::Rejection>(())
        };
        tokio::pin!(future);
        tokio::select! {
            result = &mut future => Some(result),
            () = tokio::time::sleep(std::time::Duration::from_millis(80)) => {
                announced = true;
                channel.publish(session.room.as_str(), Event::RunStarted { conversation: session.room.as_str().into(), run: run.clone() });
                crate::cancellation::prepare(&mut future, &run, controls).await
            }
        }
    };
    // Drop preparation before finalization so its resources are already cleaned.
    let outcome = match result {
        Some(Ok(())) => RunOutcome::Completed,
        Some(Err(rejection)) => {
            let conversation = session.room.as_str().into();
            let notice = Event::Notice {
                conversation,
                text: rejection.message,
            };
            channel.publish(session.room.as_str(), notice);
            lifecycle
                .finished(session, operation, TurnOutcome::Blocked)
                .await;
            RunOutcome::Failed
        }
        None => {
            lifecycle
                .finished(session, operation, TurnOutcome::Interrupted)
                .await;
            RunOutcome::Interrupted
        }
    };
    if announced {
        channel.publish(
            session.room.as_str(),
            Event::RunFinished {
                conversation: session.room.as_str().into(),
                run,
                outcome,
            },
        );
    }
    (outcome != RunOutcome::Completed).then_some(outcome)
}
