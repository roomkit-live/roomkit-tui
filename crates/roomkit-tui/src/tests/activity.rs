#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]
use super::*;
use crate::UiOptions;
use crate::render::{self, Renderers};
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

mod preview;

fn call(id: &str, intent: Option<&str>, state: ToolState) -> Event {
    let Event::Item {
        conversation,
        item: Item::Tool {
            actor,
            run,
            mut call,
        },
    } = tool("run", state)
    else {
        unreachable!()
    };
    call.id = id.into();
    call.intent = intent.map(str::to_owned);
    Event::Item {
        conversation,
        item: Item::Tool { actor, run, call },
    }
}

fn draw(session: &Session, cols: u16) -> render::Frame {
    render::screen_with_options(
        session,
        cols,
        45,
        &Renderers::default(),
        &UiOptions {
            tool_activity: true,
            animations: false,
            ..Default::default()
        },
        Duration::ZERO,
    )
}

fn message(session: &mut Session, id: &str, text: &str) {
    session.apply(Event::TextDelta {
        conversation: "room-a".into(),
        message: id.into(),
        actor: "agent".into(),
        run: "run".into(),
        text: text.into(),
    });
}

#[test]
fn consecutive_calls_replace_the_visible_intent_without_erasing_history() {
    let mut session = session();
    session.apply(call(
        "one",
        Some("Looking for documents"),
        ToolState::Pending,
    ));
    assert!(
        draw(&session, 100)
            .lines
            .join("\n")
            .contains("Looking for documents")
    );
    session.apply(call(
        "one",
        Some("Looking for documents"),
        ToolState::Succeeded,
    ));
    session.apply(call(
        "two",
        Some("Reading the selected document"),
        ToolState::Pending,
    ));
    let frame = draw(&session, 100);
    let text = frame.lines.join("\n");
    assert!(text.contains("Reading the selected document"));
    assert!(!text.contains("Looking for documents"));
    assert!(!text.contains("unknown-domain-tool"));
    assert!(text.contains("Three records"));
    assert_eq!(frame.tool_targets.len(), 1);
    assert_eq!(session.snapshot().unwrap().items.len(), 2);
    session.expanded = true;
    let details = draw(&session, 100).lines.join("\n");
    assert_eq!(details.matches("unknown-domain-tool").count(), 2);
    assert!(details.contains("Three records"));
    session.expanded = false;
    assert_eq!(draw(&session, 100).tool_targets.len(), 1);
}

#[test]
fn streamed_messages_end_a_sequence_and_parallel_work_remains_visible() {
    let mut session = session();
    started(&mut session, "run");
    session.apply(call(
        "one",
        Some("Inspecting the document"),
        ToolState::Running,
    ));
    session.apply(call("two", Some("Checking references"), ToolState::Pending));
    let text = draw(&session, 100).lines.join("\n");
    assert!(text.contains("2 ongoing"));
    session.apply(call(
        "two",
        Some("Checking references"),
        ToolState::Succeeded,
    ));
    assert!(
        draw(&session, 100)
            .lines
            .join("\n")
            .contains("Inspecting the document")
    );
    session.apply(call(
        "one",
        Some("Inspecting the document"),
        ToolState::Succeeded,
    ));
    message(&mut session, "explanation", "Here is what I found.");
    session.apply(call(
        "three",
        Some("Preparing the summary"),
        ToolState::Pending,
    ));
    let frame = draw(&session, 100);
    let text = frame.lines.join("\n");
    assert_eq!(frame.tool_targets.len(), 2);
    assert!(
        text.find("Checking references").unwrap() < text.find("Here is what I found.").unwrap()
    );
    assert!(
        text.find("Here is what I found.").unwrap() < text.find("Preparing the summary").unwrap()
    );
}

#[test]
fn failures_refusals_and_cancellations_are_not_hidden_by_later_success() {
    let mut session = session();
    for (id, state) in [
        ("failed", ToolState::Failed),
        ("denied", ToolState::Denied),
        ("cancelled", ToolState::Cancelled),
        ("last", ToolState::Succeeded),
    ] {
        session.apply(call(id, Some("Processing the request"), state));
    }
    let text = draw(&session, 100).lines.join("\n");
    for label in ["1 failed", "1 declined", "1 cancelled"] {
        assert!(text.contains(label));
    }
    let narrow = draw(&session, 32);
    assert!(narrow.lines.join("\n").contains("failed"));
    assert!(narrow.lines.iter().all(|line| line.width() <= 32));
}

