//! RMK-1285: Ctrl+S, or Ctrl+Enter, sends the draft now. During a turn it
//! cuts the turn, as Esc does, and the draft leaves once the turn stopped.

#![allow(clippy::unwrap_used, reason = "Test fixtures fail on invalid state")]
use super::*;
use roomkit_tui_protocol::Rejection;

const DRAFT: &str = "Use the second ledger";

fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

fn ctrl_s(session: &mut Session, client: &Client) {
    press(session, client, ctrl(KeyCode::Char('s')), Duration::ZERO);
}

/// A view on an operation it started, whose run has not begun yet.
fn working() -> (Session, Client, Host) {
    let (mut session, client, host) = fixture(false);
    session.apply(Event::OperationStarted {
        conversation: "room".into(),
        operation: "op".into(),
    });
    (session, client, host)
}

fn started(session: &mut Session) {
    session.apply(Event::RunStarted {
        conversation: "room".into(),
        run: "run".into(),
        answers: None,
    });
}

fn stopped(session: &mut Session) {
    session.apply(Event::RunFinished {
        conversation: "room".into(),
        run: "run".into(),
        outcome: RunOutcome::Interrupted,
    });
}

fn finished(session: &mut Session) {
    session.apply(Event::OperationFinished {
        conversation: "room".into(),
        operation: "op".into(),
        outcome: RunOutcome::Interrupted,
    });
}

fn submitted(text: &str) -> Command {
    Command::Submit {
        agent: "agent".into(),
        text: text.into(),
    }
}

fn interrupt() -> Command {
    Command::Interrupt { run: "run".into() }
}

#[test]
fn ctrl_s_during_a_turn_cuts_it_and_sends_the_draft_once_it_stopped() {
    for send_now in [ctrl(KeyCode::Char('s')), ctrl(KeyCode::Enter)] {
        let (mut session, client, host) = working();
        started(&mut session);
        session.insert(DRAFT);
        assert!(!press(&mut session, &client, send_now, Duration::ZERO));
        assert_eq!(host.try_recv().unwrap().command, interrupt());
        assert_eq!(session.draft(), DRAFT);
        assert!(session.sends_after_interruption());

        // The run ended, but its operation still holds the room.
        stopped(&mut session);
        assert!(!composer::send_waiting(&mut session, &client));
        assert!(host.try_recv().is_err());

        finished(&mut session);
        assert!(composer::send_waiting(&mut session, &client));
        assert_eq!(host.try_recv().unwrap().command, submitted(DRAFT));
        assert!(session.draft().is_empty());
        assert!(!session.sends_after_interruption());
        assert!(!composer::send_waiting(&mut session, &client));
        assert!(host.try_recv().is_err(), "The draft left twice");
    }
}

#[test]
fn a_run_that_starts_after_ctrl_s_is_cut_too() {
    let (mut session, client, host) = working();
    session.insert(DRAFT);
    ctrl_s(&mut session, &client);
    assert!(host.try_recv().is_err(), "No run to cut yet");
    assert!(session.sends_after_interruption());

    started(&mut session);
    assert!(composer::send_waiting(&mut session, &client));
    assert_eq!(host.try_recv().unwrap().command, interrupt());
    assert!(!composer::send_waiting(&mut session, &client));
    assert!(host.try_recv().is_err(), "The run was cut twice");

    stopped(&mut session);
    finished(&mut session);
    assert!(composer::send_waiting(&mut session, &client));
    assert_eq!(host.try_recv().unwrap().command, submitted(DRAFT));
}

#[test]
fn escape_keeps_a_draft_that_waited_for_the_interruption() {
    let (mut session, client, host) = fixture(true);
    session.insert("Second thoughts");
    ctrl_s(&mut session, &client);
    assert_eq!(host.try_recv().unwrap().command, interrupt());
    press(&mut session, &client, escape(), Duration::ZERO);
    assert!(!session.sends_after_interruption());
    assert!(host.try_recv().is_err(), "Esc sent another request");
    stopped(&mut session);
    assert!(!composer::send_waiting(&mut session, &client));
    assert!(host.try_recv().is_err(), "The kept draft was sent");
    assert_eq!(session.draft(), "Second thoughts");
}

/// The host refuses a run it cannot stop, or one that ended as Ctrl+S was
/// pressed: the draft then leaves once the run ends, and the run is not
/// asked again.
#[test]
fn a_refused_interruption_lets_the_draft_wait_for_the_end() {
    let (mut session, client, host) = fixture(true);
    session.insert(DRAFT);
    ctrl_s(&mut session, &client);
    let interruption = host.try_recv().unwrap();
    session.apply(Event::Reply {
        request: interruption.id,
        error: Some(Rejection::refused("This execution cannot be interrupted")),
    });
    assert_eq!(session.notice(), "This execution cannot be interrupted");
    assert!(session.sends_after_interruption());
    assert!(!composer::send_waiting(&mut session, &client));
    assert!(host.try_recv().is_err(), "The refused run was asked again");
    stopped(&mut session);
    assert!(composer::send_waiting(&mut session, &client));
    assert_eq!(host.try_recv().unwrap().command, submitted(DRAFT));
}

/// Review finding: an edited draft, a command among them, left by itself
/// once the turn stopped.
#[test]
fn an_edited_draft_never_leaves_unseen() {
    for edited in ["!rm -rf build", "/action", "Use the second ledger, then"] {
        let (mut session, client, host) = fixture(true);
        session.insert(DRAFT);
        ctrl_s(&mut session, &client);
        assert_eq!(host.try_recv().unwrap().command, interrupt());
        session.set_draft(edited);
        assert!(!session.sends_after_interruption());
        stopped(&mut session);
        assert!(!composer::send_waiting(&mut session, &client));
        assert!(host.try_recv().is_err(), "{edited:?} left unseen");
        assert_eq!(session.draft(), edited);
    }
}

