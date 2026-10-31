#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]

use roomkit_tui_protocol::*;

use super::*;
use crate::Session;
use crate::render::{self, Renderers};

#[test]
fn permission_answers_wait_for_settlement_and_rejected_answers_can_be_retried() {
    let mut session = session();
    started(&mut session, "run-1");
    session.apply(Event::PermissionRequested {
        conversation: "room-a".into(),
        permission: Permission::new("decision", "agent", "run-1", "Read a record", "Record 42"),
    });
    let command = session.decide(Decision::Deny).unwrap();
    let request = session.request(command.clone()).unwrap();
    session.sent(&request);
    assert!(session.request(command.clone()).is_err());
    assert_eq!(session.permissions().len(), 1);
    session.apply(Event::Reply {
        request: request.id,
        error: Some("Temporary policy service error".into()),
    });
    let retry = session.request(command).unwrap();
    session.sent(&retry);
    session.apply(Event::Reply {
        request: retry.id,
        error: None,
    });
    assert!(session.deciding(&PermissionId::from("decision")));
    session.apply(Event::PermissionResolved {
        conversation: "room-a".into(),
        permission: "decision".into(),
    });
    assert!(session.permissions().is_empty());
}

#[test]
fn restored_permissions_remain_readable_and_can_be_settled() {
    let mut state = snapshot();
    state.active_run = Some("run-1".into());
    state.permissions.push(Permission::new("restored", "agent", "run-1", "Review a record", "Scope\nRecord 42\nRead only\nNo external message\nDetail 5\nDetail 6\nDetail 7\nDetail 8\nDetail 9\nDetail 10\nEnd of detail"));
    let mut session = Session::default();
    session.apply(Event::Snapshot(state));
    let renderers = Renderers::default();
    assert!(render::screen(&session, 12, 8, &renderers).cursor.is_none());
    let first = render::screen(&session, 80, 24, &renderers);
    assert!(first.lines.iter().any(|line| line.contains("Scope")));
    assert!(
        !first
            .lines
            .iter()
            .any(|line| line.contains("End of detail"))
    );
    session.permission_scroll = 3;
    let last = render::screen(&session, 80, 24, &renderers);
    assert!(last.lines.iter().any(|line| line.contains("End of detail")));
    let command = session.decide(Decision::Deny).unwrap();
    let request = session.request(command).unwrap();
    session.sent(&request);
    session.apply(Event::PermissionResolved {
        conversation: "room-a".into(),
        permission: "restored".into(),
    });
    assert!(session.permissions().is_empty());
    assert!(session.snapshot().unwrap().permissions.is_empty());
    assert_eq!(session.permission_scroll, 0);
}

#[test]
fn approval_card_and_host_receipt_preserve_draft_and_exact_click_scope() {
    let mut state = snapshot();
    state.active_run = Some("run-1".into());
    let permission = Permission::new(
        "review",
        "agent",
        "run-1",
        "Run this command?",
        "Local environment\n\n$ cargo test\n\nNo external service",
    )
    .with_summary("cargo test");
    state.permissions.push(permission.clone());
    let mut session = Session::default();
    session.apply(Event::Snapshot(state));
    session.set_draft("Preserved draft");
    let frame = render::screen(&session, 100, 30, &Renderers::default());
    assert!(frame.lines.iter().any(|l| l.contains("1. Allow once")));
    assert!(frame.lines.iter().any(|l| l.contains("2. Deny")));
    assert_eq!(frame.permission_targets.len(), 2);
    assert!(
        frame
            .permission_targets
            .iter()
            .all(|t| t.permission == permission.id && t.conversation.as_str() == "room-a")
    );
    assert!(!frame.lines.iter().any(|l| l.contains("Preserved draft")));
    assert!(
        !session
            .snapshot
            .as_ref()
            .unwrap()
            .items
            .iter()
            .any(|i| matches!(i, Item::Approval(_)))
    );
    let request = session
        .request(session.decide(Decision::Allow).unwrap())
        .unwrap();
    session.sent(&request);
    assert!(
        render::screen(&session, 100, 30, &Renderers::default())
            .permission_targets
            .is_empty()
    );
    session.apply(Event::PermissionResolved {
        conversation: "room-a".into(),
        permission: permission.id.clone(),
    });
    assert!(
        !session
            .snapshot
            .as_ref()
            .unwrap()
            .items
            .iter()
            .any(|i| matches!(i, Item::Approval(_))),
        "Settlement alone is not a human decision"
    );
    let receipt = Event::Item {
        conversation: "room-a".into(),
        item: Item::Approval(PermissionDecision::new(
            permission,
            Decision::Allow,
            "person:alice",
        )),
    };
    session.apply(receipt.clone());
    session.apply(receipt);
    assert_eq!(
        session
            .snapshot
            .as_ref()
            .unwrap()
            .items
            .iter()
            .filter(|i| matches!(i, Item::Approval(_)))
            .count(),
        1
    );
    assert_eq!(session.draft(), "Preserved draft");
    let frame = render::screen(&session, 100, 30, &Renderers::default());
    assert!(
        frame
            .lines
            .iter()
            .any(|l| l.contains("approved") && l.contains("cargo test") && l.contains("this time"))
    );
}
