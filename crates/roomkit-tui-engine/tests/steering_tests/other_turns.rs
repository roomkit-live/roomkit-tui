//! A message for a running turn its sender did not open, such as a task's:
//! RoomKit keeps it for a turn of its own, and a view whose host says so
//! (`SessionConfig::can_steer_any_turn`) hands it to the running turn.

use super::*;
use roomkit::DeliverOptions;

/// A turn the application opens with its own instruction, as it opens a
/// task's: nobody in the room wrote the message it answers. Returns once the
/// turn's tool waits at `entered`, with the task delivering it.
async fn open_the_application_s_turn(
    kit: &RoomKit,
    entered: &Notify,
) -> tokio::task::JoinHandle<()> {
    let opening = kit.clone();
    let delivery = tokio::spawn(async move {
        let options = DeliverOptions {
            addressed_to: Some(vec![config().agent]),
            instruction: true,
            ..DeliverOptions::default()
        };
        opening
            .deliver(&config().room, "Look up record 42", options)
            .await
            .unwrap();
    });
    entered.notified().await;
    delivery
}

/// A host that admits every message and counts those it is told ended.
#[derive(Debug, Default)]
struct CountsEnded {
    ended: watch::Sender<usize>,
}

impl SessionLifecycle for CountsEnded {
    fn finished<'a>(
        &'a self,
        _: &'a SessionConfig,
        _: &'a OperationId,
        _: TurnOutcome,
    ) -> LifecycleFuture<'a, ()> {
        self.ended.send_modify(|ended| *ended += 1);
        Box::pin(async {})
    }
}

/// The rows of the room saying `text`.
async fn rows_saying(kit: &RoomKit, text: &str) -> Vec<RoomEvent> {
    let rows = kit
        .get_timeline(&config().room, GetTimelineOptions::default())
        .await
        .unwrap();
    rows.into_iter()
        .filter(|row| roomkit_core::rules::transcoding::text_of(&row.content) == text)
        .collect()
}

#[tokio::test]
async fn a_message_goes_to_a_turn_the_application_opened_where_the_host_says_so() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![
        ScriptedRound::calling("Checking", [("call1", "lookup_record", "{}")]),
        ScriptedRound::answering("Record 42 is in the second ledger"),
        ScriptedRound::answering("A turn too many"),
    ];
    let (channel, provider) = steering_room(&kit, rounds).await;
    let (open, gate) = watch::channel(false);
    let entered = Arc::new(Notify::new());
    hold_tools(&kit, gate, entered.clone());
    let lifecycle = Arc::new(AdmitsOnce::default());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let mut steering = config();
    steering.can_steer_any_turn = true;
    let session =
        LocalSession::new(kit.clone(), channel.clone(), steering).with_lifecycle(lifecycle.clone());
    let task = tokio::spawn(session.run(host));
    next(&client).await;
    let delivery = open_the_application_s_turn(&kit, &entered).await;
    client
        .try_send(submit(1, "one", "Use the second ledger"))
        .unwrap();
    let (mut turn, mut shown) = (None, None);
    let (message, steered_into) = loop {
        match next(&client).await {
            Event::RunStarted { run, .. } => turn = Some(run),
            Event::Reply { request, error } if request == RequestId::new(1) => {
                assert_eq!(error, None, "The message is accepted during the turn");
            }
            Event::Item {
                item: Item::Message(row),
                ..
            } => shown = Some(row.id),
            Event::MessageSteered { message, run, .. } => break (message, run),
            _ => {}
        }
    };
    assert_eq!(Some(steered_into), turn, "Handed to the running turn");
    assert_eq!(shown, Some(message), "Shown as the person's message");
    open.send_replace(true);
    delivery.await.unwrap();
    // A turn of its own would hold the room until it had asked the model.
    tokio::time::timeout(Duration::from_secs(5), channel.idle())
        .await
        .expect("Nothing more runs in the room");
    assert_eq!(provider.requests().len(), 2, "No turn of its own");
    assert!(asked_with(&provider, 1, "Use the second ledger"));
    // Stored once, as the person's message to the agent.
    let rows = rows_saying(&kit, "Use the second ledger").await;
    let [row] = rows.as_slice() else {
        panic!("Stored once: {rows:#?}");
    };
    let person = row.source.participant_id.as_ref().map(|id| id.as_str());
    assert_eq!(person, Some("alice"));
    assert_eq!(row.source.channel_id.as_str(), "console");
    assert_eq!(row.addressed_to, Some(vec![config().agent]));
    assert_eq!(row.status, roomkit::model::EventStatus::Delivered);
    assert_eq!(
        *lifecycle.finished.lock().unwrap(),
        [TurnOutcome::Completed],
        "Admitted, then told the host as delivered"
    );
    drop(client);
    task.await.unwrap().unwrap();
    channel_idle(&kit).await;
}

