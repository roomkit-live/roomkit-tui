//! RMK-1285, RMK-1303: Ctrl+S, or Ctrl+Enter, sends the draft now. It cuts
//! whatever runs, as Esc does, and the draft leaves once nothing runs, staying
//! in the composer until the host takes it.

#![allow(clippy::unwrap_used, reason = "Test fixtures fail on invalid state")]
use super::*;
use roomkit_tui_protocol::{Rejection, Request};

const DRAFT: &str = "Use the second ledger";
const NOW: Duration = Duration::ZERO;
const LATER: Duration = Duration::from_secs(1);

fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

fn ctrl_s(session: &mut Session, client: &Client) {
    press(session, client, ctrl(KeyCode::Char('s')), NOW);
}

/// One pass of the terminal's loop at `at`: whether the waiting draft moved.
fn pass(session: &mut Session, client: &Client, at: Duration) -> bool {
    composer::send_waiting(session, client, at)
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

fn run_started(session: &mut Session, run: &str) {
    session.apply(Event::RunStarted {
        conversation: "room".into(),
        run: run.into(),
        answers: None,
    });
}

fn run_stopped(session: &mut Session, run: &str) {
    session.apply(Event::RunFinished {
        conversation: "room".into(),
        run: run.into(),
        outcome: RunOutcome::Interrupted,
    });
}

fn started(session: &mut Session) {
    run_started(session, "run");
}

fn stopped(session: &mut Session) {
    run_stopped(session, "run");
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

fn interrupt_of(run: &str) -> Command {
    Command::Interrupt { run: run.into() }
}

fn interrupt() -> Command {
    interrupt_of("run")
}

/// The draft goes to the host at `at`, and stays in the composer meanwhile.
fn left(session: &mut Session, client: &Client, host: &Host, at: Duration) -> Request {
    assert!(pass(session, client, at));
    let sent = host.try_recv().unwrap();
    assert_eq!(sent.command, submitted(DRAFT));
    assert_eq!(session.draft(), DRAFT);
    sent
}

fn refused(session: &mut Session, sent: &Request, error: Rejection, at: Duration) {
    let reply = Event::Reply {
        request: sent.id,
        error: Some(error),
    };
    session.apply_at(reply, at);
}

fn taken(session: &mut Session, sent: &Request, at: Duration) {
    let reply = Event::Reply {
        request: sent.id,
        error: None,
    };
    session.apply_at(reply, at);
}

fn busy() -> Rejection {
    Rejection::busy("A request is still executing")
}

/// The view once its run was cut and stopped, with the draft waiting.
fn waiting_idle() -> (Session, Client, Host) {
    let (mut session, client, host) = fixture(true);
    session.insert(DRAFT);
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    stopped(&mut session);
    (session, client, host)
}

#[test]
fn ctrl_s_during_a_turn_cuts_it_and_sends_the_draft_once_it_stopped() {
    for send_now in [ctrl(KeyCode::Char('s')), ctrl(KeyCode::Enter)] {
        let (mut session, client, host) = working();
        started(&mut session);
        session.insert(DRAFT);
        assert!(!press(&mut session, &client, send_now, NOW));
        assert_eq!(host.try_recv().unwrap().command, interrupt());
        assert_eq!(session.draft(), DRAFT);
        assert!(session.sends_after_interruption());

        // The run ended, but its operation still holds the room.
        stopped(&mut session);
        assert!(!pass(&mut session, &client, NOW));
        assert!(host.try_recv().is_err());

        finished(&mut session);
        let sent = left(&mut session, &client, &host, NOW);
        taken(&mut session, &sent, NOW);
        assert!(session.draft().is_empty());
        assert!(!session.sends_after_interruption());
        assert!(!pass(&mut session, &client, NOW));
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
    assert!(pass(&mut session, &client, NOW));
    assert_eq!(host.try_recv().unwrap().command, interrupt());
    assert!(!pass(&mut session, &client, NOW));
    assert!(host.try_recv().is_err(), "The run was cut twice");

    stopped(&mut session);
    finished(&mut session);
    left(&mut session, &client, &host, NOW);
}

#[test]
fn escape_keeps_a_draft_that_waited_for_the_interruption() {
    let (mut session, client, host) = fixture(true);
    session.insert("Second thoughts");
    ctrl_s(&mut session, &client);
    assert_eq!(host.try_recv().unwrap().command, interrupt());
    press(&mut session, &client, escape(), NOW);
    assert!(!session.sends_after_interruption());
    assert!(host.try_recv().is_err(), "Esc sent another request");
    stopped(&mut session);
    assert!(!pass(&mut session, &client, NOW));
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
    let refusal = Rejection::refused("This execution cannot be interrupted");
    refused(&mut session, &interruption, refusal, NOW);
    assert_eq!(session.notice(), "This execution cannot be interrupted");
    assert!(session.sends_after_interruption());
    assert!(!pass(&mut session, &client, NOW));
    assert!(host.try_recv().is_err(), "The refused run was asked again");
    stopped(&mut session);
    left(&mut session, &client, &host, NOW);
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
        assert!(!pass(&mut session, &client, NOW));
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
    assert!(!pass(&mut session, &client, NOW));
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
    run_started(&mut session, "next");
    assert!(pass(&mut session, &client, NOW));
    assert_eq!(host.try_recv().unwrap().command, interrupt_of("next"));
    let refusal = Rejection::refused("This execution has already ended");
    refused(&mut session, &first, refusal, NOW);
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
    press(&mut session, &client, ctrl_c(), NOW);
    session.insert(DRAFT);
    stopped(&mut session);
    assert!(!pass(&mut session, &client, NOW));
    assert!(host.try_recv().is_err(), "A draft typed after Ctrl+C left");

    let (mut session, client, host) = fixture(true);
    session.insert(DRAFT);
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    let other = Snapshot::new("other", "Other", Actor::new("agent", "Agent"));
    session.apply(Event::Snapshot(other));
    assert!(!pass(&mut session, &client, NOW));
    assert!(
        host.try_recv().is_err(),
        "The draft went to another conversation"
    );
    assert_eq!(session.draft(), DRAFT);
}

/// RMK-1303: the host may hold the room with no run to show, as for the own
/// turn of a message sent earlier. A draft it refuses as busy goes on
/// waiting, and nothing on screen changes until the host takes it.
#[test]
fn a_draft_refused_as_busy_stays_and_is_tried_again() {
    let (mut session, client, host) = waiting_idle();
    session.scroll_back(5);
    let scrolled = session.scroll;
    session.notice = "From the host".into();
    let sent = left(&mut session, &client, &host, LATER);
    assert!(session.sends_after_interruption());
    refused(&mut session, &sent, busy(), LATER);
    assert_eq!(session.draft(), DRAFT);
    assert!(session.sends_after_interruption());
    assert_eq!(session.notice(), "From the host", "A retry reports nothing");
    assert!(!pass(&mut session, &client, LATER));
    assert!(host.try_recv().is_err(), "Tried again at once");

    let retry = LATER + Duration::from_millis(200);
    let again = left(&mut session, &client, &host, retry);
    assert_eq!(session.scroll, scrolled, "A retry moved the view");
    taken(&mut session, &again, retry);
    assert!(session.draft().is_empty());
    assert!(!session.sends_after_interruption());
    assert!(!pass(&mut session, &client, retry * 2));
    assert!(host.try_recv().is_err(), "The draft left twice");
}

/// RMK-1303: Ctrl+S cuts everything, whichever way the events come. The own
/// turn of a message sent earlier starts after the refusal, or before it.
#[test]
fn a_turn_that_starts_around_a_busy_refusal_is_cut_too() {
    for before_the_refusal in [false, true] {
        let (mut session, client, host) = waiting_idle();
        let sent = left(&mut session, &client, &host, NOW);
        if before_the_refusal {
            run_started(&mut session, "own");
            refused(&mut session, &sent, busy(), NOW);
        } else {
            refused(&mut session, &sent, busy(), NOW);
            run_started(&mut session, "own");
        }
        assert!(pass(&mut session, &client, LATER));
        assert_eq!(host.try_recv().unwrap().command, interrupt_of("own"));
        run_stopped(&mut session, "own");
        left(&mut session, &client, &host, LATER);
    }
}

/// RMK-1303: with nothing shown as running, Ctrl+S still follows the rule:
/// the host may hold the room between a turn and the next.
#[test]
fn ctrl_s_with_nothing_shown_running_still_waits_and_cuts() {
    let (mut session, client, host) = fixture(false);
    session.insert(DRAFT);
    ctrl_s(&mut session, &client);
    let sent = host.try_recv().unwrap();
    assert_eq!(sent.command, submitted(DRAFT));
    refused(&mut session, &sent, busy(), NOW);
    assert_eq!(session.draft(), DRAFT);
    started(&mut session);
    assert!(pass(&mut session, &client, LATER));
    assert_eq!(host.try_recv().unwrap().command, interrupt());
}

/// Review of 21765e6: a second press during a hold sent the draft untracked,
/// which ended the wait. Neither it nor Enter does anything more.
#[test]
fn pressing_again_changes_nothing_while_the_draft_waits() {
    let (mut session, client, host) = waiting_idle();
    let sent = left(&mut session, &client, &host, LATER);
    refused(&mut session, &sent, busy(), LATER);
    ctrl_s(&mut session, &client);
    press(&mut session, &client, key(KeyCode::Enter), LATER);
    assert!(host.try_recv().is_err(), "Sent before its time");
    assert_eq!(session.draft(), DRAFT);
    assert!(session.sends_after_interruption());
    left(
        &mut session,
        &client,
        &host,
        LATER + Duration::from_millis(200),
    );
}

/// A host that keeps refusing is not asked for ever: the draft stays in the
/// composer, with its reason.
#[test]
fn a_host_that_stays_busy_leaves_the_draft_in_the_composer() {
    let (mut session, client, host) = waiting_idle();
    let sent = left(&mut session, &client, &host, NOW);
    refused(&mut session, &sent, busy(), NOW);

    let past = Duration::from_secs(30);
    let sent = left(&mut session, &client, &host, past);
    refused(&mut session, &sent, busy(), past);
    assert_eq!(session.draft(), DRAFT);
    assert!(!session.sends_after_interruption());
    assert_eq!(session.notice(), "A request is still executing");
    assert!(!pass(&mut session, &client, past * 2));
    assert!(host.try_recv().is_err(), "The host was asked again");
}

/// Review of 21765e6: the clock measures a host that shows no run, not a
/// long run: a refusal after one starts it anew.
#[test]
fn a_run_restarts_the_time_a_busy_host_gets() {
    let (mut session, client, host) = waiting_idle();
    let sent = left(&mut session, &client, &host, NOW);
    refused(&mut session, &sent, busy(), NOW);
    run_started(&mut session, "own");
    assert!(pass(&mut session, &client, NOW));
    host.try_recv().unwrap();
    run_stopped(&mut session, "own");

    let past = Duration::from_secs(45);
    let sent = left(&mut session, &client, &host, past);
    refused(&mut session, &sent, busy(), past);
    assert!(
        session.sends_after_interruption(),
        "Gave up on a run's time"
    );
}

#[test]
fn another_refusal_leaves_the_draft_without_trying_again() {
    let (mut session, client, host) = waiting_idle();
    let sent = left(&mut session, &client, &host, NOW);
    refused(&mut session, &sent, Rejection::refused("Not allowed"), NOW);
    assert_eq!(session.draft(), DRAFT);
    assert!(!session.sends_after_interruption());
    assert_eq!(session.notice(), "Not allowed");
    assert!(!pass(&mut session, &client, LATER));
    assert!(host.try_recv().is_err());
}

/// Review of 21765e6: Esc during a send was ignored, and the draft left at
/// the next try. Dropped, it is not sent again, and the refusal says nothing.
#[test]
fn escape_while_the_draft_is_on_its_way_ends_the_wait() {
    let (mut session, client, host) = waiting_idle();
    let sent = left(&mut session, &client, &host, NOW);
    press(&mut session, &client, escape(), NOW);
    assert!(!session.sends_after_interruption());
    refused(&mut session, &sent, busy(), NOW);
    assert_eq!(session.draft(), DRAFT);
    assert!(session.notice().is_empty());
    assert!(!pass(&mut session, &client, LATER));
    assert!(host.try_recv().is_err(), "The kept draft left");
}

/// Review of 21765e6: a key typed during a send replaced the draft. The
/// composer now keeps it, and the person's edit stays, whatever the answer.
#[test]
fn an_edit_while_the_draft_is_on_its_way_stays() {
    for answer_is_busy in [true, false] {
        let (mut session, client, host) = waiting_idle();
        let sent = left(&mut session, &client, &host, NOW);
        session.set_draft("Use the third ledger");
        if answer_is_busy {
            refused(&mut session, &sent, busy(), NOW);
        } else {
            taken(&mut session, &sent, NOW);
        }
        assert!(!pass(&mut session, &client, LATER));
        assert!(host.try_recv().is_err(), "The edited draft left unseen");
        assert_eq!(session.draft(), "Use the third ledger");
        assert!(!session.sends_after_interruption());
    }
}

/// Ctrl+S on the edit, while the first text is still on its way: the edit
/// leaves next, and the first answer does not touch it.
#[test]
fn a_new_draft_waits_for_the_one_on_its_way() {
    let (mut session, client, host) = waiting_idle();
    let sent = left(&mut session, &client, &host, NOW);
    session.set_draft("Use the third ledger");
    ctrl_s(&mut session, &client);
    assert!(host.try_recv().is_err(), "Two messages on their way");
    taken(&mut session, &sent, NOW);
    assert_eq!(session.draft(), "Use the third ledger");
    assert!(pass(&mut session, &client, NOW));
    assert_eq!(
        host.try_recv().unwrap().command,
        submitted("Use the third ledger")
    );
}

/// A refusal of the first text says nothing about the edit waiting after it.
#[test]
fn a_refusal_of_the_text_on_its_way_leaves_the_new_draft_waiting() {
    let (mut session, client, host) = waiting_idle();
    let sent = left(&mut session, &client, &host, NOW);
    session.set_draft("Use the third ledger");
    ctrl_s(&mut session, &client);
    refused(&mut session, &sent, Rejection::refused("Not allowed"), NOW);
    assert!(session.sends_after_interruption());
    assert!(pass(&mut session, &client, NOW));
    assert_eq!(
        host.try_recv().unwrap().command,
        submitted("Use the third ledger")
    );
}

/// Ctrl+C means the person dropped the draft, even one already on its way.
#[test]
fn ctrl_c_forgets_a_draft_on_its_way() {
    let (mut session, client, host) = waiting_idle();
    let sent = left(&mut session, &client, &host, NOW);
    press(&mut session, &client, ctrl_c(), NOW);
    refused(&mut session, &sent, busy(), NOW);
    assert!(session.draft().is_empty(), "The dropped draft came back");
    assert!(!session.sends_after_interruption());
    assert!(!pass(&mut session, &client, LATER));
    assert!(host.try_recv().is_err(), "The dropped draft left again");
}

/// A new snapshot loses every pending answer: the draft may have left, so
/// it is not sent again, and stays for the person to judge.
#[test]
fn a_snapshot_during_the_send_ends_the_wait() {
    let (mut session, client, host) = waiting_idle();
    left(&mut session, &client, &host, NOW);
    let same = Snapshot::new("room", "Example", Actor::new("agent", "Agent"));
    session.apply(Event::Snapshot(same));
    assert!(!session.sends_after_interruption());
    assert_eq!(session.draft(), DRAFT);
    assert!(!pass(&mut session, &client, LATER));
    assert!(host.try_recv().is_err(), "Sent twice");
}

/// A message with images leaves the composer as any image message does:
/// once the host took it.
#[test]
fn a_draft_with_images_is_tried_again_too() {
    let (mut session, client, host) = fixture(false);
    let mut snapshot = Snapshot::new("room", "Example", Actor::new("agent", "Agent"));
    snapshot.can_interrupt = true;
    snapshot.can_attach_images = true;
    session.apply(Event::Snapshot(snapshot));
    session.insert(DRAFT);
    session.add_image(crate::tests::image_fixture()).unwrap();
    ctrl_s(&mut session, &client);
    let sent = host.try_recv().unwrap();
    assert!(matches!(sent.command, Command::SubmitImages { .. }));
    refused(&mut session, &sent, busy(), NOW);
    assert!(session.sends_after_interruption());

    let retry = Duration::from_millis(200);
    assert!(pass(&mut session, &client, retry));
    let again = host.try_recv().unwrap();
    assert!(matches!(again.command, Command::SubmitImages { .. }));
    taken(&mut session, &again, retry);
    assert!(session.draft().is_empty());
    assert!(!session.sends_after_interruption());
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
    assert!(!pass(&mut session, &client, NOW));
    assert!(host.try_recv().is_err());
    assert!(session.sends_after_interruption());
    taken(&mut session, &earlier, NOW);
    left(&mut session, &client, &host, NOW);
}

#[test]
fn ctrl_s_is_enter_for_a_command() {
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
        render::screen_with_options(session, cols, 30, &renderers, &options, NOW)
            .lines
            .join("\n")
    };
    assert!(hints(&session, 120).contains("Ctrl+S send now"));
    let narrow = hints(&session, 60);
    assert!(narrow.contains("Ctrl+S send now") && !narrow.contains("Alt+Enter"));
    session.insert("Now");
    ctrl_s(&mut session, &client);
    assert!(hints(&session, 120).contains("Sends once the work stops"));
}
