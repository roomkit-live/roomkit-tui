//! RMK-1232: the host hears of a steered message's turn as of any turn. A
//! message handed to the running turn starts nothing; one that gets a turn
//! of its own is `started` before that turn asks the model.

use super::*;
use roomkit_core::rules::transcoding::text_of;

/// What the host was told, for one operation.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Call {
    Admit,
    /// With how many requests the model had received by then.
    Started(usize),
    Finished(TurnOutcome),
}

/// A host that records its calls per operation; the `started` of `held`
/// never returns.
#[derive(Debug)]
struct Recording {
    provider: ScriptedProvider,
    calls: Mutex<Vec<(String, Call)>>,
    held: Option<&'static str>,
    entered: Notify,
}

impl Recording {
    fn new(provider: ScriptedProvider, held: Option<&'static str>) -> Arc<Self> {
        Arc::new(Self {
            provider,
            calls: Mutex::default(),
            held,
            entered: Notify::new(),
        })
    }

    fn record(&self, operation: &OperationId, call: Call) {
        let operation = operation.as_str().to_owned();
        self.calls.lock().unwrap().push((operation, call));
    }

    fn calls(&self, operation: &str) -> Vec<Call> {
        let calls = self.calls.lock().unwrap();
        let of = calls.iter().filter(|(id, _)| id == operation);
        of.map(|(_, call)| call.clone()).collect()
    }

    /// The calls of `operation`, once the host was told how it ended.
    async fn finished(&self, operation: &str) -> Vec<Call> {
        self.finished_times(operation, 1).await
    }

    /// The calls of `operation`, once the host was told `times` ends.
    async fn finished_times(&self, operation: &str, times: usize) -> Vec<Call> {
        let ends = |calls: Vec<Call>| {
            calls
                .iter()
                .filter(|c| matches!(c, Call::Finished(_)))
                .count()
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while ends(self.calls(operation)) < times {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("The host is told how the message ended");
        self.calls(operation)
    }
}

impl SessionLifecycle for Recording {
    fn admit<'a>(
        &'a self,
        _: &'a SessionConfig,
        operation: &'a OperationId,
        _: &'a roomkit::model::EventContent,
    ) -> LifecycleFuture<'a, Result<(), Rejection>> {
        self.record(operation, Call::Admit);
        Box::pin(async { Ok(()) })
    }

    fn started<'a>(
        &'a self,
        _: &'a SessionConfig,
        operation: &'a OperationId,
    ) -> LifecycleFuture<'a, ()> {
        let asked = self.provider.requests().len();
        self.record(operation, Call::Started(asked));
        Box::pin(async move {
            if self.held == Some(operation.as_str()) {
                self.entered.notify_one();
                std::future::pending::<()>().await;
            }
        })
    }

    fn finished<'a>(
        &'a self,
        _: &'a SessionConfig,
        operation: &'a OperationId,
        outcome: TurnOutcome,
    ) -> LifecycleFuture<'a, ()> {
        self.record(operation, Call::Finished(outcome));
        Box::pin(async {})
    }
}

/// The run that ends as `run`, and how.
async fn run_finished(client: &Client, run: &RunId) -> RunOutcome {
    loop {
        if let Event::RunFinished {
            run: ended,
            outcome,
            ..
        } = next(client).await
            && &ended == run
        {
            return outcome;
        }
    }
}

/// Failed before RMK-1232: the host never heard the turn start, so the
/// hooks a host runs as a turn starts never ran for it.
#[tokio::test]
async fn a_steered_message_s_own_turn_is_started_before_the_model_is_asked() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![
        ScriptedRound::answering("First answer"),
        ScriptedRound::answering("Second answer"),
    ];
    let (channel, provider) = steering_room(&kit, rounds).await;
    let (open, gate) = watch::channel(false);
    let held = Arc::new(Notify::new());
    hold_first_answer(&kit, gate, held.clone());
    let lifecycle = Recording::new(provider.clone(), None);
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let session =
        LocalSession::new(kit.clone(), channel, config()).with_lifecycle(lifecycle.clone());
    let task = tokio::spawn(session.run(host));
    next(&client).await;
    let own = steer_into_its_own_turn(&client, &open, &held).await;
    assert_eq!(run_finished(&client, &own).await, RunOutcome::Completed);
    let calls = lifecycle.finished("two").await;
    let first_turn_only = 1;
    let expected = [
        Call::Admit,
        Call::Started(first_turn_only),
        Call::Finished(TurnOutcome::Completed),
    ];
    assert_eq!(calls, expected, "Started before its turn asks the model");
    assert_eq!(provider.requests().len(), 2);
    drop(client);
    task.await.unwrap().unwrap();
    channel_idle(&kit).await;
}

