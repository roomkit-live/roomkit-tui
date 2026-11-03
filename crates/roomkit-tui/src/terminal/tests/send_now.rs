//! RMK-1285: Ctrl+S, or Ctrl+Enter, sends the draft now. During a turn it
//! cuts the turn, as Esc does, and the draft leaves once the turn stopped.

#![allow(clippy::unwrap_used, reason = "Test fixtures fail on invalid state")]
use super::*;
use roomkit_tui_protocol::{Rejection, Request};

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
        assert!(!composer::send_waiting(
            &mut session,
            &client,
            Duration::ZERO
        ));
        assert!(host.try_recv().is_err());

        finished(&mut session);
        assert!(composer::send_waiting(
            &mut session,
            &client,
            Duration::ZERO
        ));
        assert_eq!(host.try_recv().unwrap().command, submitted(DRAFT));
        assert!(session.draft().is_empty());
        assert!(!session.sends_after_interruption());
        assert!(!composer::send_waiting(
            &mut session,
            &client,
            Duration::ZERO
        ));
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
    assert!(composer::send_waiting(
        &mut session,
        &client,
        Duration::ZERO
    ));
    assert_eq!(host.try_recv().unwrap().command, interrupt());
    assert!(!composer::send_waiting(
        &mut session,
        &client,
        Duration::ZERO
    ));
    assert!(host.try_recv().is_err(), "The run was cut twice");

    stopped(&mut session);
    finished(&mut session);
    assert!(composer::send_waiting(
        &mut session,
        &client,
        Duration::ZERO
    ));
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
    assert!(!composer::send_waiting(
        &mut session,
        &client,
        Duration::ZERO
    ));
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
    assert!(!composer::send_waiting(
        &mut session,
        &client,
        Duration::ZERO
    ));
    assert!(host.try_recv().is_err(), "The refused run was asked again");
    stopped(&mut session);
    assert!(composer::send_waiting(
        &mut session,
        &client,
        Duration::ZERO
    ));
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
        assert!(!composer::send_waiting(
            &mut session,
            &client,
            Duration::ZERO
        ));
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
    assert!(!composer::send_waiting(
        &mut session,
        &client,
        Duration::ZERO
    ));
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
    assert!(composer::send_waiting(
        &mut session,
        &client,
        Duration::ZERO
    ));
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
    assert!(!composer::send_waiting(
        &mut session,
        &client,
        Duration::ZERO
    ));
    assert!(host.try_recv().is_err(), "A draft typed after Ctrl+C left");

    let (mut session, client, host) = fixture(true);
    session.insert(DRAFT);
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    let other = Snapshot::new("other", "Other", Actor::new("agent", "Agent"));
    session.apply(Event::Snapshot(other));
    assert!(!composer::send_waiting(
        &mut session,
        &client,
        Duration::ZERO
    ));
    assert!(
        host.try_recv().is_err(),
        "The draft went to another conversation"
    );
    assert_eq!(session.draft(), DRAFT);
}

/// A draft sent at `at` once its run stopped: the request it left as.
fn left(session: &mut Session, client: &Client, host: &Host, at: Duration) -> Request {
    assert!(composer::send_waiting(session, client, at));
    let sent = host.try_recv().unwrap();
    assert_eq!(sent.command, submitted(DRAFT));
    assert!(session.draft().is_empty());
    sent
}

fn refused(session: &mut Session, sent: &Request, error: Rejection, at: Duration) {
    let reply = Event::Reply {
        request: sent.id,
        error: Some(error),
    };
    session.apply_at(reply, at);
}

fn busy() -> Rejection {
    Rejection::busy("A request is still executing")
}

/// RMK-1303: the host may hold the room with no run to show, as for the own
/// turn of a message sent earlier. A draft it refuses as busy comes back,
/// goes on waiting, and leaves once the host takes it.
#[test]
fn a_draft_refused_as_busy_waits_and_is_tried_again() {
    let (mut session, client, host) = fixture(true);
    session.insert(DRAFT);
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    stopped(&mut session);

    let at = Duration::from_secs(1);
    let sent = left(&mut session, &client, &host, at);
    refused(&mut session, &sent, busy(), at);
    assert_eq!(session.draft(), DRAFT);
    assert!(session.sends_after_interruption());
    assert!(
        session.notice().is_empty(),
        "A retry is no refusal to report"
    );
    assert!(!composer::send_waiting(&mut session, &client, at));
    assert!(host.try_recv().is_err(), "Tried again at once");

    let later = at + Duration::from_millis(200);
    let again = left(&mut session, &client, &host, later);
    session.apply_at(
        Event::Reply {
            request: again.id,
            error: None,
        },
        later,
    );
    assert!(session.draft().is_empty());
    assert!(!session.sends_after_interruption());
    assert!(!composer::send_waiting(&mut session, &client, later * 2));
    assert!(host.try_recv().is_err(), "The draft left twice");
}

