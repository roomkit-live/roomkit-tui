//! Ported from feat/exit-warning: a host warning makes Ctrl+D ask first.
#![allow(clippy::unwrap_used, reason = "Test fixtures fail on invalid state")]
use super::*;
use roomkit_tui_protocol::{FieldKind, FormField, FormSpec, PanelSpec};

fn ctrl_d() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)
}

fn warn(session: &mut Session, text: Option<&str>) {
    session.apply(Event::ExitWarning {
        conversation: "room".into(),
        text: text.map(Into::into),
    });
}

/// The whole screen as the terminal draws it at `at`.
fn rendered(session: &Session, at: Duration) -> String {
    let frame = render::screen_with_options(
        session,
        120,
        30,
        &Renderers::default(),
        &UiOptions::default(),
        at,
    );
    frame.lines.join("\n")
}

fn open_panel(session: &mut Session) {
    session.apply(Event::Panel {
        conversation: "room".into(),
        panel: PanelSpec::new("processes", "action", "Processes", "server"),
    });
}

fn open_form(session: &mut Session) {
    session.apply(Event::Form {
        conversation: "room".into(),
        form: FormSpec::new(
            "settings",
            "action",
            "Settings",
            vec![FormField::new("label", "Label", FieldKind::Text)],
        ),
    });
}

#[test]
fn ctrl_d_detaches_at_once_without_an_exit_warning() {
    let (mut session, client, _host) = fixture(false);
    assert!(press(&mut session, &client, ctrl_d(), Duration::ZERO));
    warn(&mut session, Some("1 running process will be stopped"));
    warn(&mut session, None);
    assert!(press(&mut session, &client, ctrl_d(), Duration::ZERO));
}

#[test]
fn exit_warning_makes_ctrl_d_ask_once_and_names_what_exiting_stops() {
    let (mut session, client, _host) = fixture(false);
    session.insert("Draft");
    warn(&mut session, Some("1 running process will be stopped"));
    assert!(!press(&mut session, &client, ctrl_d(), Duration::ZERO));
    assert!(
        rendered(&session, Duration::ZERO)
            .contains("Press Ctrl+D again to exit · 1 running process will be stopped")
    );
    assert_eq!(session.draft(), "Draft", "Detaching keeps the draft");
    let mut held = ctrl_d();
    held.kind = KeyEventKind::Repeat;
    assert!(
        !press(&mut session, &client, held, Duration::ZERO),
        "A held key must not confirm"
    );
    assert!(press(
        &mut session,
        &client,
        ctrl_d(),
        Duration::from_millis(500)
    ));
}

#[test]
fn armed_exit_expires_typing_disarms_and_ctrl_c_also_confirms() {
    let (mut session, client, _host) = fixture(false);
    warn(&mut session, Some("2 running processes will be stopped"));
    assert!(!press(&mut session, &client, ctrl_d(), Duration::ZERO));
    // After the confirmation window, Ctrl+D asks again instead of leaving.
    assert!(!press(&mut session, &client, ctrl_d(), QUIT_TIMEOUT));
    assert!(!press(
        &mut session,
        &client,
        key(KeyCode::Char('x')),
        QUIT_TIMEOUT
    ));
    assert!(!press(&mut session, &client, ctrl_d(), QUIT_TIMEOUT));
    assert!(press(&mut session, &client, ctrl_c(), QUIT_TIMEOUT));
    // Ctrl+C arms with its own key name and the same warning.
    let (mut session, client, _host) = fixture(false);
    warn(&mut session, Some("2 running processes will be stopped"));
    assert!(!press(&mut session, &client, ctrl_c(), Duration::ZERO));
    assert!(
        rendered(&session, Duration::ZERO)
            .contains("Press Ctrl+C again to exit · 2 running processes will be stopped")
    );
}

#[test]
fn an_open_picker_or_permission_cannot_skip_the_exit_confirmation() {
    let (mut session, client, _host) = fixture(false);
    warn(&mut session, Some("1 running process will be stopped"));
    session.picker = Some(crate::ChoicePicker::new(
        "action".into(),
        "Choices".into(),
        vec![],
    ));
    assert!(!press(&mut session, &client, ctrl_d(), Duration::ZERO));
    assert!(press(&mut session, &client, ctrl_d(), Duration::ZERO));
    let (mut session, client, _host) = fixture(true);
    warn(&mut session, Some("1 running process will be stopped"));
    request_permission(&mut session, "p", Duration::ZERO);
    assert!(!press(&mut session, &client, ctrl_d(), Duration::ZERO));
    assert!(press(&mut session, &client, ctrl_d(), Duration::ZERO));
}

fn open_approval(session: &mut Session) {
    request_permission(session, "p", Duration::ZERO);
}

/// Arm the exit with Ctrl+D over the screen `open` shows, whose last help
/// line contains `help`: the hint replaces that line until the exit expires.
fn assert_armed_exit_is_shown(name: &str, open: fn(&mut Session), help: &str) {
    let hint = "Press Ctrl+D again to exit · 1 running process will be stopped";
    let (mut session, client, _host) = fixture(true);
    warn(&mut session, Some("1 running process will be stopped"));
    open(&mut session);
    assert!(!rendered(&session, Duration::ZERO).contains(hint), "{name}");
    assert!(!press(&mut session, &client, ctrl_d(), Duration::ZERO));
    assert!(
        rendered(&session, Duration::ZERO).contains(hint),
        "The {name} hides the armed exit"
    );
    let expired = rendered(&session, QUIT_TIMEOUT);
    assert!(!expired.contains(hint), "The {name} keeps an expired hint");
    assert!(expired.contains(help), "The {name} loses its help");
}

/// Review finding: Ctrl+D arms the exit whichever screen has focus, but only
/// the conversation footer showed the hint, so a second press from a panel, a
/// form or an approval card stopped the host's work unannounced.
#[test]
fn an_armed_exit_is_shown_over_a_panel_a_form_and_an_approval_card() {
    assert_armed_exit_is_shown("panel", open_panel, "Esc close");
    assert_armed_exit_is_shown("form", open_form, "Ctrl+S save");
    assert_armed_exit_is_shown("approval", open_approval, "Esc interrupt");
}

#[test]
fn exit_warning_is_scoped_bounded_and_cleared_by_a_new_view() {
    let (mut session, _client, _host) = fixture(false);
    let snapshot = session.snapshot().cloned().unwrap();
    warn(&mut session, Some("1 running process will be stopped"));
    session.apply(Event::ExitWarning {
        conversation: "other".into(),
        text: None,
    });
    warn(&mut session, Some(&"x".repeat(257)));
    assert_eq!(
        session.exit_warning(),
        Some("1 running process will be stopped")
    );
    session.apply(Event::Snapshot(snapshot));
    assert!(session.exit_warning().is_none());
}
