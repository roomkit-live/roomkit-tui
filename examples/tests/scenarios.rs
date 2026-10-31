//! The same interface supports two independent example applications.
#![allow(
    clippy::unwrap_used,
    reason = "Invalid example state must fail these tests"
)]

use roomkit_tui_examples::{Demo, Profile, check};
use roomkit_tui_protocol::*;

#[test]
fn both_applications_complete_the_same_generic_interaction_contract() {
    for profile in [Profile::Assistant, Profile::Orders] {
        let events = check(profile).unwrap();
        for event in &events {
            let wire = serde_json::to_string(event).unwrap();
            assert_eq!(serde_json::from_str::<Event>(&wire).unwrap(), *event);
        }
    }
}

#[test]
fn refusing_a_tool_never_schedules_its_execution() {
    let mut host = Demo::new(Profile::Orders);
    let events = host.handle(Request::new(
        RequestId::new(1),
        "test-operation-1",
        "example",
        Command::Submit {
            agent: "assistant".into(),
            text: "Inspect order 42".into(),
        },
    ));
    let permission = events
        .iter()
        .find_map(|event| match event {
            Event::PermissionRequested { permission, .. } => Some(permission.id.clone()),
            _ => None,
        })
        .unwrap();
    let events = host.handle(Request::new(
        RequestId::new(2),
        "test-operation-2",
        "example",
        Command::Decide {
            permission,
            decision: Decision::Deny,
        },
    ));
    assert!(events.iter().any(|event| matches!(event, Event::Item { item: Item::Tool { call, .. }, .. } if call.state == ToolState::Denied)));
    assert!(host.advance().is_none());
}

#[test]
fn an_unsupported_effort_is_rejected_instead_of_displayed_as_effective() {
    let mut host = Demo::new(Profile::Assistant);
    host.handle(Request::new(
        RequestId::new(1),
        "test-operation-1",
        "example",
        Command::Invoke {
            name: "model".into(),
            arguments: "fast".into(),
        },
    ));
    let events = host.handle(Request::new(
        RequestId::new(2),
        "test-operation-2",
        "example",
        Command::Invoke {
            name: "effort".into(),
            arguments: "high".into(),
        },
    ));
    assert!(matches!(
        events.as_slice(),
        [Event::Reply { error: Some(_), .. }]
    ));
}