/// A message the running turn reads starts no turn: the host is not told
/// one started, before or after RMK-1232.
#[tokio::test]
async fn a_message_handed_to_the_running_turn_starts_nothing() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![
        ScriptedRound::calling("Checking", [("call1", "lookup_record", "{}")]),
        ScriptedRound::answering("Record 42 is in the second ledger"),
    ];
    let (channel, provider) = steering_room(&kit, rounds).await;
    let (open, gate) = watch::channel(false);
    let entered = Arc::new(Notify::new());
    hold_tools(&kit, gate, entered.clone());
    let lifecycle = Recording::new(provider.clone(), None);
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let session =
        LocalSession::new(kit.clone(), channel, config()).with_lifecycle(lifecycle.clone());
    let task = tokio::spawn(session.run(host));
    next(&client).await;
    client
        .try_send(submit(1, "one", "Look up record 42"))
        .unwrap();
    entered.notified().await;
    client
        .try_send(submit(2, "two", "Use the second ledger"))
        .unwrap();
    while !matches!(next(&client).await, Event::MessageSteered { .. }) {}
    open.send_replace(true);
    collect_operation(&client).await;
    let calls = lifecycle.finished("two").await;
    assert_eq!(calls, [Call::Admit, Call::Finished(TurnOutcome::Completed)]);
    assert!(asked_with(&provider, 1, "Use the second ledger"));
    assert_eq!(provider.requests().len(), 2, "No turn of its own");
    drop(client);
    task.await.unwrap().unwrap();
    channel_idle(&kit).await;
}

/// Esc while the host starts the message's own turn: the host hears the
/// turn start, then end interrupted, and nothing of the turn is said. The
/// review of 2b03f64: Esc waited for the host's start, which a host that
/// never returns held for good; it drops it, as it drops a preparation.
#[tokio::test]
async fn stopping_a_steered_message_s_own_turn_as_it_starts() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![
        ScriptedRound::answering("First answer"),
        ScriptedRound::answering("Never said"),
    ];
    let (channel, provider) = steering_room(&kit, rounds).await;
    let (open, gate) = watch::channel(false);
    let held = Arc::new(Notify::new());
    hold_first_answer(&kit, gate, held.clone());
    let lifecycle = Recording::new(provider.clone(), Some("two"));
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let session =
        LocalSession::new(kit.clone(), channel, config()).with_lifecycle(lifecycle.clone());
    let task = tokio::spawn(session.run(host));
    next(&client).await;
    let own = steer_into_its_own_turn(&client, &open, &held).await;
    let starting = lifecycle.entered.notified();
    let starting = tokio::time::timeout(Duration::from_secs(5), starting).await;
    starting.expect("The host hears its own turn start");
    let interrupt = Command::Interrupt { run: own.clone() };
    client
        .try_send(Request::new(RequestId::new(3), "stop", "room", interrupt))
        .unwrap();
    assert_eq!(reply(&client, 3).await, None, "The person stops it");
    assert_eq!(run_finished(&client, &own).await, RunOutcome::Interrupted);
    let calls = lifecycle.finished("two").await;
    let expected = [
        Call::Admit,
        Call::Started(1),
        Call::Finished(TurnOutcome::Interrupted),
    ];
    assert_eq!(calls, expected);
    let rows = kit
        .get_timeline(&config().room, GetTimelineOptions::default())
        .await
        .unwrap();
    let said = |row: &RoomEvent| text_of(&row.content).contains("Never said");
    assert!(
        !rows.iter().any(said),
        "The stopped turn's answer is dropped"
    );
    drop(client);
    task.await.unwrap().unwrap();
    channel_idle(&kit).await;
    // The light review of 0ab4ad3: the stream, woken as the stop was
    // registered, read on before the engine applied it. On this
    // single-threaded runtime the cancel now lands first.
    assert_eq!(provider.requests().len(), 1, "The stopped turn never asked");
}

