#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]
//! A host is trusted to own the conversation, not to keep the view bounded.

use roomkit_tui_protocol::*;

use super::snapshot;
use crate::Session;
use crate::render::{self, Renderers};

fn running() -> Session {
    let mut state = snapshot();
    state.active_run = Some("run".into());
    let mut session = Session::default();
    session.apply(Event::Snapshot(state));
    session
}

fn request(session: &mut Session, id: String, title: String) {
    let permission = Permission::new(id, "agent", "run", title, "detail");
    session.apply(Event::PermissionRequested {
        conversation: "room-a".into(),
        permission,
    });
}

/// Review finding 4: snapshots and permissions from a third-party host grew
/// the view without limit.
#[test]
fn oversized_host_input_is_refused_or_shortened() {
    let mut state = snapshot();
    state.title = "x".repeat(1 << 20);
    state.commands = (0..100_000)
        .map(|i| CommandSpec::new(format!("c{i}"), ""))
        .collect();
    let mut session = Session::default();
    session.apply(Event::Snapshot(state.clone()));
    assert!(
        session.snapshot().is_none(),
        "Too many commands are refused"
    );
    state.commands.truncate(10);
    session.apply(Event::Snapshot(state));
    assert!(
        session.snapshot().unwrap().title.len() <= 256,
        "Titles are shortened"
    );

    let mut session = running();
    for i in 0..1_000 {
        request(&mut session, format!("p{i}"), "t".into());
    }
    assert_eq!(session.permissions().len(), 64);
    let mut session = running();
    request(&mut session, "long".into(), "t".repeat(10_000));
    assert!(
        session.permissions().is_empty(),
        "An unreadable approval is not shown"
    );
    assert!(session.notice().contains("cannot display in full"));
}

/// Review finding 7: scrolling past the oldest line used to need as many
/// presses back before the view moved again.
#[test]
fn scrolling_is_bounded_by_the_history_shown() {
    let mut state = snapshot();
    state.items = (0..30)
        .map(|i| Item::Message(Message::new(format!("m{i}"), "agent", format!("line {i}"))))
        .collect();
    let mut session = Session::default();
    session.apply(Event::Snapshot(state));
    let renderers = Renderers::default();
    session.scroll_back(10_000);
    let top = render::screen(&session, 80, 20, &renderers);
    session.sync(&top, std::time::Duration::ZERO);
    assert!(
        session.scroll() < 100,
        "The offset is clamped to the history"
    );
    session.scroll_forward(8);
    let next = render::screen(&session, 80, 20, &renderers);
    assert_ne!(top.lines, next.lines, "One page forward moves the view");
}

/// Review finding 7: a choice field displayed its opaque value.
#[test]
fn choice_fields_show_labels_never_values() {
    let mut session = Session::default();
    let mut state = snapshot();
    state
        .commands
        .push(CommandSpec::new("configure", "Configure"));
    session.apply(Event::Snapshot(state));
    let choices = vec![Choice::new("opaque-7f3a", "Fast model")];
    let field =
        FormField::new("model", "Model", FieldKind::Choice { choices }).with_value("opaque-7f3a");
    session.apply(Event::Form {
        conversation: "room-a".into(),
        form: FormSpec::new("rev", "configure", "Configure", vec![field]),
    });
    let frame = render::screen(&session, 100, 30, &Renderers::default())
        .lines
        .join("\n");
    assert!(
        frame.contains("Fast model") && !frame.contains("opaque-7f3a"),
        "{frame}"
    );
}

/// A field kind from a newer host is refused rather than shown unprotected.
#[test]
fn unknown_items_and_field_kinds_from_a_newer_host_are_ignored() {
    let mut session = running();
    session.apply(Event::Item {
        conversation: "room-a".into(),
        item: Item::Unknown,
    });
    assert!(session.snapshot().unwrap().items.is_empty());
    session.apply(Event::Unknown);
    let field = FormField::new("when", "When", FieldKind::Unknown);
    session.apply(Event::Form {
        conversation: "room-a".into(),
        form: FormSpec::new("rev", "summarize", "Schedule", vec![field]),
    });
    assert!(session.form().is_none());
}

/// Review: the card cut its title at the first line break and at the screen
/// width, so part of what a person approved was hidden.
#[test]
fn an_approval_title_is_shown_in_full_or_not_at_all() {
    let mut session = running();
    let title = "Run the tests\n&& then remove every generated build directory recursively";
    let permission = Permission::new("p", "agent", "run", title, "detail");
    session.apply(Event::PermissionRequested {
        conversation: "room-a".into(),
        permission,
    });
    let frame = render::screen(&session, 40, 30, &Renderers::default());
    let text = frame.lines.join(" ");
    for word in title.split_whitespace() {
        assert!(text.contains(word), "{word} is missing: {text}");
    }
    assert_eq!(frame.approval.as_ref().map(|id| id.as_str()), Some("p"));
    let mut long = running();
    let permission = Permission::new("q", "agent", "run", "word ".repeat(50), "detail");
    long.apply(Event::PermissionRequested {
        conversation: "room-a".into(),
        permission,
    });
    let tiny = render::screen(&long, 32, 16, &Renderers::default());
    assert!(
        tiny.lines.join(" ").contains("Enlarge terminal"),
        "{:#?}",
        tiny.lines
    );
    assert!(tiny.approval.is_none() && tiny.permission_targets.is_empty());
}

/// Review: scrolling to the very top with `usize::MAX` overflowed the layout.
#[test]
fn scrolling_to_the_top_with_the_largest_offset_shows_the_oldest_items() {
    let mut state = snapshot();
    state.items = (0..30)
        .map(|i| Item::Message(Message::new(format!("m{i}"), "agent", format!("line {i}"))))
        .collect();
    let mut session = Session::default();
    session.apply(Event::Snapshot(state));
    session.scroll_back(usize::MAX);
    let frame = render::screen(&session, 80, 20, &Renderers::default());
    assert!(frame.lines.join("\n").contains("line 0"));
}

/// Review: a sidebar action could not be clicked while the sidebar had the
/// keyboard (after F8).
#[test]
fn sidebar_actions_can_be_clicked_while_the_sidebar_has_focus() {
    let mut session = Session::default();
    session.apply(Event::Snapshot(snapshot()));
    let action = SidebarAction::new("summarize", "report");
    let entry = SidebarEntry::new("Report").with_action(action);
    let sections = vec![SidebarSection::new(
        "Records",
        SidebarContent::Entries {
            entries: vec![entry],
        },
    )];
    session.apply(Event::SidebarChanged {
        conversation: "room-a".into(),
        sidebar: Some(SidebarSpec::new("Details", sections)),
    });
    session.sidebar.as_mut().unwrap().focused = true;
    let frame = render::screen_with_options(
        &session,
        140,
        40,
        &Renderers::default(),
        &crate::UiOptions::default(),
        std::time::Duration::ZERO,
    );
    let target = frame
        .action_targets
        .iter()
        .find(|t| t.sidebar)
        .expect("a visible sidebar action");
    assert!(session.action_command(target).is_some());
}
