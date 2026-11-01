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
    kit.before_tool_use(roomkit::ToolHookSpec::new("hold"), move |_, _| {
        let mut gate = gate.clone();
        async move {
            let _ = gate.wait_for(|open| *open).await;
            Ok(roomkit::ToolUse::Allow)
        }
    })
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