/// The review of 2b03f64: a client resending the message with the same
/// operation, as a transport retry does, erased what the first one expected,
/// and its own turn ran without the host hearing it start.
#[tokio::test]
async fn a_resent_message_s_own_turn_is_still_started() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![
        ScriptedRound::answering("First answer"),
        ScriptedRound::answering("Second answer"),
    ];
    let (channel, provider) = steering_room(&kit, rounds).await;
    let (open, gate) = watch::channel(false);
    let held = Arc::new(Notify::new());
    hold_first_answer(&kit, gate, held.clone());
    let lifecycle = Recording::new(provider.clone(), None);
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let session =
        LocalSession::new(kit.clone(), channel, config()).with_lifecycle(lifecycle.clone());
    let task = tokio::spawn(session.run(host));
    next(&client).await;
    client.try_send(submit(1, "one", "First question")).unwrap();
    held.notified().await;
    for id in [2, 3] {
        client
            .try_send(submit(id, "two", "Second question"))
            .unwrap();
        assert_eq!(reply(&client, id).await, None, "Accepted for the turn");
    }
    open.send_replace(true);
    let mut steered = None;
    let own = loop {
        match next(&client).await {
            Event::MessageSteered { message, .. } => steered = Some(message),
            Event::RunStarted {
                run,
                answers: Some(message),
                ..
            } if Some(&message) == steered.as_ref() => break run,
            _ => {}
        }
    };
    assert_eq!(run_finished(&client, &own).await, RunOutcome::Completed);
    let calls = lifecycle.finished_times("two", 2).await;
    let starts = calls
        .iter()
        .filter(|c| matches!(c, Call::Started(_)))
        .count();
    assert_eq!(starts, 1, "Its own turn is started once: {calls:?}");
    assert_eq!(provider.requests().len(), 2, "One turn of its own");
    drop(client);
    task.await.unwrap().unwrap();
    channel_idle(&kit).await;
}

/// A host that refuses every turn, and counts the admissions it was asked for.
#[derive(Debug, Default)]
struct Closed {
    asked: Mutex<Vec<String>>,
}

impl SessionLifecycle for Closed {
    fn admit<'a>(
        &'a self,
        _: &'a SessionConfig,
        operation: &'a OperationId,
        _: &'a roomkit::model::EventContent,
    ) -> LifecycleFuture<'a, Result<(), Rejection>> {
        self.asked
            .lock()
            .unwrap()
            .push(operation.as_str().to_owned());
        Box::pin(async { Err(Rejection::refused("Application is closing")) })
    }
}

/// RMK-1311, security review: a turn with no new message still passes the
/// host's gate. Nothing reaches the model when the host refuses.
#[tokio::test]
async fn an_answer_passes_the_host_s_gate() {
    let kit = RoomKit::new(Default::default());
    let (channel, provider) = steering_room(&kit, vec![ScriptedRound::answering("Done")]).await;
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let lifecycle = Arc::new(Closed::default());
    let session =
        LocalSession::new(kit.clone(), channel, config()).with_lifecycle(lifecycle.clone());
    let task = tokio::spawn(session.run(host));
    next(&client).await;

    client
        .try_send(answer(1, "one", config().agent.as_str()))
        .unwrap();
    collect_operation(&client).await;
    assert_eq!(*lifecycle.asked.lock().unwrap(), ["one"]);
    assert!(provider.requests().is_empty(), "The refused turn ran");
    drop(client);
    task.await.unwrap().unwrap();
    channel_idle(&kit).await;
}