#[test]
fn groups_are_scoped_to_actors_and_runs_even_with_reused_call_ids() {
    let mut session = session();
    for (actor, run, intent) in [
        ("agent", "first", "First action"),
        ("other", "first", "Other participant"),
        ("agent", "second", "Another turn"),
    ] {
        let Event::Item {
            conversation,
            item: Item::Tool { call, .. },
        } = call("same", Some(intent), ToolState::Succeeded)
        else {
            unreachable!()
        };
        session.apply(Event::Item {
            conversation,
            item: Item::Tool {
                actor: actor.into(),
                run: run.into(),
                call,
            },
        });
    }
    let frame = draw(&session, 100);
    assert_eq!(frame.tool_targets.len(), 3);
    let text = frame.lines.join("\n");
    for intent in ["First action", "Other participant", "Another turn"] {
        assert!(text.contains(intent));
    }
}

#[test]
fn old_calls_use_tool_names_and_unicode_intents_stay_plain_and_bounded() {
    let mut session = session();
    let Event::Item {
        item: Item::Tool { call: original, .. },
        ..
    } = call("old", None, ToolState::Succeeded)
    else {
        unreachable!()
    };
    let mut wire = serde_json::to_value(&original).unwrap();
    wire.as_object_mut().unwrap().remove("intent");
    let restored: ToolCall = serde_json::from_value(wire).unwrap();
    assert_eq!(restored.intent, None);
    session.apply(call("old", None, ToolState::Succeeded));
    assert!(
        draw(&session, 100)
            .lines
            .join("\n")
            .contains("unknown-domain-tool")
    );
    let intent = format!("\x1b Recherche 文档 👩‍💻 {}", "é".repeat(250));
    session.apply(call("new", Some(&intent), ToolState::Pending));
    let frame = draw(&session, 48);
    assert!(
        frame
            .lines
            .iter()
            .all(|line| line.width() <= 48 && !line.contains('\x1b'))
    );
    assert!(frame.lines.join("\n").contains("Recherche 文档"));
    assert!(!frame.lines.join("\n").contains("unknown-domain-tool"));
    let snapshot = session.snapshot().unwrap();
    let restored: Snapshot =
        serde_json::from_str(&serde_json::to_string(snapshot).unwrap()).unwrap();
    let mut resumed = Session::default();
    resumed.apply(Event::Snapshot(restored));
    assert_eq!(draw(&resumed, 48).lines, frame.lines);
}

#[test]
fn missing_or_blank_intents_show_the_current_tool_with_its_real_state() {
    for intent in [None, Some(" \t ")] {
        for (state, symbol) in [
            (ToolState::Pending, "◇"),
            (ToolState::Running, "◌"),
            (ToolState::Succeeded, "✓"),
            (ToolState::Failed, "×"),
            (ToolState::Denied, "⊘"),
            (ToolState::Cancelled, "−"),
        ] {
            let mut session = session();
            session.apply(call(
                "first",
                Some("Earlier activity"),
                ToolState::Succeeded,
            ));
            session.apply(call("second", intent, state));
            let frame = draw(&session, 100);
            let text = frame.lines.join("\n");
            assert!(
                text.contains(&format!("{symbol} unknown-domain-tool")),
                "{text}"
            );
            assert!(!text.contains("Earlier activity"));
            assert!(!text.contains("Action completed"));
            assert!(text.contains("Three records"));
            assert_eq!(frame.tool_targets.len(), 1);
            assert_eq!(frame.tool_targets[0].call.as_str(), "second");
        }
    }
}

#[test]
fn approvals_are_visible_boundaries_between_activity_sequences() {
    let mut session = session();
    session.apply(call(
        "one",
        Some("Reading the document"),
        ToolState::Succeeded,
    ));
    session.apply(Event::Item {
        conversation: "room-a".into(),
        item: Item::Approval(PermissionDecision::new(
            Permission::new(
                "approval",
                "agent",
                "run",
                "Update the document",
                "Exact change",
            ),
            Decision::Allow,
            "person:alice",
        )),
    });
    session.apply(call(
        "two",
        Some("Updating the document"),
        ToolState::Succeeded,
    ));
    let frame = draw(&session, 100);
    assert_eq!(frame.tool_targets.len(), 2);
    let text = frame.lines.join("\n");
    assert!(text.contains("Update the document"));
    assert!(text.contains("Reading the document") && text.contains("Updating the document"));
}
