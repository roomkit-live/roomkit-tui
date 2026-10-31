use roomkit_tui_protocol::{Permission, PermissionDecision, RunOutcome};
use serde_json::json;

use super::*;

fn progress() -> ToolProgress {
    let mut progress = ToolProgress::new("room".into());
    progress.apply(Event::RunStarted {
        conversation: "room".into(),
        run: "turn".into(),
    });
    progress
}

fn tool(run: &str, id: &str, state: ToolState) -> Event {
    Event::Item {
        conversation: "room".into(),
        item: Item::Tool {
            actor: "agent".into(),
            run: run.into(),
            call: ToolCall::new(id, "lookup", json!({}), state),
        },
    }
}

fn permission(id: &str) -> Permission {
    Permission::new(id, "agent", "turn", "Read?", "detail")
}

fn requested(id: &str) -> Event {
    Event::PermissionRequested {
        conversation: "room".into(),
        permission: permission(id),
    }
}

fn answered(id: &str, decision: Decision) -> Event {
    Event::Item {
        conversation: "room".into(),
        item: Item::Approval(PermissionDecision::new(
            permission(id),
            decision,
            "person:alice",
        )),
    }
}

fn resolved(id: &str) -> Event {
    Event::PermissionResolved {
        conversation: "room".into(),
        permission: id.into(),
    }
}

/// The (call, state) of each tool event among `events`.
fn tools(events: &[Event]) -> Vec<(String, ToolState)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Item {
                item: Item::Tool { call, .. },
                ..
            } => Some((call.id.as_str().to_owned(), call.state)),
            _ => None,
        })
        .collect()
}

fn shown(call: &str, state: ToolState) -> (String, ToolState) {
    (call.to_owned(), state)
}

#[test]
fn a_started_call_runs_only_within_a_running_turn() {
    let mut progress = progress();
    let events = progress.apply(tool("turn", "a", ToolState::Pending));
    assert_eq!(tools(&events), [shown("a", ToolState::Running)]);
    // A call of a turn that is not running, such as restored history.
    let events = progress.apply(tool("old", "b", ToolState::Pending));
    assert_eq!(tools(&events), [shown("b", ToolState::Pending)]);
    let events = progress.apply(tool("turn", "a", ToolState::Succeeded));
    assert_eq!(tools(&events), [shown("a", ToolState::Succeeded)]);
}

#[test]
fn a_round_waits_for_its_last_decision_then_runs() {
    let mut progress = progress();
    progress.apply(tool("turn", "a", ToolState::Pending));
    progress.apply(tool("turn", "b", ToolState::Pending));
    let events = progress.apply(requested("p1"));
    assert!(matches!(events[0], Event::PermissionRequested { .. }));
    assert_eq!(
        tools(&events),
        [
            shown("a", ToolState::Pending),
            shown("b", ToolState::Pending)
        ]
    );
    assert!(tools(&progress.apply(requested("p2"))).is_empty());
    let events = progress.apply(tool("turn", "c", ToolState::Pending));
    assert_eq!(tools(&events), [shown("c", ToolState::Pending)]);
    progress.apply(answered("p1", Decision::Allow));
    assert!(
        tools(&progress.apply(resolved("p1"))).is_empty(),
        "Another decision still holds the round"
    );
    progress.apply(answered("p2", Decision::Allow));
    let events = progress.apply(resolved("p2"));
    assert!(matches!(events[0], Event::PermissionResolved { .. }));
    assert_eq!(
        tools(&events),
        [
            shown("a", ToolState::Running),
            shown("b", ToolState::Running),
            shown("c", ToolState::Running)
        ]
    );
}

#[test]
fn a_refused_or_expired_decision_holds_only_its_round() {
    for answer in [Some(Decision::Deny), None] {
        let mut progress = progress();
        progress.apply(tool("turn", "a", ToolState::Pending));
        progress.apply(tool("turn", "b", ToolState::Pending));
        progress.apply(requested("p1"));
        if let Some(decision) = answer {
            progress.apply(answered("p1", decision));
        }
        assert!(tools(&progress.apply(resolved("p1"))).is_empty());
        progress.apply(tool("turn", "a", ToolState::Denied));
        let events = progress.apply(tool("turn", "b", ToolState::Succeeded));
        assert_eq!(tools(&events), [shown("b", ToolState::Succeeded)]);
        // The held round has ended; the turn's next round runs.
        let events = progress.apply(tool("turn", "c", ToolState::Pending));
        assert_eq!(tools(&events), [shown("c", ToolState::Running)]);
    }
}

#[test]
fn a_finished_turn_leaves_no_call_running() {
    let mut progress = progress();
    progress.apply(tool("turn", "a", ToolState::Pending));
    progress.apply(tool("turn", "b", ToolState::Pending));
    progress.apply(tool("turn", "b", ToolState::Succeeded));
    let events = progress.apply(Event::RunFinished {
        conversation: "room".into(),
        run: "turn".into(),
        outcome: RunOutcome::Interrupted,
    });
    assert_eq!(tools(&events), [shown("a", ToolState::Pending)]);
    assert!(matches!(events.last(), Some(Event::RunFinished { .. })));
    let events = progress.apply(tool("turn", "late", ToolState::Pending));
    assert_eq!(tools(&events), [shown("late", ToolState::Pending)]);
}
