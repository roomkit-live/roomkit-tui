//! Approvals asked by application work, outside a model turn.

use super::*;
use roomkit_tui_engine::{CommandExecution, CommandHandler, CommandResult};

/// Asks before deploying, from host work, and shows the answer it got.
struct Deployer(ConversationChannel);

impl CommandHandler for Deployer {
    async fn handle(&self, _: Request, view: SessionConfig) -> Result<CommandResult, Rejection> {
        let channel = self.0.clone();
        let work = CommandExecution::new("deploy-1".into(), move |context| async move {
            let request =
                PermissionRequest::new("Deploy?", "Deploy to production", Duration::from_secs(2));
            let decision = channel
                .ask_permission(&view.room, &view.agent, request)
                .await;
            let answer = format!("{decision:?}");
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

/// RMK-1107: the broker knew only model turns, so host work waited for the
/// whole timeout and was denied without the person ever being asked.
#[tokio::test]
async fn host_work_asks_the_person_who_invoked_it() {
    let kit = RoomKit::new(Default::default());
    let channel = prepare(&kit, ScriptedProvider::new([]), Arc::default(), false).await;
    let mut view = config();
    view.commands.push(CommandSpec::new("deploy", "Deploy"));
    let (client, host) = connection(NonZeroUsize::new(64).unwrap());
    let session = LocalSession::new(kit.clone(), channel.clone(), view);
    let task = tokio::spawn(session.run_with_commands(host, Deployer(channel)));
    assert!(matches!(next(&client).await, Event::Snapshot(_)));
    let deploy = Command::Invoke {
        name: "deploy".into(),
        arguments: String::new(),
    };
    client
        .try_send(Request::new(RequestId::new(1), "deploy", "room", deploy))
        .unwrap();
    let permission = loop {
        match next(&client).await {
            Event::PermissionRequested { permission, .. } => break permission,
            Event::RunFinished { .. } => panic!("The work ended without asking"),
            _ => {}
        }
    };
    assert_eq!(permission.run.as_str(), "deploy-1");
    let approve = Command::Decide {
        permission: permission.id,
        decision: Decision::Allow,
    };
    client
        .try_send(Request::new(RequestId::new(2), "approve", "room", approve))
        .unwrap();
    let answer = loop {
        if let Event::Item {
            item: Item::Message(message),
            ..
        } = next(&client).await
        {
            break message.text;
        }
    };
    assert_eq!(answer, "Allow", "The work received the person's answer");
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}
