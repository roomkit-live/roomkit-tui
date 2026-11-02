//! What a view shows of a call between its start and its end. RoomKit
//! records only those two rows; a round's calls execute once every decision
//! of the round is answered.

use super::*;
use std::collections::BTreeMap;

#[derive(Debug, PartialEq)]
enum Seen {
    Tool(String, ToolState),
    Requested,
    Resolved,
}

/// Run one turn whose round calls `auto1`, `ask` and `auto2`. `ask` waits
/// for `decision` when one is given; otherwise every call is allowed at once.
async fn round(decision: Option<Decision>) -> Vec<Seen> {
    let kit = RoomKit::new(Default::default());
    let provider = ScriptedProvider::new([
        ScriptedRound::calling(
            "Checking",
            [
                ("auto1", "lookup_record", "{}"),
                ("ask", "lookup_record", "{}"),
                ("auto2", "lookup_record", "{}"),
            ],
        ),
        ScriptedRound::answering("Done"),
    ]);
    let channel = prepare(&kit, provider, Arc::new(AtomicUsize::new(0)), false).await;
    let gate = channel.clone();
    kit.hook(
        roomkit::HookTrigger::BeforeToolUse,
        move |event: roomkit::ToolCallEvent, _| {
            let gate = gate.clone();
            async move {
                if decision.is_none() || event.tool_call_id.as_str() != "ask" {
                    return Ok(roomkit::model::HookResult::allow(Default::default()));
                }
                let request =
                    PermissionRequest::new("Read?", "Read a record", Duration::from_secs(30));
                Ok(match gate.ask_tool_permission(&event, request).await {
                    Decision::Allow => roomkit::model::HookResult::allow(Default::default()),
                    _ => roomkit::model::HookResult::block("Denied", Default::default()),
                })
            }
        },
        hook_options("permission"),
    )
    .unwrap();
    let (client, host) = connection(NonZeroUsize::new(128).unwrap());
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel.clone(), config()).run(host));
    next(&client).await;
    client.try_send(request("round")).unwrap();
    let mut seen = Vec::new();
    loop {
        match next(&client).await {
            Event::Item {
                item: Item::Tool { call, .. },
                ..
            } => seen.push(Seen::Tool(call.id.as_str().to_owned(), call.state)),
            Event::PermissionRequested { permission, .. } => {
                seen.push(Seen::Requested);
                let decide = Command::Decide {
                    permission: permission.id,
                    decision: decision.unwrap(),
                };
                let answer = Request::new(RequestId::new(2), "answer", "room", decide);
                client.try_send(answer).unwrap();
            }
            Event::PermissionResolved { .. } => seen.push(Seen::Resolved),
            Event::OperationFinished { .. } => break,
            _ => {}
        }
    }
    drop(client);
    task.await.unwrap().unwrap();
    seen
}

fn position(seen: &[Seen], marker: &Seen) -> usize {
    seen.iter().position(|s| s == marker).unwrap()
}

/// The state each call shows after the first `upto` events.
fn shown(seen: &[Seen], upto: usize) -> BTreeMap<&str, ToolState> {
    let mut states = BTreeMap::new();
    for event in &seen[..upto] {
        if let Seen::Tool(call, state) = event {
            states.insert(call.as_str(), *state);
        }
    }
    states
}

fn states_between(seen: &[Seen], from: usize, to: usize) -> Vec<(&str, ToolState)> {
    seen[from..to]
        .iter()
        .filter_map(|s| match s {
            Seen::Tool(call, state) => Some((call.as_str(), *state)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn calls_run_from_their_start_when_no_decision_holds_them() {
    let seen = round(None).await;
    for call in ["auto1", "ask", "auto2"] {
        let states: Vec<_> = seen
            .iter()
            .filter_map(|s| match s {
                Seen::Tool(id, state) if id == call => Some(*state),
                _ => None,
            })
            .collect();
        assert_eq!(
            states,
            [ToolState::Running, ToolState::Succeeded],
            "{call}: {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_round_waits_pending_for_its_decision_then_runs_once_allowed() {
    let seen = round(Some(Decision::Allow)).await;
    let requested = position(&seen, &Seen::Requested);
    let resolved = position(&seen, &Seen::Resolved);
    assert!(
        states_between(&seen, 0, requested)
            .iter()
            .all(|(_, state)| *state == ToolState::Running),
        "Before a decision is asked, started calls run: {seen:?}"
    );
    assert!(
        states_between(&seen, requested, resolved)
            .iter()
            .all(|(_, state)| *state == ToolState::Pending),
        "While the decision is unanswered, the round's calls wait: {seen:?}"
    );
    assert!(
        shown(&seen, resolved)
            .values()
            .all(|state| *state == ToolState::Pending),
        "No call keeps running while the round waits: {seen:?}"
    );
    for call in ["auto1", "ask", "auto2"] {
        let after: Vec<_> = states_between(&seen, resolved, seen.len())
            .into_iter()
            .filter(|(id, _)| *id == call)
            .map(|(_, state)| state)
            .collect();
        assert_eq!(
            after,
            [ToolState::Running, ToolState::Succeeded],
            "{call} runs once allowed, then ends: {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_decision_never_shows_its_round_running() {
    let seen = round(Some(Decision::Deny)).await;
    let requested = position(&seen, &Seen::Requested);
    assert!(
        states_between(&seen, requested, seen.len())
            .iter()
            .all(|(_, state)| *state != ToolState::Running),
        "A refusal never claims a call of its round runs: {seen:?}"
    );
    let last = shown(&seen, seen.len());
    assert_eq!(last["ask"], ToolState::Denied, "{seen:?}");
    assert_eq!(last["auto1"], ToolState::Succeeded, "{seen:?}");
    assert_eq!(last["auto2"], ToolState::Succeeded, "{seen:?}");
}
