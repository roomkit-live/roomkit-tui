//! Scoped requests use the engine's delivery handle; acknowledgments never
//! claim completion before its joined cleanup and durable rows have settled.
use roomkit::{DeliveryHandle, InboundResult, RoomKitError};
use roomkit_tui_protocol::{Event, RunId, RunOutcome};
use tokio::sync::{mpsc, oneshot};

use crate::ConversationChannel;

type Answer = Result<(), &'static str>;
type Request = (RunId, oneshot::Sender<Answer>);
pub(crate) type Receiver = mpsc::UnboundedReceiver<Request>;

#[derive(Clone)]
pub(crate) struct Control(mpsc::UnboundedSender<Request>);

pub(crate) fn connection() -> (Control, Receiver) {
    let (sender, receiver) = mpsc::unbounded_channel();
    (Control(sender), receiver)
}

impl Control {
    /// Whether the work it controls has ended.
    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }

    pub async fn interrupt(&self, run: RunId) -> Answer {
        let (answer, received) = oneshot::channel();
        self.0
            .send((run, answer))
            .map_err(|_| "This execution has already ended")?;
        received
            .await
            .unwrap_or(Err("This execution has already ended"))
    }
}

/// Run host preparation while keeping scoped interruption responsive. Dropping
/// the future cancels preparation; disconnect alone deliberately does not.
pub(crate) async fn prepare<T>(
    future: impl std::future::Future<Output = T>,
    run: &RunId,
    controls: &mut Receiver,
) -> Option<T> {
    tokio::pin!(future);
    loop {
        tokio::select! {
            result = &mut future => return Some(result),
            Some((requested, answer)) = controls.recv() => {
                if &requested == run {
                    let _ = answer.send(Ok(()));
                    return None;
                }
                let _ = answer.send(Err("This execution is no longer active in this attachment"));
            }
        }
    }
}

/// Returns whether explicit interruption was accepted, as well as completion.
pub(crate) async fn wait(
    delivery: DeliveryHandle,
    run: RunId,
    room: &str,
    channel: &ConversationChannel,
    controls: &mut Receiver,
) -> (Result<InboundResult, RoomKitError>, bool) {
    let mut stopping = None;
    let result = loop {
        tokio::select! {
            result = delivery.wait() => break result,
            Some((requested, answer)) = controls.recv() => {
                let result = if requested != run {
                    Err("This execution is no longer active in this attachment")
                } else if stopping.is_some() {
                    Ok(())
                } else {
                    // Suppress a stream-end event while the engine still has
                    // finalizers and cancelled tool rows to settle.
                    let guard = channel.stopping.register(room, &run);
                    if delivery.request_cancel() {
                        stopping = Some(guard);
                        channel.approvals.stop_turn(room, &run);
                        Ok(())
                    } else {
                        // The stream may have ended, deferring its end, just
                        // before the engine refused: report it now. Reading and
                        // removing the entry at once leaves no gap for it.
                        if let Some(outcome) = guard.release() {
                            finished(channel, room, &run, outcome);
                        }
                        Err("This execution cannot be interrupted or has already ended")
                    }
                };
                let _ = answer.send(result);
            }
        }
    };
    let interrupted = stopping.is_some();
    // A stream that ended before the interruption already reported its run.
    if stopping
        .as_ref()
        .is_some_and(|guard| guard.deferred().is_some())
    {
        let outcome = match &result {
            Ok(done) if done.error.is_none() => match done.cancellation_reason.as_deref() {
                Some("caller_cancelled") => RunOutcome::Interrupted,
                None => RunOutcome::Completed,
                Some(_) => RunOutcome::Failed,
            },
            Err(_) | Ok(_) => RunOutcome::Failed,
        };
        finished(channel, room, &run, outcome);
    }
    (result, interrupted)
}

fn finished(channel: &ConversationChannel, room: &str, run: &RunId, outcome: RunOutcome) {
    let event = Event::RunFinished {
        conversation: room.into(),
        run: run.clone(),
        outcome,
    };
    channel.publish(room, event);
}
