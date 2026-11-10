//! Only the person whose message started a turn answers its requests.

use super::*;

/// A view of the room for `sender`, its client and its serving task.
async fn view(
    kit: &RoomKit,
    channel: &ConversationChannel,
    sender: &str,
) -> (Client, tokio::task::JoinHandle<Result<(), AdapterError>>) {
    let mut config = config();
    config.sender = sender.into();
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel.clone(), config).run(host));
    assert!(matches!(next(&client).await, Event::Snapshot(_)));
    (client, task)
}

/// An agent declaring no window of recent events (`max_context_events: 0`),
/// whose tool asks its permission from the tool handler.
fn agent_declaring_no_window(channel: &ConversationChannel) -> AiChannel<ScriptedProvider> {
    let gate = channel.clone();
    let handler = ToolHandler::new(move |_, _| {
        let gate = gate.clone();
        async move {
            let request = PermissionRequest::new("Read?", "Read a record", Duration::from_secs(5));
            let decision = gate
                .ask_permission(&config().room, &config().agent, request)
                .await;
            Ok(ToolResult::Text(format!("{decision:?}")))
        }
    });
    let provider = ScriptedProvider::new([
        ScriptedRound::calling("Checking", [("call1", "lookup_record", "{}")]),
        ScriptedRound::answering("Done"),
    ]);
    let lookup = AiTool {
        name: "lookup_record".to_owned(),
        description: "Read".to_owned(),
        parameters: json!({"type":"object","properties":{},"additionalProperties":false}),
        tags: Vec::new(),
        changes_state: false,
        defer_loading: false,
    };
    let options = AiChannelOptions {
        tools: vec![ToolEntry::Definition(lookup)],
        tool_handler: Some(handler),
        max_context_events: 0,
        ..Default::default()
    };
    AiChannel::new(config().agent, provider, options).unwrap()
}

/// RMK-1177: roomkit-rs sizes a stream's context by the windows its room's
/// channels declare. The console declared none, so beside an agent declaring
/// none and with no hook registered, the engine could not find the turn's
/// trigger, lost its requester, and let Bob approve Alice's request.
#[tokio::test]
async fn only_the_requester_answers_beside_an_agent_declaring_no_window() {
    let kit = RoomKit::new(Default::default());
    let channel = ConversationChannel::new(ChannelId::new("console").unwrap());
    kit.register_channel(agent_declaring_no_window(&channel))
        .unwrap();
    kit.register_channel(channel.clone()).unwrap();
    let room = config().room;
    kit.create_room(CreateRoomOptions {
        room_id: Some(room.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    for id in [channel.id().clone(), config().agent] {
        kit.attach_channel(&room, &id, Default::default())
            .await
            .unwrap();
    }
    let (alice, alice_task) = view(&kit, &channel, "alice").await;
    let (bob, bob_task) = view(&kit, &channel, "bob").await;
    alice.try_send(request("op")).unwrap();
    let permission = loop {
        if let Event::PermissionRequested { permission, .. } = next(&bob).await {
            break permission;
        }
    };
    let decide = |decision| Command::Decide {
        permission: permission.id.clone(),
        decision,
    };
    bob.try_send(Request::new(
        RequestId::new(2),
        "bob",
        "room",
        decide(Decision::Allow),
    ))
    .unwrap();
    let refused = loop {
        if let Event::Reply { request, error } = next(&bob).await
            && request == RequestId::new(2)
        {
            break error.map(|rejection| rejection.code);
        }
    };
    assert_eq!(refused, Some(ErrorCode::Refused), "Only Alice may answer");
    alice
        .try_send(Request::new(
            RequestId::new(3),
            "alice",
            "room",
            decide(Decision::Deny),
        ))
        .unwrap();
    let accepted = loop {
        if let Event::Reply { request, error } = next(&alice).await
            && request == RequestId::new(3)
        {
            break error.is_none();
        }
    };
    assert!(accepted, "Alice answers her own request");
    while !matches!(next(&alice).await, Event::OperationFinished { .. }) {}
    drop((alice, bob));
    alice_task.await.unwrap().unwrap();
    bob_task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

/// RMK-1311, review of aa33d7c: a turn that answers what the conversation holds
/// starts from an instruction, never stored, so its stream finds no trigger:
/// Bob could approve the requests of the turn Alice asked for.
#[tokio::test]
async fn only_the_requester_answers_in_an_answer_turn() {
    let kit = RoomKit::new(Default::default());
    let channel = ConversationChannel::new(ChannelId::new("console").unwrap());
    kit.register_channel(agent_declaring_no_window(&channel))
        .unwrap();
    kit.register_channel(channel.clone()).unwrap();
    let room = config().room;
    kit.create_room(CreateRoomOptions {
        room_id: Some(room.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    for id in [channel.id().clone(), config().agent] {
        kit.attach_channel(&room, &id, Default::default())
            .await
            .unwrap();
    }
    let (alice, alice_task) = view(&kit, &channel, "alice").await;
    let (bob, bob_task) = view(&kit, &channel, "bob").await;

    let asked = Request::new(
        RequestId::new(1),
        "answer",
        "room",
        Command::Answer {
            agent: config().agent.as_str().into(),
        },
    );
    alice.try_send(asked).unwrap();
    let permission = loop {
        if let Event::PermissionRequested { permission, .. } = next(&bob).await {
            break permission;
        }
    };
    let decide = |decision| Command::Decide {
        permission: permission.id.clone(),
        decision,
    };
    bob.try_send(Request::new(
        RequestId::new(2),
        "bob",
        "room",
        decide(Decision::Allow),
    ))
    .unwrap();
    let refused = loop {
        if let Event::Reply { request, error } = next(&bob).await
            && request == RequestId::new(2)
        {
            break error.map(|rejection| rejection.code);
        }
    };
    assert_eq!(refused, Some(ErrorCode::Refused), "Only Alice may answer");
    alice
        .try_send(Request::new(
            RequestId::new(3),
            "alice",
            "room",
            decide(Decision::Deny),
        ))
        .unwrap();
    while !matches!(next(&alice).await, Event::OperationFinished { .. }) {}
    drop((alice, bob));
    alice_task.await.unwrap().unwrap();
    bob_task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}