/// Light review: the same number of images, another image, left too.
#[test]
fn a_draft_with_other_images_never_leaves_unseen() {
    let (mut session, client, host) = fixture(true);
    let mut snapshot = Snapshot::new("room", "Example", Actor::new("agent", "Agent"));
    snapshot.active_run = Some("run".into());
    snapshot.can_interrupt = true;
    snapshot.can_attach_images = true;
    session.apply(Event::Snapshot(snapshot));
    session.insert(DRAFT);
    session.add_image(crate::tests::image_fixture()).unwrap();
    ctrl_s(&mut session, &client);
    assert_eq!(host.try_recv().unwrap().command, interrupt());
    session.remove_last_image();
    let mut other = crate::tests::image_fixture();
    other.name = "other.png".into();
    session.add_image(other).unwrap();
    assert!(!session.sends_after_interruption());
    stopped(&mut session);
    assert!(!composer::send_waiting(&mut session, &client));
    assert!(host.try_recv().is_err(), "Another image left unseen");
}

/// Light review: the refusal of the first run's interruption, arriving
/// once a second run was asked to stop, cleared that second interruption.
#[test]
fn a_late_refusal_leaves_the_next_interruption_pending() {
    let (mut session, client, host) = working();
    started(&mut session);
    session.insert(DRAFT);
    ctrl_s(&mut session, &client);
    let first = host.try_recv().unwrap();
    stopped(&mut session);
    session.apply(Event::RunStarted {
        conversation: "room".into(),
        run: "next".into(),
        answers: None,
    });
    assert!(composer::send_waiting(&mut session, &client));
    assert_eq!(
        host.try_recv().unwrap().command,
        Command::Interrupt { run: "next".into() }
    );
    session.apply(Event::Reply {
        request: first.id,
        error: Some(Rejection::refused("This execution has already ended")),
    });
    assert!(
        session.interrupting(),
        "The second interruption was forgotten"
    );
}

#[test]
fn ctrl_c_or_a_conversation_switch_sends_nothing() {
    let (mut session, client, host) = fixture(true);
    session.insert(DRAFT);
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    press(&mut session, &client, ctrl_c(), Duration::ZERO);
    session.insert(DRAFT);
    stopped(&mut session);
    assert!(!composer::send_waiting(&mut session, &client));
    assert!(host.try_recv().is_err(), "A draft typed after Ctrl+C left");

    let (mut session, client, host) = fixture(true);
    session.insert(DRAFT);
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    let other = Snapshot::new("other", "Other", Actor::new("agent", "Agent"));
    session.apply(Event::Snapshot(other));
    assert!(!composer::send_waiting(&mut session, &client));
    assert!(
        host.try_recv().is_err(),
        "The draft went to another conversation"
    );
    assert_eq!(session.draft(), DRAFT);
}

/// Review finding: the host may still hold the room after the view saw
/// the run end, as for a steered message's own turn; refused as busy, the
/// draft comes back to the composer instead of only to its history.
#[test]
fn a_waited_draft_refused_as_busy_comes_back() {
    let (mut session, client, host) = fixture(true);
    session.insert(DRAFT);
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    stopped(&mut session);
    assert!(composer::send_waiting(&mut session, &client));
    let sent = host.try_recv().unwrap();
    assert_eq!(sent.command, submitted(DRAFT));
    assert!(session.draft().is_empty());
    session.apply(Event::Reply {
        request: sent.id,
        error: Some(Rejection::busy("A request is still executing")),
    });
    assert_eq!(session.draft(), DRAFT);
}

#[test]
fn ctrl_s_is_enter_without_a_turn_or_for_a_command() {
    let (mut session, client, host) = fixture(false);
    session.insert("Hello");
    ctrl_s(&mut session, &client);
    assert_eq!(host.try_recv().unwrap().command, submitted("Hello"));

    let (mut session, client, host) = fixture(true);
    session.insert("/action");
    session.dismiss_completions();
    ctrl_s(&mut session, &client);
    assert!(matches!(
        host.try_recv().unwrap().command,
        Command::Invoke { .. }
    ));
    assert!(!session.sends_after_interruption());
}

#[test]
fn ctrl_s_never_waits_on_a_turn_it_cannot_cut_or_with_nothing_to_send() {
    let (mut session, client, host) = fixture(true);
    ctrl_s(&mut session, &client);
    assert!(host.try_recv().is_err());
    assert!(!session.sends_after_interruption());

    let mut snapshot = Snapshot::new("room", "Example", Actor::new("agent", "Agent"));
    snapshot.active_run = Some("run".into());
    snapshot.can_interrupt = false;
    session.apply(Event::Snapshot(snapshot));
    session.insert("Now");
    ctrl_s(&mut session, &client);
    assert!(host.try_recv().is_err());
    assert!(!session.sends_after_interruption());
    assert_eq!(
        session.notice(),
        "This execution cannot be interrupted through this connection"
    );
}

#[test]
fn the_hint_names_the_key_and_the_wait() {
    let (mut session, client, _host) = fixture(true);
    let hints = |session: &Session, cols| {
        let options = UiOptions::default();
        let renderers = Renderers::default();
        render::screen_with_options(session, cols, 30, &renderers, &options, Duration::ZERO)
            .lines
            .join("\n")
    };
    assert!(hints(&session, 120).contains("Ctrl+S send now"));
    let narrow = hints(&session, 60);
    assert!(narrow.contains("Ctrl+S send now") && !narrow.contains("Alt+Enter"));
    session.insert("Now");
    ctrl_s(&mut session, &client);
    assert!(hints(&session, 120).contains("Sends once the turn stops"));
}
