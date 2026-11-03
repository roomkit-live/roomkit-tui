//! A run stopped while its answer is still on its way to the console.

use super::*;
use tokio::sync::{Notify, watch};

/// Hold the agent's answers as they are stored, telling `held` once one
/// waits for `gate`.
fn hold_answers(kit: &RoomKit, gate: watch::Receiver<bool>, held: Arc<Notify>) {
    kit.hook(
        roomkit::HookTrigger::BeforeBroadcast,
        move |event: roomkit::model::RoomEvent, _| {
            let (mut gate, held) = (gate.clone(), held.clone());
            let answer = event.source.channel_id == config().agent;
            async move {
                if answer {
                    held.notify_one();
                    let _ = gate.wait_for(|open| *open).await;
                }
                Ok(roomkit::model::HookResult::allow(Default::default()))
            }
        },
        hook_options("hold-answers"),
    )
    .unwrap();
}

/// The review of 56f6268: since roomkit-rs RMK-1152, a stopped delivery's
/// wait returns once its turns are over, while its answer still streams to
/// the console. The engine reported the run before its stream ended: the
/// stream then ended it as completed, or nobody ended it.
#[tokio::test]
async fn a_run_stopped_as_its_answer_is_stored_ends_once_interrupted() {
    let kit = RoomKit::new(Default::default());
    let provider = ScriptedProvider::new([ScriptedRound::answering("Partial answer")]);
    let channel = prepare(&kit, provider, Arc::default(), false).await;
    let (open, gate) = watch::channel(false);
    let held = Arc::new(Notify::new());
    hold_answers(&kit, gate, held.clone());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    next(&client).await;
    client.try_send(request("ask")).unwrap();
    let mut events = Vec::new();
    let run = loop {
        let event = next(&client).await;
        let started = match &event {
            Event::RunStarted { run, .. } => Some(run.clone()),
            _ => None,
        };
        events.push(event);
        if let Some(run) = started {
            break run;
        }
    };
    held.notified().await;
    let stop = Command::Interrupt { run: run.clone() };
    client
        .try_send(Request::new(RequestId::new(2), "stop", "room", stop))
        .unwrap();
    // Give the engine the time to settle the run before its stream ends.
    tokio::time::sleep(Duration::from_millis(50)).await;
    open.send_replace(true);
    events.extend(collect_operation(&client).await);
    let ends: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::RunFinished {
                run: ended,
                outcome,
                ..
            } if *ended == run => Some(*outcome),
            _ => None,
        })
        .collect();
    assert_eq!(
        ends,
        [RunOutcome::Interrupted],
        "Ended once, interrupted, before its operation: {events:#?}"
    );
    assert_operation(&events, "ask", RunOutcome::Interrupted);
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}