/// A client resending an operation replays it: the turn that took the
/// message is not handed it a second time.
#[tokio::test]
async fn an_operation_sent_again_is_not_handed_to_the_turn_twice() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![
        ScriptedRound::calling("Checking", [("call1", "lookup_record", "{}")]),
        ScriptedRound::answering("Record 42 is in the second ledger"),
        ScriptedRound::answering("A turn too many"),
    ];
    let (channel, provider) = steering_room(&kit, rounds).await;
    let (open, gate) = watch::channel(false);
    let entered = Arc::new(Notify::new());
    hold_tools(&kit, gate, entered.clone());
    let lifecycle = Arc::new(CountsEnded::default());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let mut steering = config();
    steering.can_steer_any_turn = true;
    let session =
        LocalSession::new(kit.clone(), channel.clone(), steering).with_lifecycle(lifecycle.clone());
    let task = tokio::spawn(session.run(host));
    next(&client).await;
    let delivery = open_the_application_s_turn(&kit, &entered).await;
    for request in [1, 2] {
        client
            .try_send(submit(request, "one", "Use the second ledger"))
            .unwrap();
        assert_eq!(reply(&client, request).await, None);
    }
    // Both deliveries are over before the turn reads on.
    let mut ended = lifecycle.ended.subscribe();
    tokio::time::timeout(Duration::from_secs(5), ended.wait_for(|ended| *ended == 2))
        .await
        .expect("Both sends end")
        .unwrap();
    open.send_replace(true);
    delivery.await.unwrap();
    // A turn of its own would hold the room until it had asked the model.
    tokio::time::timeout(Duration::from_secs(5), channel.idle())
        .await
        .expect("Nothing more runs in the room");
    assert_eq!(provider.requests().len(), 2, "No turn of its own");
    let read = provider.requests()[1]
        .messages
        .iter()
        .filter(|message| message.text().contains("Use the second ledger"))
        .count();
    assert_eq!(read, 1, "Read once by the turn");
    assert_eq!(rows_saying(&kit, "Use the second ledger").await.len(), 1);
    drop(client);
    task.await.unwrap().unwrap();
    channel_idle(&kit).await;
}

/// RoomKit's rule holds for a view whose host says nothing: only the author
/// of the message a turn answers writes to it.
#[tokio::test]
async fn a_message_waits_a_turn_the_application_opened_by_default() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![
        ScriptedRound::calling("Checking", [("call1", "lookup_record", "{}")]),
        ScriptedRound::answering("Record 42 is in the first ledger"),
        ScriptedRound::answering("Now from the second ledger"),
    ];
    let (channel, provider) = steering_room(&kit, rounds).await;
    let (open, gate) = watch::channel(false);
    let entered = Arc::new(Notify::new());
    hold_tools(&kit, gate, entered.clone());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    next(&client).await;
    let delivery = open_the_application_s_turn(&kit, &entered).await;
    client
        .try_send(submit(1, "one", "Use the second ledger"))
        .unwrap();
    let mut steered = None;
    let own = loop {
        match next(&client).await {
            Event::Reply { request, error } if request == RequestId::new(1) => {
                assert_eq!(error, None, "The message is accepted during the turn");
                open.send_replace(true);
            }
            Event::MessageSteered { message, .. } => steered = Some(message),
            Event::RunStarted {
                run,
                answers: Some(message),
                ..
            } if Some(&message) == steered.as_ref() => break run,
            _ => {}
        }
    };
    loop {
        if let Event::RunFinished { run, .. } = next(&client).await
            && run == own
        {
            break;
        }
    }
    delivery.await.unwrap();
    assert_eq!(provider.requests().len(), 3, "A turn of its own");
    assert!(!asked_with(&provider, 1, "Use the second ledger"));
    assert!(asked_with(&provider, 2, "Use the second ledger"));
    drop(client);
    task.await.unwrap().unwrap();
    channel_idle(&kit).await;
}
