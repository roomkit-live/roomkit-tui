//! Application commands, forms, host work and prepared messages.

use super::*;
#[tokio::test]
async fn application_commands_wait_for_idle_and_validate_switches_before_replacing_the_view() {
    use roomkit_tui_engine::{CommandHandler, CommandResult};
    struct Switcher(Arc<AtomicUsize>);
    impl CommandHandler for Switcher {
        async fn handle(
            &self,
            request: Request,
            mut view: SessionConfig,
        ) -> Result<CommandResult, Rejection> {
            self.0.fetch_add(1, Ordering::SeqCst);
            let Command::Invoke { arguments, .. } = request.command else {
                return Err("Unsupported".into());
            };
            view.room = RoomId::new(arguments).map_err(|_| "Invalid room")?;
            Ok(CommandResult::Switch(Box::new(view)))
        }
    }
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
        ScriptedRound::answering("Complete"),
    ]);
    let channel = prepare(&kit, provider, Arc::new(AtomicUsize::new(0)), false).await;
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
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let notify = entered.clone();
    let gate = release.clone();
    kit.hook(
        roomkit::HookTrigger::BeforeToolUse,
        move |_: roomkit::ToolCallEvent, _| {
            let notify = notify.clone();
            let gate = gate.clone();
            async move {
                notify.notify_one();
                gate.notified().await;
                Ok(roomkit::model::HookResult::allow(Default::default()))
            }
        },
        hook_options("hold-for-switch"),
    )
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut view = config();
    view.commands = vec![CommandSpec::new("resume", "Switch room").with_arguments("id")];
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let task = tokio::spawn(
        LocalSession::new(kit.clone(), channel, view)
            .run_with_commands(host, Switcher(calls.clone())),
    );
    assert!(matches!(next(&client).await, Event::Snapshot(_)));
    let invoke = |id, target: &str| {
        Request::new(
            RequestId::new(id),
            format!("switch-{id}"),
            "room",
            Command::Invoke {
                name: "resume".into(),
                arguments: target.into(),
            },
        )
    };
    client.try_send(invoke(2, "missing")).unwrap();
    assert!(matches!(
        next(&client).await,
        Event::Reply { error: Some(_), .. }
    ));
    client.try_send(request("slow-turn")).unwrap();
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    client.try_send(invoke(3, "other")).unwrap();
    loop {
        if matches!(
            next(&client).await,
            Event::Reply { request, error: Some(_) } if request == RequestId::new(3)
        ) {
            break;
        }
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "The busy command never reached application code"
    );
    release.notify_one();
    collect_operation(&client).await;
    // Review finding 9: the room is free as soon as the operation is reported
    // finished; the view used to refuse requests until its task was joined.
    client.try_send(invoke(4, "other")).unwrap();
    assert!(matches!(
        next(&client).await,
        Event::Reply { error: None, .. }
    ));
    assert!(
        matches!(next(&client).await, Event::Snapshot(snapshot) if snapshot.conversation.as_str() == "other" && snapshot.items.is_empty())
    );
    client.try_send(invoke(5, "room")).unwrap();
    assert!(matches!(
        next(&client).await,
        Event::Reply { error: Some(_), .. }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

#[tokio::test]
async fn forms_dispatch_only_advertised_bounded_scoped_values_and_setup_blocks_messages() {
    use roomkit_tui_engine::{CommandHandler, CommandResult};
    struct Handler(Arc<AtomicUsize>);
    impl CommandHandler for Handler {
        async fn handle(
            &self,
            request: Request,
            _: SessionConfig,
        ) -> Result<CommandResult, Rejection> {
            let Command::SubmitForm { form, values, .. } = request.command else {
                return Err("Unsupported".into());
            };
            if form != "host-token" || values.get("credential") != Some("fixture-secret") {
                return Err("Invalid form".into());
            }
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(CommandResult::Events(vec![Event::Notice {
                conversation: "room".into(),
                text: "Saved".into(),
            }]))
        }
    }
    let kit = RoomKit::new(Default::default());
    let channel = prepare(
        &kit,
        ScriptedProvider::new([]),
        Arc::new(AtomicUsize::new(0)),
        false,
    )
    .await;
    let calls = Arc::new(AtomicUsize::new(0));
    let mut config = config();
    config
        .commands
        .push(CommandSpec::new("settings", "Configure"));
    config.submission_error = Some("Configure an agent first".into());
    config
        .actor_names
        .insert("agent".into(), "Configured name".into());
    let (client, host) = connection(NonZeroUsize::new(32).unwrap());
    let task = tokio::spawn(
        LocalSession::new(kit.clone(), channel, config)
            .run_with_commands(host, Handler(calls.clone())),
    );
    assert!(
        matches!(next(&client).await, Event::Snapshot(s) if s.actors.iter().any(|a| a.name == "Configured name"))
    );
    assert!(
        matches!(next(&client).await, Event::Notice { text, .. } if text == "Configure an agent first")
    );
    client.try_send(request("blocked-message")).unwrap();
    assert!(
        matches!(next(&client).await, Event::Reply { error: Some(e), .. } if e.message == "Configure an agent first")
    );
    let command = Command::SubmitForm {
        name: "settings".into(),
        form: "host-token".into(),
        values: Box::new([("credential", "fixture-secret")].into_iter().collect()),
    };
    for (conversation, name, value) in [
        ("other", "settings", "fixture-secret".into()),
        ("room", "unknown", "fixture-secret".into()),
        ("room", "settings", "x".repeat(8193)),
    ] {
        let mut request = request("rejected");
        request.conversation = conversation.into();
        request.command = Command::SubmitForm {
            name: name.into(),
            form: "host-token".into(),
            values: Box::new([("credential".to_owned(), value)].into_iter().collect()),
        };
        client.try_send(request).unwrap();
        assert!(matches!(
            next(&client).await,
            Event::Reply { error: Some(_), .. }
        ));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    client
        .try_send({
            let mut save = request("save");
            save.command = command;
            save
        })
        .unwrap();
    assert!(matches!(
        next(&client).await,
        Event::Reply { error: None, .. }
    ));
    assert!(matches!(next(&client).await, Event::Notice { text, .. } if text == "Saved"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let events = kit
        .get_timeline(&RoomId::new("room").unwrap(), Default::default())
        .await
        .unwrap();
    assert!(!format!("{events:?}").contains("fixture-secret"));
    drop(client);
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn host_execution_is_scoped_interruptible_and_does_not_call_the_model() {
    use roomkit_tui_engine::{CommandExecution, CommandHandler, CommandResult};
    struct Handler;
    impl CommandHandler for Handler {
        async fn handle(
            &self,
            request: Request,
            _: SessionConfig,
        ) -> Result<CommandResult, Rejection> {
            let Command::Invoke { arguments, .. } = request.command else {
                unreachable!()
            };
            // Each execution needs a new run: a terminal ignores a repeat.
            let run = match arguments.as_str() {
                "" => "host-run".to_owned(),
                other => format!("host-run-{other}"),
            };
            Ok(CommandResult::Execute(CommandExecution::new(
                run.into(),
                move |mut context| async move {
                    if arguments == "panic" {
                        panic!("fixture panic");
                    }
                    context.publish(Item::Message(Message::new(
                        "host-item",
                        "person:alice",
                        "Working",
                    )));
                    context.cancelled().await;
                    // Completion is emitted only after joined host cleanup.
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    context.publish(Item::Message(Message::new(
                        "host-item",
                        "person:alice",
                        "Stopped",
                    )));
                    Ok(RunOutcome::Interrupted)
                },
            )))
        }
    }
    let kit = RoomKit::new(Default::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let channel = prepare(&kit, ScriptedProvider::new([]), calls.clone(), false).await;
    let mut config = config();
    config.commands.push(CommandSpec::new("work", "Host work"));
    let check = LocalSession::new(kit.clone(), channel.clone(), config.clone());
    let (client, host) = connection(NonZeroUsize::new(64).unwrap());
    let task = tokio::spawn(
        LocalSession::new(kit.clone(), channel, config).run_with_commands(host, Handler),
    );
    assert!(matches!(next(&client).await, Event::Snapshot(_)));
    let mut start = request("host-operation");
    start.command = Command::Invoke {
        name: "work".into(),
        arguments: String::new(),
    };
    client.try_send(start.clone()).unwrap();
    while !matches!(next(&client).await, Event::Item { .. }) {}
    assert!(matches!(check.snapshot().await, Err(AdapterError::Busy)));
    let mut interrupt = request("interrupt");
    interrupt.command = Command::Interrupt {
        run: "wrong-run".into(),
    };
    client.try_send(interrupt.clone()).unwrap();
    assert!(matches!(
        next(&client).await,
        Event::Reply { error: Some(_), .. }
    ));
    client.try_send(request("blocked-submit")).unwrap();
    assert!(matches!(
        next(&client).await,
        Event::Reply { error: Some(_), .. }
    ));
    interrupt.command = Command::Interrupt {
        run: "host-run".into(),
    };
    client.try_send(interrupt).unwrap();
    assert!(matches!(
        next(&client).await,
        Event::Reply { error: None, .. }
    ));
    assert!(
        matches!(next(&client).await, Event::Item { item: Item::Message(m), .. } if m.text == "Stopped")
    );
    assert!(matches!(
        next(&client).await,
        Event::RunFinished {
            outcome: RunOutcome::Interrupted,
            ..
        }
    ));
    assert!(
        matches!(next(&client).await, Event::OperationFinished { operation, outcome: RunOutcome::Interrupted, .. } if operation.as_str() == "host-operation")
    );
    assert!(check.snapshot().await.is_ok());
    start.command = Command::Invoke {
        name: "work".into(),
        arguments: "panic".into(),
    };
    client.try_send(start).unwrap();
    let events = collect_operation(&client).await;
    assert_operation(&events, "host-operation", RunOutcome::Failed);
    assert!(events.iter().any(|event| matches!(
        event,
        Event::RunFinished {
            outcome: RunOutcome::Failed,
            ..
        }
    )));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(check.snapshot().await.is_ok());
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

#[tokio::test]
async fn host_prepared_messages_use_normal_validation_and_operation_lifecycle() {
    use roomkit_tui_engine::{CommandHandler, CommandResult};
    struct Handler;
    impl CommandHandler for Handler {
        async fn handle(
            &self,
            request: Request,
            _: SessionConfig,
        ) -> Result<CommandResult, Rejection> {
            let Command::Invoke { arguments, .. } = request.command else {
                unreachable!()
            };
            Ok(CommandResult::Submit(arguments))
        }
    }
    let kit = RoomKit::new(Default::default());
    let channel = prepare(
        &kit,
        ScriptedProvider::new([ScriptedRound::answering("Done")]),
        Arc::new(AtomicUsize::new(0)),
        false,
    )
    .await;
    let mut view = config();
    view.commands
        .push(CommandSpec::new("template", "Prepared message"));
    let (client, host) = connection(NonZeroUsize::new(64).unwrap());
    let task = tokio::spawn(
        LocalSession::new(kit.clone(), channel, view).run_with_commands(host, Handler),
    );
    assert!(matches!(next(&client).await, Event::Snapshot(_)));
    let mut invalid = request("empty");
    invalid.command = Command::Invoke {
        name: "template".into(),
        arguments: String::new(),
    };
    client.try_send(invalid).unwrap();
    assert!(matches!(
        next(&client).await,
        Event::Reply { error: Some(_), .. }
    ));
    let mut valid = request("prepared");
    valid.command = Command::Invoke {
        name: "template".into(),
        arguments: "A prepared user request".into(),
    };
    client.try_send(valid).unwrap();
    let mut started = false;
    let mut message = false;
    loop {
        match next(&client).await {
            Event::Reply { error, .. } => assert!(error.is_none()),
            Event::OperationStarted { operation, .. } => {
                assert_eq!(operation.as_str(), "prepared");
                started = true;
            }
            Event::Item {
                item: Item::Message(item),
                ..
            } if item.text.contains("A prepared user request") => message = true,
            Event::OperationFinished {
                operation, outcome, ..
            } => {
                assert_eq!(operation.as_str(), "prepared");
                assert_eq!(outcome, RunOutcome::Completed);
                break;
            }
            _ => {}
        }
    }
    assert!(started && message);
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}
