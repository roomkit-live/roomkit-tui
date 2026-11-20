//! Approvals asked by application work, outside a model turn.

use super::*;
use roomkit_tui_engine::{CommandExecution, CommandHandler, CommandResult};

/// Asks before deploying, from host work, through its context. `wait` shows
/// the answer it got; `twice` and `thrice` ask again and show every answer,
/// ignoring interruption; `leave` asks from a detached task and ends without
/// waiting for it.
struct Deployer;

impl CommandHandler for Deployer {
    async fn handle(&self, request: Request, _: SessionConfig) -> Result<CommandResult, Rejection> {
        let Command::Invoke { arguments, .. } = request.command else {
            return Err("Unsupported".into());
        };
        let work = CommandExecution::new("deploy-1".into(), move |context| async move {
            let wait = Duration::from_secs(if arguments == "thrice" { 5 } else { 2 });
            let request = PermissionRequest::new("Deploy?", "Deploy to production", wait);
            if arguments == "leave" {
                tokio::spawn(async move { context.ask_permission(request).await });
                tokio::time::sleep(Duration::from_millis(100)).await;
                return Ok(RunOutcome::Completed);
            }
            let times = match arguments.as_str() {
                "twice" => 2,
                "thrice" => 3,
                _ => 1,
            };
            let mut answers = Vec::new();
            for _ in 0..times {
                let decision = context.ask_permission(request.clone()).await;
                answers.push(format!("{decision:?}"));
            }
            let answer = answers.join("/");
            context.publish(Item::Message(Message::new(
                "answer",
                "person:alice",
                answer,
            )));
            Ok(RunOutcome::Completed)
        });
        Ok(CommandResult::Execute(work))
    }
}

/// A view of the room for `sender`, its client and its serving task.
async fn attach<S: ConversationStore>(
    kit: &RoomKit<S>,
    channel: &ConversationChannel,
    sender: &str,
) -> (Client, tokio::task::JoinHandle<Result<(), AdapterError>>) {
    let mut view = config();
    view.sender = sender.into();
    view.commands.push(CommandSpec::new("deploy", "Deploy"));
    let (client, host) = connection(NonZeroUsize::new(64).unwrap());
    let session = LocalSession::new(kit.clone(), channel.clone(), view);
    let task = tokio::spawn(session.run_with_commands(host, Deployer));
    assert!(matches!(next(&client).await, Event::Snapshot(_)));
    (client, task)
}

fn deploy(client: &Client, mode: &str) {
    let command = Command::Invoke {
        name: "deploy".into(),
        arguments: mode.into(),
    };
    let request = Request::new(RequestId::new(1), "deploy", "room", command);
    client.try_send(request).unwrap();
}

fn decide(id: u64, permission: &Permission, decision: Decision) -> Request {
    let command = Command::Decide {
        permission: permission.id.clone(),
        decision,
    };
    Request::new(RequestId::new(id), format!("decide-{id}"), "room", command)
}

async fn requested(client: &Client) -> Permission {
    loop {
        match next(client).await {
            Event::PermissionRequested { permission, .. } => return permission,
            Event::RunFinished { .. } => panic!("The work ended without asking"),
            _ => {}
        }
    }
}

async fn reply(client: &Client, id: u64) -> Option<Rejection> {
    loop {
        if let Event::Reply { request, error } = next(client).await
            && request == RequestId::new(id)
        {
            return error;
        }
    }
}

async fn answer(client: &Client) -> String {
    loop {
        if let Event::Item {
            item: Item::Message(message),
            ..
        } = next(client).await
        {
            return message.text;
        }
    }
}

