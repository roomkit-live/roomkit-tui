//! What a view is told when a model provider fails.
use super::*;

/// A provider failure is explained once: a streamed run tells it from its
/// stream, and the turn that delivered the message does not tell it again.
#[tokio::test]
async fn a_provider_failure_is_told_once() {
    let round = || ScriptedRound::answering("");
    for (failure, expected) in [
        (
            round().failing("Request timed out."),
            "Model request timed out. Increase the provider timeout in your configuration, then retry.",
        ),
        (
            round().failing("The provider answered with a body it could not read: missing content"),
            "The provider returned an invalid response. Check protocol compatibility and host diagnostics.",
        ),
        (
            round().failing("opaque stream failure"),
            "The provider request failed. Check the host diagnostics; completed tool actions may remain.",
        ),
        (
            round()
                .refusing(403, "Anthropic declined this request")
                .refused_by("anthropic"),
            "Provider access denied. Check the API key and endpoint permissions.",
        ),
        (
            round().refusing(503, "connect error"),
            "Provider server failed. Check its health and logs, then retry.",
        ),
    ] {
        for buffered in [false, true] {
            assert_provider_notice(failure.clone(), expected, buffered).await;
        }
    }
}

async fn assert_provider_notice(failure: ScriptedRound, expected: &str, buffered: bool) {
    let kit = RoomKit::new(Default::default());
    let provider = ScriptedProvider::new([failure]);
    let channel = prepare(&kit, provider, Arc::default(), buffered).await;
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    next(&client).await;
    client.try_send(request("failure")).unwrap();
    let events = collect_operation(&client).await;
    assert_operation(&events, "failure", RunOutcome::Failed);
    let notices: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            Event::Notice { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(notices, [expected], "buffered: {buffered}");
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

#[tokio::test]
async fn a_view_reads_each_external_streams_own_failure_record() {
    use roomkit::model::{InboundMessage, TextContent};

    let kit = RoomKit::new(Default::default());
    let refusal = |status| {
        ScriptedRound::answering("")
            .refusing(status, "Anthropic declined this request")
            .refused_by("anthropic")
    };
    let provider = ScriptedProvider::new([
        refusal(403),
        refusal(503),
        ScriptedRound::answering("").failing("openai (403): private detail"),
    ]);
    let channel = prepare(&kit, provider, Arc::default(), false).await;
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let id = channel.id().clone();
    let view = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    assert!(matches!(next(&client).await, Event::Snapshot(_)));
    for expected in [
        "Provider access denied. Check the API key and endpoint permissions.",
        "Provider server failed. Check its health and logs, then retry.",
        "The provider request failed. Check the host diagnostics; completed tool actions may remain.",
    ] {
        let mut message = InboundMessage::new(id.clone(), "alice", TextContent::new("hello"));
        message.addressed_to = Some(vec![config().agent]);
        let options = roomkit::ProcessInboundOptions {
            room_id: Some(config().room),
            ..Default::default()
        };
        assert!(
            kit.process_inbound(message, options)
                .await
                .unwrap()
                .error
                .is_some()
        );
        let mut notices = Vec::new();
        loop {
            match next(&client).await {
                Event::Notice { text, .. } => notices.push(text),
                Event::RunFinished { outcome, .. } => {
                    assert_eq!(outcome, RunOutcome::Failed);
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(notices, [expected]);
    }
    drop(client);
    view.await.unwrap().unwrap();
    kit.close().await.unwrap();
}
