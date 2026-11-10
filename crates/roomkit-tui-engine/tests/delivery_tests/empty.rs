use super::*;

/// A completed generation with no visible answer is not a successful UI turn.
#[tokio::test]
async fn an_empty_answer_is_explained_once_preserves_usage_and_allows_retry() {
    for text in ["", " \n"] {
        let kit = RoomKit::new(Default::default());
        let provider = NamedProvider(ScriptedProvider::new([
            ScriptedRound::answering(text)
                .with_finish_reason("max_tokens")
                .with_usage(
                    json!({"input_tokens":20, "output_tokens":1024})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            ScriptedRound::answering("Recovered answer."),
        ]));
        let channel = prepare(&kit, provider, Arc::default(), false).await;
        let (client, host) = connection(NonZeroUsize::new(128).unwrap());
        let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
        next(&client).await;
        client.try_send(request("empty")).unwrap();
        let events = collect_operation(&client).await;
        assert_operation(&events, "empty", RunOutcome::Failed);
        let notices: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                Event::Notice { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            notices,
            [
                "The model returned no visible answer. Retry, or adjust the output limit and reasoning effort."
            ]
        );
        assert!(
            events.iter().any(|e| matches!(e,
                Event::ContextUpdated { usage, .. } if usage.tokens == 1044
            )),
            "{events:?}"
        );
        client.try_send(request("retry")).unwrap();
        let events = collect_operation(&client).await;
        assert_operation(&events, "retry", RunOutcome::Completed);
        assert!(
            events.iter().any(|e| matches!(e,
                Event::MessageFinalized { message, .. } if message.text == "Recovered answer."
            )),
            "{events:?}"
        );
        drop(client);
        task.await.unwrap().unwrap();
        kit.close().await.unwrap();
    }
}

/// The engine's automatic retry after a tool must finish before judging the turn.
#[tokio::test]
async fn an_empty_generation_after_a_tool_keeps_the_engines_retry() {
    let kit = RoomKit::new(Default::default());
    let provider = ScriptedProvider::new([
        ScriptedRound::calling("", [("c1", "lookup_record", "{}")]),
        ScriptedRound::answering("").with_finish_reason("end_turn"),
        ScriptedRound::answering("The record is ready."),
    ]);
    let calls = Arc::new(AtomicUsize::new(0));
    let channel = prepare(&kit, provider, calls.clone(), false).await;
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    next(&client).await;
    client.try_send(request("tool-retry")).unwrap();
    let events = collect_operation(&client).await;
    assert_operation(&events, "tool-retry", RunOutcome::Completed);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        events.iter().any(|e| matches!(e,
            Event::MessageFinalized { message, .. } if message.text == "The record is ready."
        )),
        "{events:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(e, Event::Notice { .. })),
        "{events:?}"
    );
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}