/// RMK-1107: the broker knew only model turns, so host work waited for the
/// whole timeout and was denied without the person ever being asked; its
/// decisions must also be stored once it settles, as a turn's are.
#[tokio::test]
async fn host_work_asks_the_person_who_invoked_it() {
    let kit = RoomKit::new(Default::default());
    let channel = prepare(&kit, ScriptedProvider::new([]), Arc::default(), false).await;
    let (alice, alice_task) = attach(&kit, &channel, "alice").await;
    let (bob, bob_task) = attach(&kit, &channel, "bob").await;
    deploy(&alice, "wait");
    let permission = requested(&alice).await;
    assert_eq!(permission.run.as_str(), "deploy-1");
    let permission_seen_by_bob = requested(&bob).await;
    bob.try_send(decide(2, &permission_seen_by_bob, Decision::Allow))
        .unwrap();
    let refused = reply(&bob, 2).await.map(|rejection| rejection.code);
    assert_eq!(refused, Some(ErrorCode::Refused), "Only Alice may answer");
    alice
        .try_send(decide(2, &permission, Decision::Allow))
        .unwrap();
    assert_eq!(answer(&alice).await, "Allow");
    while !matches!(next(&alice).await, Event::OperationFinished { .. }) {}
    let rows = kit
        .get_timeline(&config().room, GetTimelineOptions::default())
        .await
        .unwrap();
    let stored = rows
        .iter()
        .any(|row| row.metadata.contains_key("roomkit_tui_approval"));
    assert!(stored, "The decision is stored when the work settles");
    drop((alice, bob));
    alice_task.await.unwrap().unwrap();
    bob_task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

/// RMK-1107 review: an interrupted run's approvals stayed answerable, those
/// pending and those it asked afterwards, and a late Allow reached work the
/// person had stopped.
#[tokio::test]
async fn interrupted_host_work_can_no_longer_be_approved() {
    let kit = RoomKit::new(Default::default());
    let channel = prepare(&kit, ScriptedProvider::new([]), Arc::default(), false).await;
    let (alice, task) = attach(&kit, &channel, "alice").await;
    deploy(&alice, "twice");
    let permission = requested(&alice).await;
    let interrupt = Command::Interrupt {
        run: "deploy-1".into(),
    };
    alice
        .try_send(Request::new(RequestId::new(2), "stop", "room", interrupt))
        .unwrap();
    assert!(reply(&alice, 2).await.is_none());
    alice
        .try_send(decide(3, &permission, Decision::Allow))
        .unwrap();
    let (mut late, mut answer, mut asked_again, mut settled) = (None, None, false, false);
    while late.is_none() || !settled {
        match next(&alice).await {
            Event::Reply { request, error } if request == RequestId::new(3) => late = Some(error),
            Event::PermissionRequested { .. } => asked_again = true,
            Event::Item {
                item: Item::Message(message),
                ..
            } => answer = Some(message.text),
            Event::OperationFinished { .. } => settled = true,
            _ => {}
        }
    }
    assert!(late.flatten().is_some(), "No longer answerable");
    assert!(
        !asked_again,
        "A request asked after the interruption is not shown"
    );
    assert_eq!(answer.as_deref(), Some("Deny/Deny"));
    drop(alice);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

/// Work that ends with a request unanswered withdraws it before the view
/// learns the run has finished.
#[tokio::test]
async fn an_unanswered_approval_is_withdrawn_before_the_run_finishes() {
    let kit = RoomKit::new(Default::default());
    let channel = prepare(&kit, ScriptedProvider::new([]), Arc::default(), false).await;
    let (alice, task) = attach(&kit, &channel, "alice").await;
    deploy(&alice, "leave");
    let permission = requested(&alice).await;
    let withdrawn = loop {
        match next(&alice).await {
            Event::PermissionResolved { permission: id, .. } => break id,
            Event::RunFinished { .. } => panic!("The run finished with a request still pending"),
            _ => {}
        }
    };
    assert_eq!(withdrawn, permission.id);
    drop(alice);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

/// RMK-1111: a turn delivered in the room for another channel's message
/// replaced the work's run. Meanwhile its requests went to that turn, with no
/// requester, so Bob could approve Alice's deploy; afterwards they found no
/// run and were denied at their timeout, unshown.
#[tokio::test]
async fn host_work_keeps_its_approvals_while_another_channel_turn_runs() {
    use roomkit::ProcessInboundOptions;
    use roomkit::model::{EventContent, InboundMessage, TextContent};
    use tokio::sync::watch;

    let kit = RoomKit::new(Default::default());
    let provider = ScriptedProvider::new([
        ScriptedRound::calling(
            "Checking",
            [(
                "call1",
                "lookup_record",
                r#"{"_roomkit_intent":"Checking the record"}"#,
            )],
        ),
        ScriptedRound::answering("Done"),
    ]);
    let channel = prepare(&kit, provider, Arc::default(), false).await;
    let sms = ConversationChannel::buffered(ChannelId::new("sms").unwrap());
    kit.register_channel(sms.clone()).unwrap();
    let room = config().room;
    kit.attach_channel(&room, sms.id(), Default::default())
        .await
        .unwrap();
    let (open, gate) = watch::channel(false);
    kit.hook(
        roomkit::HookTrigger::BeforeToolUse,
        move |_: roomkit::ToolCallEvent, _| {
            let mut gate = gate.clone();
            async move {
                let _ = gate.wait_for(|open| *open).await;
                Ok(roomkit::model::HookResult::allow(Default::default()))
            }
        },
        hook_options("hold"),
    )
    .unwrap();
    let (alice, alice_task) = attach(&kit, &channel, "alice").await;
    let (bob, bob_task) = attach(&kit, &channel, "bob").await;
    deploy(&alice, "thrice");
    let first = requested(&alice).await;

    let text = EventContent::Text(TextContent::new("Status?"));
    let mut message = InboundMessage::new(sms.id().clone(), "carol", text);
    message.addressed_to = Some(vec![config().agent]);
    let options = ProcessInboundOptions {
        room_id: Some(room),
        organization_id: None,
        defer_delivery: false,
    };
    let inbound = {
        let kit = kit.clone();
        tokio::spawn(async move { kit.process_inbound(message, options).await.map(|_| ()) })
    };
    let turn = loop {
        if let Event::RunStarted { run, .. } = next(&alice).await {
            break run;
        }
    };
    alice.try_send(decide(2, &first, Decision::Allow)).unwrap();
    let during = requested(&alice).await;
    assert_eq!(during.run.as_str(), "deploy-1", "Not the turn {turn:?}");
    bob.try_send(decide(3, &during, Decision::Allow)).unwrap();
    let refused = reply(&bob, 3).await.map(|rejection| rejection.code);
    assert_eq!(refused, Some(ErrorCode::Refused), "Only Alice may answer");

    // Alice answers once the turn has ended, so the work asks again after it.
    open.send(true).unwrap();
    while !matches!(next(&alice).await, Event::RunFinished { run, .. } if run == turn) {}
    alice.try_send(decide(4, &during, Decision::Allow)).unwrap();
    let after = requested(&alice).await;
    assert_eq!(after.run.as_str(), "deploy-1");
    alice.try_send(decide(5, &after, Decision::Allow)).unwrap();
    assert_eq!(answer(&alice).await, "Allow/Allow/Allow");
    inbound.await.unwrap().unwrap();
    drop((alice, bob));
    alice_task.await.unwrap().unwrap();
    bob_task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

/// A room where a message of another channel, `sms`, starts a turn of the
/// view's agent that asks before its tool: the kit, the view's channel and
/// `sms`.
async fn room_with_another_channel() -> (RoomKit, ConversationChannel, ConversationChannel) {
    let kit = RoomKit::new(Default::default());
    let provider = ScriptedProvider::new([
        ScriptedRound::answering("Hello"),
        ScriptedRound::calling("Checking", [("call1", "lookup_record", "{}")]),
        ScriptedRound::answering("Done"),
    ]);
    let channel = prepare(&kit, provider, Arc::default(), false).await;
    let sms = ConversationChannel::buffered(ChannelId::new("sms").unwrap());
    kit.register_channel(sms.clone()).unwrap();
    kit.attach_channel(&config().room, sms.id(), Default::default())
        .await
        .unwrap();
    let asking = channel.clone();
    kit.hook(
        roomkit::HookTrigger::BeforeToolUse,
        move |event: roomkit::ToolCallEvent, _| {
            let asking = asking.clone();
            async move {
                let request =
                    PermissionRequest::new("Read?", "Read a record", Duration::from_secs(30));
                let decision = asking.ask_tool_permission(&event, request).await;
                Ok(match decision {
                    Decision::Allow => roomkit::model::HookResult::allow(Default::default()),
                    _ => roomkit::model::HookResult::block("Denied", Default::default()),
                })
            }
        },
        hook_options("permission"),
    )
    .unwrap();
    (kit, channel, sms)
}

/// `sms` sends a message to the view's agent, from a task of its own.
fn message_from(
    kit: &RoomKit,
    sms: &ConversationChannel,
) -> tokio::task::JoinHandle<Result<(), roomkit::RoomKitError>> {
    use roomkit::ProcessInboundOptions;
    use roomkit::model::{EventContent, InboundMessage, TextContent};
    let text = EventContent::Text(TextContent::new("Status?"));
    let mut message = InboundMessage::new(sms.id().clone(), "carol", text);
    message.addressed_to = Some(vec![config().agent]);
    let options = ProcessInboundOptions {
        room_id: Some(config().room),
        organization_id: None,
        defer_delivery: false,
    };
    let kit = kit.clone();
    tokio::spawn(async move { kit.process_inbound(message, options).await.map(|_| ()) })
}

/// Whether a decision row is stored in the room within two seconds.
async fn decision_stored(kit: &RoomKit) -> bool {
    let room = config().room;
    let stored = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let rows = kit
                .get_timeline(&room, GetTimelineOptions::default())
                .await
                .unwrap();
            if rows
                .iter()
                .any(|row| row.metadata.contains_key("roomkit_tui_approval"))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    stored.await.is_ok()
}

/// A decision for a run the view did not start, here a turn for another
/// channel's message, is stored at once, also once the view has run a turn:
/// no later turn of the view is waited for.
#[tokio::test]
async fn a_decision_for_another_channel_s_turn_is_stored_after_a_turn_of_the_view() {
    let (kit, channel, sms) = room_with_another_channel().await;
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel.clone(), config()).run(host));
    next(&client).await;
    client.try_send(request("greeting")).unwrap();
    collect_operation(&client).await;
    let inbound = message_from(&kit, &sms);
    let permission = requested(&client).await;
    client
        .try_send(decide(2, &permission, Decision::Allow))
        .unwrap();
    assert_eq!(reply(&client, 2).await, None);
    inbound.await.unwrap().unwrap();
    assert!(decision_stored(&kit).await, "The decision is stored");
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

/// Holds the end of the view's turn, as a host's slow `finished` does,
/// telling `entered` once it holds it.
#[derive(Debug, Default)]
struct HoldFinished {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl roomkit_tui_engine::SessionLifecycle for HoldFinished {
    fn finished<'a>(
        &'a self,
        _: &'a SessionConfig,
        _: &'a OperationId,
        _: roomkit_tui_engine::TurnOutcome,
    ) -> roomkit_tui_engine::LifecycleFuture<'a, ()> {
        Box::pin(async move {
            self.entered.notify_one();
            self.release.notified().await;
        })
    }
}

/// A decision taken while the view's turn ends, past the store of its own
/// decisions, is stored: the ended turn takes no interruption any more, so
/// the decision is not left to it.
#[tokio::test]
async fn a_decision_taken_while_the_view_s_turn_ends_is_stored() {
    let (kit, channel, sms) = room_with_another_channel().await;
    let hold = Arc::new(HoldFinished::default());
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let session =
        LocalSession::new(kit.clone(), channel.clone(), config()).with_lifecycle(hold.clone());
    let task = tokio::spawn(session.run(host));
    next(&client).await;
    client.try_send(request("greeting")).unwrap();
    hold.entered.notified().await;
    let inbound = message_from(&kit, &sms);
    // The view's own run ends meanwhile: `requested` would take it for the end.
    let permission = loop {
        if let Event::PermissionRequested { permission, .. } = next(&client).await {
            break permission;
        }
    };
    client
        .try_send(decide(2, &permission, Decision::Allow))
        .unwrap();
    assert_eq!(reply(&client, 2).await, None);
    let stored = decision_stored(&kit).await;
    hold.release.notify_one();
    let _ = tokio::time::timeout(Duration::from_secs(5), inbound).await;
    assert!(stored, "The decision is stored while the turn ends");
    drop(client);
    let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
}

/// Decisions are stored one room at a time per room: a room whose lock a
/// turn holds keeps its own decisions waiting, never another room's, and a
/// view attaches elsewhere meanwhile.
#[tokio::test]
async fn a_room_whose_decisions_wait_for_its_lock_holds_no_other_room() {
    use roomkit::ProcessInboundOptions;
    use roomkit::model::{EventContent, HookResult, InboundMessage, RoomEvent, TextContent};
    use tokio::sync::{Notify, watch};

    let kit = RoomKit::new(Default::default());
    let provider = ScriptedProvider::new([ScriptedRound::answering("Done")]);
    let channel = prepare(&kit, provider, Arc::default(), false).await;
    let sms = ConversationChannel::buffered(ChannelId::new("sms").unwrap());
    kit.register_channel(sms.clone()).unwrap();
    let room = config().room;
    kit.attach_channel(&room, sms.id(), Default::default())
        .await
        .unwrap();
    let other = RoomId::new("other").unwrap();
    kit.create_room(CreateRoomOptions {
        room_id: Some(other.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    for id in [channel.id(), &config().agent] {
        kit.attach_channel(&other, id, Default::default())
            .await
            .unwrap();
    }
    // The answer, as it is stored with the room locked, waits for a person's
    // review, then for the gate.
    let (open, gate) = watch::channel(false);
    let held = Arc::new(Notify::new());
    let (holding, asking) = (held.clone(), channel.clone());
    kit.hook(
        roomkit::HookTrigger::BeforeBroadcast,
        move |event: RoomEvent, _| {
            let (mut gate, held, asking) = (gate.clone(), holding.clone(), asking.clone());
            let answer = event.source.channel_id == config().agent;
            async move {
                if answer {
                    let request =
                        PermissionRequest::new("Send?", "Send the answer", Duration::from_secs(30));
                    asking
                        .ask_permission(&config().room, &config().agent, request)
                        .await;
                    held.notify_one();
                    let _ = gate.wait_for(|open| *open).await;
                }
                Ok(HookResult::allow(Default::default()))
            }
        },
        hook_options("hold-answer"),
    )
    .unwrap();
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel.clone(), config()).run(host));
    next(&client).await;

    let text = EventContent::Text(TextContent::new("Status?"));
    let mut message = InboundMessage::new(sms.id().clone(), "carol", text);
    message.addressed_to = Some(vec![config().agent]);
    let options = ProcessInboundOptions {
        room_id: Some(room.clone()),
        organization_id: None,
        defer_delivery: false,
    };
    let inbound = {
        let kit = kit.clone();
        tokio::spawn(async move { kit.process_inbound(message, options).await.map(|_| ()) })
    };
    let permission = requested(&client).await;
    client
        .try_send(decide(2, &permission, Decision::Allow))
        .unwrap();
    assert_eq!(reply(&client, 2).await, None);
    held.notified().await;

    let mut elsewhere = config();
    elsewhere.room = other;
    let attaching = LocalSession::new(kit.clone(), channel.clone(), elsewhere);
    let snapshot = tokio::time::timeout(Duration::from_secs(1), attaching.snapshot()).await;
    open.send_replace(true);
    assert!(
        snapshot.is_ok(),
        "Another room's view waited for this room's turn"
    );
    inbound.await.unwrap().unwrap();
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}
