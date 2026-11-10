//! A request the provider refuses as past the model's context window.

use super::*;

/// A conversation too short for the kit to compact: the refusal reaches the
/// view as the context limit, naming its run before the run ends, and not
/// as an unexplained engine failure.
#[tokio::test]
async fn a_refusal_past_the_context_window_is_told_as_such_with_its_run() {
    for buffered in [false, true] {
        refused_past_the_context_window(buffered).await;
    }
}

async fn refused_past_the_context_window(buffered: bool) {
    let kit = RoomKit::new(Default::default());
    let refused = ScriptedRound::answering("")
        .refusing(400, "This model's maximum context length is 131072 tokens")
        .with_context_overflow(true);
    let provider = NamedProvider(ScriptedProvider::new([refused]));
    let channel = prepare(&kit, provider, Arc::default(), buffered).await;
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel, config()).run(host));
    assert!(matches!(next(&client).await, Event::Snapshot(_)));

    client.try_send(request("too-long")).unwrap();
    let events = collect_operation(&client).await;

    assert_operation(&events, "too-long", RunOutcome::Failed);
    let run = events
        .iter()
        .find_map(|event| match event {
            Event::RunStarted { run, .. } => Some(run.clone()),
            _ => None,
        })
        .expect("the refused request's run");
    let notices: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            Event::Notice { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        notices.len(),
        1,
        "the refusal is explained once: {notices:?}"
    );
    assert!(notices[0].contains("context limit"), "{notices:?}");
    let at = |found: &dyn Fn(&Event) -> bool| events.iter().position(found);
    let exceeded = at(
        &|event| matches!(event, Event::ContextExceeded { run: refused, .. } if *refused == run),
    )
    .unwrap_or_else(|| panic!("the refusal names its run (buffered: {buffered})"));
    let finished =
        at(&|event| matches!(event, Event::RunFinished { run: ended, .. } if *ended == run))
            .unwrap();
    assert!(exceeded < finished, "told while the run is the latest");
    drop(client);
    task.await.unwrap().unwrap();
}
