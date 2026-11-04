//! RMK-1371: a message is committed, and told steered, before its delivery
//! reaches the running turn. A turn cut in between never takes it, and RoomKit
//! gives it a turn of its own, which `RunStarted` names.

use super::lifecycle::run_finished;
use super::*;
use roomkit::model::{ChannelBinding, ChannelOutput, ChannelType, InboundMessage, RoomContext};
use roomkit::{Channel, ChannelError, default_inbound_event};
use std::time::Instant;

/// A transport that holds the delivery of the agent's first row until `gate`
/// opens, telling `held` once it waits. A set keeps the room's delivery lane
/// until its targets answered, so the sets of later events wait behind it.
struct HoldsTheLane {
    gate: watch::Receiver<bool>,
    held: Arc<Notify>,
    rows: AtomicUsize,
}

impl Channel for HoldsTheLane {
    fn channel_id(&self) -> ChannelId {
        ChannelId::new("slow").unwrap()
    }

    fn channel_type(&self) -> ChannelType {
        ChannelType::WEBSOCKET
    }

    async fn handle_inbound(
        &self,
        message: InboundMessage,
        context: RoomContext,
    ) -> Result<RoomEvent, ChannelError> {
        Ok(default_inbound_event(self, &message, &context))
    }

    async fn deliver(
        &self,
        event: RoomEvent,
        _binding: ChannelBinding,
        _context: RoomContext,
    ) -> Result<ChannelOutput, ChannelError> {
        let agent = event.source.channel_id == config().agent;
        if agent && self.rows.fetch_add(1, Ordering::SeqCst) == 0 {
            self.held.notify_one();
            let mut gate = self.gate.clone();
            let _ = gate.wait_for(|open| *open).await;
        }
        Ok(ChannelOutput::empty())
    }
}

/// What a client that keeps asking for an answer gets: the turn of its own
/// answers the message, the host refuses `Answer` while it runs, then takes it
/// as any turn, and the model answers again. No client should: the message's
/// `RunStarted` told it the message has its turn (`Command::Answer`), and the
/// terminal stops asking there (`scripts/check-steering.py`).
#[tokio::test]
async fn a_message_delivered_after_the_cut_gets_a_turn_of_its_own() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![
        ScriptedRound::calling("Checking", [("call1", "lookup_record", "{}")]),
        ScriptedRound::answering("Its own answer"),
        ScriptedRound::answering("A second answer"),
    ];
    let (channel, provider) = steering_room(&kit, rounds).await;
    let (open, gate) = watch::channel(false);
    let held = Arc::new(Notify::new());
    let slow = HoldsTheLane {
        gate,
        held: held.clone(),
        rows: AtomicUsize::new(0),
    };
    kit.register_channel(slow).unwrap();
    let slow = ChannelId::new("slow").unwrap();
    kit.attach_channel(&config().room, &slow, Default::default())
        .await
        .unwrap();
    // The turn waits at its call, as it waits for a person's approval.
    let (_tools, waiting) = watch::channel(false);
    hold_tools(&kit, waiting, Arc::new(Notify::new()));
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    next(&client).await;

    client
        .try_send(submit(1, "one", "Look up record 42"))
        .unwrap();
    held.notified().await;
    client
        .try_send(submit(2, "two", "Use the second ledger"))
        .unwrap();
    let (message, run) = loop {
        if let Event::MessageSteered { message, run, .. } = next(&client).await {
            break (message, run);
        }
    };
    let cut = Command::Interrupt { run: run.clone() };
    client
        .try_send(Request::new(RequestId::new(3), "cut", "room", cut))
        .unwrap();
    assert_eq!(run_finished(&client, &run).await, RunOutcome::Interrupted);

    // Nothing runs, and the host still holds the message.
    client.try_send(answer(4, "early", "agent")).unwrap();
    let refused = reply(&client, 4).await.map(|rejection| rejection.code);
    assert_eq!(refused, Some(ErrorCode::Busy));

    open.send_replace(true);
    let own = loop {
        if let Event::RunStarted {
            run,
            answers: Some(answered),
            ..
        } = next(&client).await
        {
            assert_eq!(answered, message, "It answers the steered message");
            break run;
        }
    };
    assert_eq!(run_finished(&client, &own).await, RunOutcome::Completed);
    assert_eq!(provider.requests().len(), 2, "One request for each turn");
    assert!(asked_with(&provider, 1, "Use the second ledger"));

    // The room comes free with no event to say so.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut id = 5;
    while reply_to_answer(&client, id).await.is_some() {
        assert!(Instant::now() < deadline, "The host stayed busy");
        tokio::time::sleep(Duration::from_millis(1)).await;
        id += 1;
    }
    let answering = format!("again-{id}");
    while !matches!(next(&client).await, Event::OperationFinished { operation, .. } if operation.as_str() == answering)
    {
    }
    assert_eq!(provider.requests().len(), 3, "The model answered again");
    drop(client);
    task.await.unwrap().unwrap();
    channel_idle(&kit).await;
}

async fn reply_to_answer(client: &Client, id: u64) -> Option<Rejection> {
    let operation = format!("again-{id}");
    client.try_send(answer(id, &operation, "agent")).unwrap();
    reply(client, id).await
}
