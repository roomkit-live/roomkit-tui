//! Approvals asked by application work, outside a model turn.

use super::*;
use roomkit_tui_engine::{CommandExecution, CommandHandler, CommandResult};

/// Asks before deploying, from host work. `wait` shows the answer it got;
/// `leave` asks from a detached task and ends without waiting for it.
struct Deployer(ConversationChannel);

impl CommandHandler for Deployer {
    async fn handle(
        &self,
        request: Request,
        view: SessionConfig,
    ) -> Result<CommandResult, Rejection> {
        let Command::Invoke { arguments, .. } = request.command else {
            return Err("Unsupported".into());
        };
        let channel = self.0.clone();
        let work = CommandExecution::new("deploy-1".into(), move |context| async move {
            let request =
                PermissionRequest::new("Deploy?", "Deploy to production", Duration::from_secs(2));
            let ask = async move {
                channel
                    .ask_permission(&view.room, &view.agent, request)
                    .await
            };
            if arguments == "leave" {
                tokio::spawn(ask);
                tokio::time::sleep(Duration::from_millis(100)).await;
            } else {
                let answer = format!("{:?}", ask.await);
                context.publish(Item::Message(Message::new(
                    "answer",
                    "person:alice",
                    answer,
                )));
            }
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
    let task = tokio::spawn(session.run_with_commands(host, Deployer(channel.clone())));
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

/// RMK-1107 review: an interrupted run's approval stayed answerable, and a
/// late Allow reached work the person had stopped.
#[tokio::test]
async fn interrupting_host_work_withdraws_its_pending_approval() {
    let kit = RoomKit::new(Default::default());
    let channel = prepare(&kit, ScriptedProvider::new([]), Arc::default(), false).await;
    let (alice, task) = attach(&kit, &channel, "alice").await;
    deploy(&alice, "wait");
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
    let (mut late, mut answer, mut settled) = (None, None, false);
    while late.is_none() || !settled {
        match next(&alice).await {
            Event::Reply { request, error } if request == RequestId::new(3) => late = Some(error),
            Event::Item {
                item: Item::Message(message),
                ..
            } => answer = Some(message.text),
            Event::OperationFinished { .. } => settled = true,
            _ => {}
        }
    }
    assert!(late.flatten().is_some(), "No longer answerable");
    assert_eq!(answer.as_deref(), Some("Deny"));
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