/// RMK-1303: Ctrl+S cuts everything, whichever way the events come: the own
/// turn of a message sent earlier, starting after a busy refusal, is cut too.
#[test]
fn a_turn_that_starts_after_a_busy_refusal_is_cut_too() {
    let (mut session, client, host) = fixture(true);
    session.insert(DRAFT);
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    stopped(&mut session);
    let sent = left(&mut session, &client, &host, Duration::ZERO);
    refused(&mut session, &sent, busy(), Duration::ZERO);

    session.apply(Event::RunStarted {
        conversation: "room".into(),
        run: "own".into(),
        answers: None,
    });
    let later = Duration::from_secs(1);
    assert!(composer::send_waiting(&mut session, &client, later));
    assert_eq!(
        host.try_recv().unwrap().command,
        Command::Interrupt { run: "own".into() }
    );
    session.apply(Event::RunFinished {
        conversation: "room".into(),
        run: "own".into(),
        outcome: RunOutcome::Interrupted,
    });
    left(&mut session, &client, &host, later);
}

/// A host that keeps refusing is not asked for ever: the draft stays in the
/// composer, with its reason.
#[test]
fn a_host_that_stays_busy_leaves_the_draft_in_the_composer() {
    let (mut session, client, host) = fixture(true);
    session.insert(DRAFT);
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    stopped(&mut session);
    let sent = left(&mut session, &client, &host, Duration::ZERO);
    refused(&mut session, &sent, busy(), Duration::ZERO);

    let past = Duration::from_secs(30);
    let sent = left(&mut session, &client, &host, past);
    refused(&mut session, &sent, busy(), past);
    assert_eq!(session.draft(), DRAFT);
    assert!(!session.sends_after_interruption());
    assert_eq!(session.notice(), "A request is still executing");
    assert!(!composer::send_waiting(&mut session, &client, past * 2));
    assert!(host.try_recv().is_err(), "The host was asked again");
}

#[test]
fn another_refusal_brings_the_draft_back_without_trying_again() {
    let (mut session, client, host) = fixture(true);
    session.insert(DRAFT);
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    stopped(&mut session);
    let sent = left(&mut session, &client, &host, Duration::ZERO);
    refused(
        &mut session,
        &sent,
        Rejection::refused("Not allowed"),
        Duration::ZERO,
    );
    assert_eq!(session.draft(), DRAFT);
    assert!(!session.sends_after_interruption());
    assert_eq!(session.notice(), "Not allowed");
    assert!(!composer::send_waiting(
        &mut session,
        &client,
        Duration::from_secs(1)
    ));
    assert!(host.try_recv().is_err());
}

/// A busy refusal of a draft on its way to a conversation the view left
/// must not start sending it to the new one.
#[test]
fn a_conversation_switch_forgets_a_draft_on_its_way() {
    let (mut session, client, host) = fixture(true);
    session.insert(DRAFT);
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    stopped(&mut session);
    let sent = left(&mut session, &client, &host, Duration::ZERO);
    let other = Snapshot::new("other", "Other", Actor::new("agent", "Agent"));
    session.apply(Event::Snapshot(other));
    refused(&mut session, &sent, busy(), Duration::ZERO);
    assert!(!session.sends_after_interruption());
    assert!(!composer::send_waiting(
        &mut session,
        &client,
        Duration::from_secs(1)
    ));
    assert!(
        host.try_recv().is_err(),
        "The draft went to another conversation"
    );
}

/// Ctrl+C means the person dropped the draft, even one already on its way:
/// a busy refusal must not bring it back and send it again.
#[test]
fn ctrl_c_forgets_a_draft_on_its_way() {
    let (mut session, client, host) = fixture(true);
    session.insert(DRAFT);
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    stopped(&mut session);
    let sent = left(&mut session, &client, &host, Duration::ZERO);
    press(&mut session, &client, ctrl_c(), Duration::ZERO);
    refused(&mut session, &sent, busy(), Duration::ZERO);
    assert!(session.draft().is_empty(), "The dropped draft came back");
    assert!(!session.sends_after_interruption());
    let later = Duration::from_secs(1);
    assert!(!composer::send_waiting(&mut session, &client, later));
    assert!(host.try_recv().is_err(), "The dropped draft left again");
}

/// A message awaiting its reply would refuse the draft locally and end the
/// wait: the draft waits for that reply too.
#[test]
fn a_message_awaiting_its_reply_holds_the_draft() {
    let (mut session, client, host) = fixture(true);
    let earlier = session.request(submitted("Earlier")).unwrap();
    session.sent(&earlier);
    session.insert(DRAFT);
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    stopped(&mut session);
    assert!(!composer::send_waiting(
        &mut session,
        &client,
        Duration::ZERO
    ));
    assert!(host.try_recv().is_err());
    assert!(session.sends_after_interruption());
    session.apply(Event::Reply {
        request: earlier.id,
        error: None,
    });
    left(&mut session, &client, &host, Duration::ZERO);
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
