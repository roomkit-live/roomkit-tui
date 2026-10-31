//! The headless scenario run by `--check` and `--json`.

use std::error::Error;

use roomkit_tui::{Session, SessionObserver};
use roomkit_tui_protocol::*;

use crate::{Demo, Profile};

/// Execute a complete scripted scenario with the production UI projection.
/// Returns the ordered wire events for optional JSON output.
pub fn check(profile: Profile) -> Result<Vec<Event>, Box<dyn Error>> {
    let mut demo = Demo::new(profile);
    let observer = SessionObserver::default();
    let mut session = Session::default().with_observer(observer.clone());
    let mut events = vec![demo.initial()];
    session.apply(events[0].clone());
    let mut observations = vec![observer.latest().ok_or("Missing initial observation")?];
    session.insert("Inspect the fictional record");
    let request = session.request(session.input_command()?)?;
    session.sent(&request);
    session.set_draft("");
    for event in demo.handle(request) {
        session.apply(event.clone());
        events.push(event);
    }
    observations.push(observer.latest().ok_or("Missing permission observation")?);
    let decision = session
        .decide(Decision::Allow)
        .ok_or("No permission was requested")?;
    let request = session.request(decision)?;
    session.sent(&request);
    for event in demo.handle(request) {
        session.apply(event.clone());
        events.push(event);
    }
    observations.push(observer.latest().ok_or("Missing decision observation")?);
    while let Some(event) = demo.advance() {
        session.apply(event.clone());
        events.push(event);
    }
    observations.push(observer.latest().ok_or("Missing completion observation")?);
    if observations[0].run.is_some()
        || observations[1].permissions.is_empty()
        || !observations[2].permissions.is_empty()
        || observations[2]
            .run
            .as_ref()
            .is_none_or(|run| run.outcome.is_some())
        || observations[3].run.as_ref().and_then(|run| run.outcome) != Some(RunOutcome::Completed)
        || observations
            .iter()
            .any(|state| state.conversation.as_str() != "example")
    {
        return Err("Session observation did not follow the host-confirmed lifecycle".into());
    }
    let snapshot = session.snapshot().ok_or("No snapshot")?;
    if session.active_run().is_some() || !session.permissions().is_empty() {
        return Err("Execution did not settle".into());
    }
    let success = snapshot.items.iter().any(|item| matches!(item, Item::Tool { call, .. } if call.state == ToolState::Succeeded && call.data.is_some()));
    let answer = snapshot.items.iter().any(|item| matches!(item, Item::Message(message) if message.id.as_str() == "assistant-1" && !message.text.is_empty()));
    if !success || !answer || session.context().and_then(ContextUsage::percent) != Some(12.5) {
        return Err("Scripted scenario did not produce the expected observed state".into());
    }
    Ok(events)
}
