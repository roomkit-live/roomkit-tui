//! Scoped requests use the engine's delivery handle. An acknowledgment only
//! asks the cut; an interrupted run is reported once its cut turns are over
//! and its stream to this console has ended.
use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use roomkit::{CancelOptions, DeliveryHandle, InboundResult, RoomKitError};
use roomkit_tui_protocol::{Event, RunId, RunOutcome};
use tokio::sync::{mpsc, oneshot};

use crate::ConversationChannel;
use crate::activity::StopGuard;

type Answer = Result<(), &'static str>;

const NOT_ACTIVE: &str = "This execution is no longer active in this attachment";
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
                let _ = answer.send(Err(NOT_ACTIVE));
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
                let stop = Stop { delivery: &delivery, run: &run, room, channel };
                let _ = answer.send(stop.interrupt(&requested, &mut stopping));
            }
        }
    };
    let Some(guard) = stopping else {
        return (result, false);
    };
    // RoomKit's wait returns once the cut turns are over, while the run's
    // answer may still stream here: its stream decides whether it is the
    // one reporting the run.
    stream_ended(&run, room, channel, controls).await;
    // A stream that ended before the interruption already reported its run.
    if guard.release().is_some() {
        finished(channel, room, &run, outcome(&result));
    }
    (result, true)
}

/// The interruption of one run, as its delivery is waited.
struct Stop<'a> {
    delivery: &'a DeliveryHandle,
    run: &'a RunId,
    room: &'a str,
    channel: &'a ConversationChannel,
}

impl Stop<'_> {
    /// Answer a request to interrupt `requested`: the run is asked to stop
    /// once, and `stopping` then keeps its stream's end for the interruption.
    fn interrupt(&self, requested: &RunId, stopping: &mut Option<StopGuard>) -> Answer {
        let Stop {
            delivery,
            run,
            room,
            channel,
        } = *self;
        if requested != run {
            return Err(NOT_ACTIVE);
        }
        if stopping.is_some() {
            return Ok(());
        }
        // Suppress a stream-end event while the engine still has
        // finalizers and cancelled tool rows to settle.
        let guard = channel.stopping.register(room, run);
        if ask_cancel(delivery) {
            // Only now: a stream woken earlier reads on before the engine
            // applies the cancel.
            channel.stopping.accepted();
            *stopping = Some(guard);
            channel.approvals.stop_turn(room, run);
            return Ok(());
        }
        // The stream may have ended, deferring its end, just before the
        // engine refused: report it now. Reading and removing the entry at
        // once leaves no gap for it.
        if let Some(outcome) = guard.release() {
            finished(channel, room, run, outcome);
        }
        Err("This execution cannot be interrupted or has already ended")
    }
}

/// Wait until the stream of `run` to this console has ended, still
/// acknowledging a repeated interruption of it.
async fn stream_ended(
    run: &RunId,
    room: &str,
    channel: &ConversationChannel,
    controls: &mut Receiver,
) {
    let ended = channel.live_streams.ended(room, run);
    tokio::pin!(ended);
    loop {
        tokio::select! {
            () = &mut ended => return,
            Some((requested, answer)) = controls.recv() => {
                let stopping = if &requested == run { Ok(()) } else { Err(NOT_ACTIVE) };
                let _ = answer.send(stopping);
            }
        }
    }
}

/// How an interrupted run ended, from its delivery's result.
fn outcome(result: &Result<InboundResult, RoomKitError>) -> RunOutcome {
    match result {
        Ok(done) if done.error.is_none() => match done.cancellation_reason.as_deref() {
            Some("caller_cancelled") => RunOutcome::Interrupted,
            None => RunOutcome::Completed,
            Some(_) => RunOutcome::Failed,
        },
        Err(_) | Ok(_) => RunOutcome::Failed,
    }
}

/// Ask `delivery` to cut its turns, without waiting for them to be over:
/// whether it was asked. RoomKit's `cancel` asks the cut as it is first
/// polled, then waits for the turns; dropped then, the cut stands, and
/// `wait` joins the turns it cut.
fn ask_cancel(delivery: &DeliveryHandle) -> bool {
    if delivery.done() {
        return false;
    }
    let mut cancel = pin!(delivery.cancel(CancelOptions::default()));
    let polled = cancel
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()));
    // Refused from the room's own work: nothing is cut.
    !matches!(polled, Poll::Ready(Err(RoomKitError::Runtime(_))))
}

fn finished(channel: &ConversationChannel, room: &str, run: &RunId, outcome: RunOutcome) {
    let event = Event::RunFinished {
        conversation: room.into(),
        run: run.clone(),
        outcome,
    };
    channel.publish(room, event);
}
