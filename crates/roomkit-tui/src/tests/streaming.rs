#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]

use std::num::NonZeroUsize;
use std::sync::mpsc::{TryRecvError, TrySendError};

use roomkit_tui_protocol::*;

use super::*;
use crate::{Session, connection};

#[test]
fn streamed_text_is_reconciled_with_final_message_without_duplication() {
    let mut session = session();
    started(&mut session, "run-1");
    for text in ["Hello ", "world"] {
        session.apply(Event::TextDelta {
            conversation: "room-a".into(),
            message: "m-1".into(),
            actor: "agent".into(),
            run: "run-1".into(),
            text: text.into(),
        });
    }
    session.apply(Event::Item {
        conversation: "room-a".into(),
        item: Item::Message(Message::new("m-1", "agent", "Hello world!")),
    });
    assert_eq!(session.snapshot().unwrap().items.len(), 1);
    assert!(
        matches!(&session.snapshot().unwrap().items[0], Item::Message(m) if m.text == "Hello world!")
    );
}

#[test]
fn a_streamed_draft_takes_the_durable_message_identity_without_duplicate_content() {
    let mut session = session();
    started(&mut session, "run-1");
    session.apply(Event::TextDelta {
        conversation: "room-a".into(),
        message: "draft-1".into(),
        actor: "agent".into(),
        run: "run-1".into(),
        text: "Draft".into(),
    });
    let final_event = Event::MessageFinalized {
        conversation: "room-a".into(),
        draft: "draft-1".into(),
        message: Message::new("stored-1", "agent", "Reviewed answer"),
    };
    session.apply(final_event.clone());
    session.apply(final_event);
    let items = &session.snapshot().unwrap().items;
    assert_eq!(items.len(), 1);
    assert!(
        matches!(&items[0], Item::Message(m) if m.id.as_str() == "stored-1" && m.text == "Reviewed answer")
    );
}

#[test]
fn calls_are_scoped_to_runs_and_other_conversations_do_not_change_the_view() {
    let mut session = session();
    session.apply(tool("run-1", ToolState::Pending));
    session.apply(tool("run-1", ToolState::Succeeded));
    session.apply(tool("run-2", ToolState::Running));
    session.apply(Event::Item {
        conversation: "room-b".into(),
        item: Item::Message(Message::new("m", "agent", "Private in another room")),
    });
    let items = &session.snapshot().unwrap().items;
    assert_eq!(items.len(), 2);
    assert!(matches!(&items[0], Item::Tool { call, .. } if call.state == ToolState::Succeeded));
    assert!(matches!(&items[1], Item::Tool { call, .. } if call.state == ToolState::Running));
}

#[test]
fn accepted_interruption_does_not_claim_that_execution_has_stopped() {
    let mut session = session();
    started(&mut session, "run-1");
    let request = session
        .request(Command::Interrupt {
            run: "run-1".into(),
        })
        .unwrap();
    session.sent(&request);
    session.apply(Event::Reply {
        request: request.id,
        error: None,
    });
    assert!(session.interrupting());
    assert_eq!(session.active_run(), Some(&RunId::from("run-1")));
    session.apply(Event::RunFinished {
        conversation: "room-a".into(),
        run: "older-run".into(),
        outcome: RunOutcome::Completed,
    });
    assert!(session.active_run().is_some());
    session.apply(Event::RunFinished {
        conversation: "room-a".into(),
        run: "run-1".into(),
        outcome: RunOutcome::Interrupted,
    });
    assert!(session.active_run().is_none());
    assert!(!session.interrupting());
}

#[test]
fn bounded_transport_retains_rejected_events_and_reports_disconnects() {
    let (client, host) = connection(NonZeroUsize::MIN);
    let first = Event::Notice {
        conversation: "room-a".into(),
        text: "first".into(),
    };
    let second = Event::Notice {
        conversation: "room-a".into(),
        text: "second".into(),
    };
    host.try_send(first.clone()).unwrap();
    let error = host.try_send(second.clone()).unwrap_err();
    assert!(matches!(&error, TrySendError::Full(event) if **event == second));
    assert_eq!(client.try_recv().unwrap(), first);
    if let TrySendError::Full(event) = error {
        host.try_send(*event).unwrap();
    }
    assert_eq!(client.try_recv().unwrap(), second);
    drop(host);
    assert_eq!(client.try_recv(), Err(TryRecvError::Disconnected));
}

#[test]
fn projection_limits_do_not_split_utf8_or_keep_unbounded_history() {
    let mut session = Session::new(crate::Limits {
        items: NonZeroUsize::new(2).unwrap(),
        text_bytes: NonZeroUsize::new(32).unwrap(),
    });
    session.apply(Event::Snapshot(snapshot()));
    for i in 0..4 {
        session.apply(Event::Item {
            conversation: "room-a".into(),
            item: Item::Message(Message::new(
                MessageId::new(i.to_string()),
                "agent",
                "界".repeat(100),
            )),
        });
    }
    let items = &session.snapshot().unwrap().items;
    assert_eq!(items.len(), 2);
    assert!(
        matches!(&items[0], Item::Message(m) if m.id.as_str() == "2" && m.text.len() <= 32 && m.text.contains("truncated"))
    );
}
