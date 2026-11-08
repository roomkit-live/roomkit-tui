//! Regressions for defects found by the pre-release review. Each test failed
//! on the code before its fix.

use super::*;
use roomkit::{CloseRoomOptions, model::HookResult};
use roomkit_tui_engine::{CommandHandler, CommandResult};
use tokio::sync::Notify;

async fn limited(kit: &RoomKit, rounds: Vec<ScriptedRound>, buffered: bool) -> ConversationChannel {
    let lookup = AiTool {
        name: "lookup_record".to_owned(),
        description: "Read".to_owned(),
        parameters: json!({"type":"object","properties":{},"additionalProperties":false}),
        tags: Vec::new(),
        changes_state: false,
    };
    let options = AiChannelOptions {
        tools: vec![ToolEntry::Definition(lookup)],
        tool_handler: Some(ToolHandler::new(|_, _| async {
            Ok(ToolResult::Json(json!({"status":"ready"})))
        })),
        max_tool_rounds: 1,
        ..Default::default()
    };
    let provider = ScriptedProvider::new(rounds);
    let agent = AiChannel::new(ChannelId::new("agent").unwrap(), provider, options).unwrap();
    kit.register_channel(agent).unwrap();
    let id = ChannelId::new("console").unwrap();
    let channel = if buffered {
        ConversationChannel::buffered(id)
    } else {
        ConversationChannel::new(id)
    };
    kit.register_channel(channel.clone()).unwrap();
    let room = config().room;
    let options = CreateRoomOptions {
        room_id: Some(room.clone()),
        ..Default::default()
    };
    kit.create_room(options).await.unwrap();
    for id in [channel.id(), &config().agent] {
        kit.attach_channel(&room, id, Default::default())
            .await
            .unwrap();
    }
    channel
}

fn calling() -> ScriptedRound {
    ScriptedRound::calling("Checking", [("call1", "lookup_record", "{}")])
}

/// Finding 13: a buffered delivery ending on the round limit reported success.
#[tokio::test]
async fn a_buffered_round_limit_is_a_failure_with_a_notice() {
    let kit = RoomKit::new(Default::default());
    // The second generation exceeds the one allowed round of calls.
    let channel = limited(&kit, vec![calling(), calling()], true).await;
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    next(&client).await;
    client.try_send(request("limited")).unwrap();
    let events = collect_operation(&client).await;
    let outcome = events.iter().find_map(|e| match e {
        Event::RunFinished { outcome, .. } => Some(*outcome),
        _ => None,
    });
    assert_eq!(outcome, Some(RunOutcome::Failed), "{events:#?}");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Notice { text, .. } if text.contains("round limit"))),
        "{events:#?}"
    );
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

/// Finding 14: tool items named the engine's correlation, not their run.
#[tokio::test]
async fn tool_items_belong_to_the_announced_run() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![calling(), ScriptedRound::answering("Done")];
    let channel = prepare(&kit, ScriptedProvider::new(rounds), Arc::default(), false).await;
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel.clone(), config()).run(host));
    next(&client).await;
    client.try_send(request("one")).unwrap();
    let events = collect_operation(&client).await;
    let started = events.iter().find_map(|e| match e {
        Event::RunStarted { run, .. } => Some(run.clone()),
        _ => None,
    });
    let tools: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::Item {
                item: Item::Tool { run, .. },
                ..
            } => Some(run.clone()),
            _ => None,
        })
        .collect();
    assert!(!tools.is_empty());
    assert!(
        tools.iter().all(|run| Some(run) == started.as_ref()),
        "{tools:?} {started:?}"
    );
    let restored = LocalSession::new(kit.clone(), channel, config())
        .snapshot()
        .await
        .unwrap();
    assert!(restored.items.iter().all(|item| match item {
        Item::Tool { run, .. } => Some(run) == started.as_ref(),
        _ => true,
    }));
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

struct SlowSubmit {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

impl CommandHandler for SlowSubmit {
    async fn handle(&self, _: Request, _: SessionConfig) -> Result<CommandResult, Rejection> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(CommandResult::Submit("Prepared text".into()))
    }
}

