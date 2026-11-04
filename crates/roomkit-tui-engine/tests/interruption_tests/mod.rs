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

fn used(input: u64) -> roomkit::model::Metadata {
    json!({ "input_tokens": input, "output_tokens": 10 })
        .as_object()
        .unwrap()
        .clone()
}

/// RMK-1333: three steps at 1000, 2000 and 3000 tokens, the second held as
/// its text is stored, then stopped: the context follows each step as it
/// goes, and the stop keeps what the steps told.
#[tokio::test]
async fn a_run_tells_its_context_at_each_step_and_a_stop_keeps_it() {
    let kit = RoomKit::new(Default::default());
    let provider = NamedProvider(ScriptedProvider::new([
        ScriptedRound::calling("One.", [("c1", "lookup_record", "{}")]).with_usage(used(1000)),
        ScriptedRound::calling("Two.", [("c2", "lookup_record", "{}")]).with_usage(used(2000)),
        ScriptedRound::answering("Three.").with_usage(used(3000)),
    ]));
    let channel = prepare(&kit, provider, Arc::default(), false).await;
    let (open, gate) = watch::channel(false);
    let held = Arc::new(Notify::new());
    let on_hold = held.clone();
    kit.hook(
        roomkit::HookTrigger::BeforeBroadcast,
        move |event: roomkit::model::RoomEvent, _| {
            let (mut gate, on_hold) = (gate.clone(), on_hold.clone());
            let second = event.source.channel_id == config().agent
                && roomkit_core::rules::transcoding::text_of(&event.content) == "Two.";
            async move {
                if second {
                    on_hold.notify_one();
                    let _ = gate.wait_for(|open| *open).await;
                }
                Ok(roomkit::model::HookResult::allow(Default::default()))
            }
        },
        hook_options("hold-the-second-step"),
    )
    .unwrap();
    let mut config = config();
    config.model = Some(ModelInfo::new("local", "fixture"));
    config.context_capacity = Some(10_000);
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config).run(host));
    next(&client).await;
    client.try_send(request("ask")).unwrap();

    let mut told = Vec::new();
    let mut run = None;
    while told.len() < 2 {
        match next(&client).await {
            Event::RunStarted { run: started, .. } => run = Some(started),
            Event::ContextUpdated {
                run: measured,
                usage,
                ..
            } => {
                assert_eq!(Some(&measured), run.as_ref(), "The view keeps this run's");
                assert_eq!(usage.capacity, Some(10_000));
                told.push(usage.tokens);
            }
            _ => {}
        }
    }
    assert_eq!(told, [1010, 2010], "Each step, as it goes");
    held.notified().await;
    let run = run.unwrap();
    let stop = Command::Interrupt { run: run.clone() };
    client
        .try_send(Request::new(RequestId::new(2), "stop", "room", stop))
        .unwrap();
    // The stop is taken: the answer it holds up may go.
    while !matches!(next(&client).await, Event::Reply { request, .. } if request == RequestId::new(2))
    {
    }
    open.send_replace(true);

    let events = collect_operation(&client).await;
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::RunFinished { outcome: RunOutcome::Interrupted, run: ended, .. } if *ended == run
        )),
        "{events:#?}"
    );
    let usage = events
        .iter()
        .rev()
        .find_map(|event| match event {
            Event::UsageUpdated { context, usage, .. } => Some((context.clone(), usage.clone())),
            _ => None,
        })
        .expect("the stop keeps what the steps told");
    assert_eq!(usage.0.map(|context| context.tokens), Some(2010));
    assert_eq!(usage.1.input_tokens, Some(3000));
    assert!(usage.1.complete, "The third generation was never asked");
    drop(client);
    task.await.unwrap().unwrap();
    roomkit::testing::idle(&kit).await;
}

/// A turn whose call `second` makes is stopped with no message of its own, after
/// one that had: what it is told of its usage, and what a later restore reads.
async fn stopped_with_no_message(second: ScriptedRound) -> (Option<u64>, Option<u64>) {
    let kit = RoomKit::new(Default::default());
    let provider = NamedProvider(ScriptedProvider::new([
        ScriptedRound::answering("Done.").with_usage(used(100)),
        second,
        ScriptedRound::answering("Never.").with_usage(used(6000)),
    ]));
    let channel = prepare(&kit, provider, Arc::default(), false).await;
    let (open, gate) = watch::channel(false);
    let held = Arc::new(Notify::new());
    let on_hold = held.clone();
    kit.hook(
        roomkit::HookTrigger::BeforeBroadcast,
        move |event: roomkit::model::RoomEvent, _| {
            let (mut gate, on_hold) = (gate.clone(), on_hold.clone());
            let call = event.r#type == roomkit::model::EventType::ToolCallStart;
            async move {
                if call {
                    on_hold.notify_one();
                    let _ = gate.wait_for(|open| *open).await;
                }
                Ok(roomkit::model::HookResult::allow(Default::default()))
            }
        },
        hook_options("hold-the-call"),
    )
    .unwrap();
    let mut config = config();
    config.model = Some(ModelInfo::new("local", "fixture"));
    config.context_capacity = Some(10_000);
    let restored_config = config.clone();
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel.clone(), config).run(host));
    next(&client).await;
    client.try_send(request("first")).unwrap();
    collect_operation(&client).await;

    let again = Command::Submit {
        agent: "agent".into(),
        text: "Again.".into(),
    };
    client
        .try_send(Request::new(RequestId::new(2), "second", "room", again))
        .unwrap();
    let run = loop {
        if let Event::RunStarted { run, .. } = next(&client).await {
            break run;
        }
    };
    held.notified().await;
    let stop = Command::Interrupt { run: run.clone() };
    client
        .try_send(Request::new(RequestId::new(3), "stop", "room", stop))
        .unwrap();
    while !matches!(next(&client).await, Event::Reply { request, .. } if request == RequestId::new(3))
    {
    }
    open.send_replace(true);

    let events = collect_operation(&client).await;
    let told = events.iter().rev().find_map(|event| match event {
        Event::UsageUpdated {
            run: measured,
            context,
            ..
        } if *measured == run => context.as_ref().map(|context| context.tokens),
        _ => None,
    });
    drop(client);
    task.await.unwrap().unwrap();
    roomkit::testing::idle(&kit).await;
    // A person's message after it says nothing of the measure: one no agent
    // answers, which would measure its own turn.
    let person = roomkit::model::TextContent::new("Back.");
    let unanswered = roomkit::SendEventOptions {
        visibility: roomkit::model::Visibility::Transport,
        ..Default::default()
    };
    let room = restored_config.room.clone();
    kit.send_event(&room, channel.id(), person, unanswered)
        .await
        .unwrap();
    roomkit::testing::idle(&kit).await;
    let restored = LocalSession::new(kit.clone(), channel, restored_config)
        .snapshot()
        .await
        .unwrap();
    (told, restored.context.map(|context| context.tokens))
}

/// RMK-1344: its record rides its last row, the call's end, so the turn is told
/// what it used, and a later restore reads that, not the turn's before it.
#[tokio::test]
async fn a_stopped_turn_with_no_message_tells_what_it_used() {
    let second = ScriptedRound::calling("", [("c1", "lookup_record", "{}")]);
    let measured = stopped_with_no_message(second.with_usage(used(5000))).await;
    assert_eq!(measured, (Some(5010), Some(5010)));
}

/// A turn with text before its call holds its record on that message: the rows
/// of the call after it, an agent's with no record, leave the measure.
#[tokio::test]
async fn an_agent_s_call_rows_with_no_record_leave_its_measure() {
    let second = ScriptedRound::calling("Checking.", [("c1", "lookup_record", "{}")]);
    let measured = stopped_with_no_message(second.with_usage(used(5000))).await;
    assert_eq!(measured, (Some(5010), Some(5010)));
}

/// RMK-1344: the record of a turn that told no usage says nothing of the turn
/// before it: no measure is told, and a later restore reads none as current.
#[tokio::test]
async fn a_stopped_turn_that_told_no_usage_is_not_told_the_last_one_s() {
    let second = ScriptedRound::calling("", [("c1", "lookup_record", "{}")]);
    assert_eq!(stopped_with_no_message(second).await, (None, None));
}

/// A view that may no longer read its room is told no measure of its record.
#[tokio::test]
async fn a_view_that_may_not_read_is_told_no_usage() {
    let kit = RoomKit::new(Default::default());
    let provider = NamedProvider(ScriptedProvider::new([
        ScriptedRound::calling("One.", [("c1", "lookup_record", "{}")]).with_usage(used(1000)),
        ScriptedRound::answering("Two.").with_usage(used(2000)),
    ]));
    let channel = prepare(&kit, provider, Arc::default(), false).await;
    let revoking = kit.clone();
    kit.hook(
        roomkit::HookTrigger::BeforeToolUse,
        move |_: roomkit::ToolCallEvent, _| {
            let kit = revoking.clone();
            async move {
                let room = RoomId::new("room").unwrap();
                let console = ChannelId::new("console").unwrap();
                kit.set_access(
                    &room,
                    &console,
                    roomkit::model::Access::WriteOnly,
                    Default::default(),
                )
                .await
                .unwrap();
                Ok(roomkit::model::HookResult::allow(Default::default()))
            }
        },
        hook_options("revoke-the-view"),
    )
    .unwrap();
    let mut config = config();
    config.model = Some(ModelInfo::new("local", "fixture"));
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config).run(host));
    next(&client).await;
    client.try_send(request("ask")).unwrap();
    let events = collect_operation(&client).await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::UsageUpdated { .. })),
        "{events:#?}"
    );
    drop(client);
    task.await.unwrap().unwrap();
}

/// The kit's close hands a reading target its stream's end: the run was cut.
#[tokio::test]
async fn a_run_the_kit_closes_ends_interrupted() {
    let kit = RoomKit::new(Default::default());
    let provider = ScriptedProvider::new([
        ScriptedRound::calling("One.", [("c1", "lookup_record", "{}")]),
        ScriptedRound::answering("Never."),
    ]);
    let channel = prepare(&kit, provider, Arc::default(), false).await;
    let (open, gate) = watch::channel(false);
    let held = Arc::new(Notify::new());
    hold_answers(&kit, gate, held.clone());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    next(&client).await;
    client.try_send(request("ask")).unwrap();
    held.notified().await;
    let closing = kit.clone();
    tokio::spawn(async move { closing.close().await });
    tokio::task::yield_now().await;
    open.send_replace(true);
    let outcome = loop {
        if let Event::RunFinished { outcome, .. } = next(&client).await {
            break outcome;
        }
    };
    assert_eq!(outcome, RunOutcome::Interrupted);
    drop(client);
    let _ = task.await;
}