/// Findings 8 and 10: a view awaiting its command handler stopped forwarding
/// events, and started a second operation in a room another view had started.
#[tokio::test]
async fn concurrent_views_never_start_two_operations_in_one_room() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![
        ScriptedRound::answering("First"),
        ScriptedRound::answering("Second"),
    ];
    let channel = prepare(&kit, ScriptedProvider::new(rounds), Arc::default(), false).await;
    // Hold view B's operation in admission until the gate opens for good.
    let (open, gate) = tokio::sync::watch::channel(false);
    let admitted = Arc::new(Notify::new());
    let reached = admitted.clone();
    kit.hook(
        roomkit::HookTrigger::BeforeBroadcast,
        move |_: roomkit::model::RoomEvent, _| {
            let (mut gate, reached) = (gate.clone(), reached.clone());
            async move {
                reached.notify_one();
                let _ = gate.wait_for(|open| *open).await;
                Ok(HookResult::allow(Default::default()))
            }
        },
        hook_options("hold"),
    )
    .unwrap();
    let handler = SlowSubmit {
        entered: Arc::new(Notify::new()),
        release: Arc::new(Notify::new()),
    };
    let (entered, release) = (handler.entered.clone(), handler.release.clone());
    let mut alice = config();
    alice.commands = vec![CommandSpec::new("prepare", "Prepare a message")];
    let mut bob = config();
    bob.sender = "bob".into();
    let (a, a_host) = connection(NonZeroUsize::new(256).unwrap());
    let (b, b_host) = connection(NonZeroUsize::new(256).unwrap());
    let view_a = LocalSession::new(kit.clone(), channel.clone(), alice);
    let a_task = tokio::spawn(view_a.run_with_commands(a_host, handler));
    let b_task = tokio::spawn(LocalSession::new(kit.clone(), channel.clone(), bob).run(b_host));
    next(&a).await;
    next(&b).await;
    let invoke = Command::Invoke {
        name: "prepare".into(),
        arguments: String::new(),
    };
    a.try_send(Request::new(RequestId::new(1), "a-prepare", "room", invoke))
        .unwrap();
    entered.notified().await;
    b.try_send(request("b-submit")).unwrap();
    admitted.notified().await;
    let forwarded = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Event::OperationStarted { .. } = next(&a).await {
                break;
            }
        }
    })
    .await;
    assert!(
        forwarded.is_ok(),
        "View A stopped forwarding events while its handler waited"
    );
    release.notify_one();
    let reply = loop {
        if let Event::Reply { request, error } = next(&a).await
            && request == RequestId::new(1)
        {
            break error;
        }
    };
    assert_eq!(reply.map(|r| r.code), Some(ErrorCode::Busy));
    open.send_replace(true);
    drop((a, b));
    let _ = (a_task.await, b_task.await);
    kit.close().await.unwrap();
}

/// Finding 12: a room closed with an unsaved decision could not be viewed.
#[tokio::test]
async fn a_closed_room_still_restores_with_its_unsaved_decisions() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![calling(), ScriptedRound::answering("Done")];
    let channel = prepare(&kit, ScriptedProvider::new(rounds), Arc::default(), false).await;
    let (gate, closer) = (channel.clone(), kit.clone());
    kit.hook(
        roomkit::HookTrigger::BeforeToolUse,
        move |event: roomkit::ToolCallEvent, _| {
            let (gate, closer) = (gate.clone(), closer.clone());
            async move {
                let request =
                    PermissionRequest::new("Read?", "Read a record", Duration::from_secs(30));
                let decision = gate.ask_tool_permission(&event, request).await;
                // The tool closes the room: the decision can no longer be stored.
                closer
                    .close_room(event.room_id.as_ref().unwrap(), CloseRoomOptions::default())
                    .await
                    .unwrap();
                Ok(match decision {
                    Decision::Allow => roomkit::model::HookResult::allow(Default::default()),
                    _ => roomkit::model::HookResult::block("Denied", Default::default()),
                })
            }
        },
        hook_options("permission"),
    )
    .unwrap();
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel.clone(), config()).run(host));
    next(&client).await;
    client.try_send(request("closing")).unwrap();
    let permission = loop {
        if let Event::PermissionRequested { permission, .. } = next(&client).await {
            break permission;
        }
    };
    let decide = Command::Decide {
        permission: permission.id.clone(),
        decision: Decision::Allow,
    };
    client
        .try_send(Request::new(RequestId::new(2), "decide", "room", decide))
        .unwrap();
    collect_operation(&client).await;
    drop(client);
    task.await.unwrap().unwrap();
    let restored = LocalSession::new(kit.clone(), channel, config())
        .snapshot()
        .await;
    let restored = restored.expect("A closed room can still be viewed");
    assert!(restored.items.iter().any(
        |item| matches!(item, Item::Approval(record) if record.permission.id == permission.id)
    ));
    kit.close().await.unwrap();
}

/// Finding 11: an application closing its engine right after the view lost
/// a decision; it can now wait until every operation has settled.
#[tokio::test]
async fn idle_waits_for_accepted_work_to_settle() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![ScriptedRound::answering("Done")];
    let channel = prepare(&kit, ScriptedProvider::new(rounds), Arc::default(), false).await;
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel.clone(), config()).run(host));
    next(&client).await;
    client.try_send(request("settling")).unwrap();
    loop {
        if let Event::OperationStarted { .. } = next(&client).await {
            break;
        }
    }
    drop(client);
    task.await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(5), channel.idle())
        .await
        .expect("accepted work settles");
    let rows = kit
        .get_timeline(&config().room, Default::default())
        .await
        .unwrap();
    assert!(
        rows.iter()
            .any(|row| roomkit_core::rules::transcoding::text_of(&row.content) == "Done")
    );
    kit.close().await.unwrap();
}
