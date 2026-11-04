//! RMK-1285, RMK-1303: Ctrl+S, or Ctrl+Enter, sends the draft now. It cuts
//! whatever runs, as Esc does, and the draft leaves once nothing runs, staying
//! in the composer until the host takes it.

#![allow(clippy::unwrap_used, reason = "Test fixtures fail on invalid state")]
use super::*;
use roomkit_tui_protocol::{InputAction, Message, Rejection, Request};

use crate::Steering;

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
    assert!(
        session.notice().is_empty(),
        "Sent, as any message clears it"
    );
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

/// The run seen while the draft is on its way may be the one it started: it
/// is not cut before the host has answered.
#[test]
fn a_draft_on_its_way_never_cuts_the_turn_it_started() {
    let (mut session, client, host) = waiting_idle();
    let sent = left(&mut session, &client, &host, NOW);
    run_started(&mut session, "its own");
    assert!(!pass(&mut session, &client, NOW));
    assert!(host.try_recv().is_err(), "The draft cut its own turn");
    taken(&mut session, &sent, NOW);
    assert!(session.draft().is_empty());
    assert!(!pass(&mut session, &client, LATER));
    assert!(host.try_recv().is_err(), "The draft cut its own turn");
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

/// Light review of 9a3f306: Esc can come too late. The message left, and a
/// copy left behind would be sent twice: a notice saying so did not outlive
/// the turn's start, so the composer empties as for any message taken.
#[test]
fn escape_too_late_leaves_no_copy_behind() {
    let (mut session, client, host) = waiting_idle();
    let sent = left(&mut session, &client, &host, NOW);
    press(&mut session, &client, escape(), NOW);
    taken(&mut session, &sent, NOW);
    assert!(session.draft().is_empty(), "A copy stayed to be sent again");
    assert!(!session.sends_after_interruption());
    assert!(!pass(&mut session, &client, LATER));
    assert!(host.try_recv().is_err());
}

/// A dropped draft silences being busy, not another refusal.
#[test]
fn another_refusal_of_a_dropped_draft_is_told() {
    let (mut session, client, host) = waiting_idle();
    let sent = left(&mut session, &client, &host, NOW);
    press(&mut session, &client, escape(), NOW);
    refused(&mut session, &sent, Rejection::refused("Not allowed"), NOW);
    assert_eq!(session.notice(), "Not allowed");
}

/// The draft leaves once, whatever it became: a snapshot may advertise an
/// action that turns it into a command, which no pending message holds back.
#[test]
fn one_request_at_a_time_whatever_the_draft_became() {
    let (mut session, client, host) = fixture(true);
    session.insert("!run");
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    let mut snapshot = Snapshot::new("room", "Example", Actor::new("agent", "Agent"));
    snapshot.can_interrupt = true;
    snapshot.commands = vec![CommandSpec::new("run", "Run")];
    snapshot.input_actions = vec![InputAction::new('!', "run", "Run")];
    session.apply(Event::Snapshot(snapshot));
    assert!(pass(&mut session, &client, NOW));
    assert!(matches!(
        host.try_recv().unwrap().command,
        Command::Invoke { .. }
    ));
    assert!(!pass(&mut session, &client, NOW));
    assert!(host.try_recv().is_err(), "Sent again before its answer");
}

/// A second press on a waiting draft says nothing, even if a run it cannot
/// cut has started since.
#[test]
fn pressing_again_over_a_run_it_cannot_cut_says_nothing() {
    let (mut session, client, host) = fixture(false);
    let mut snapshot = Snapshot::new("room", "Example", Actor::new("agent", "Agent"));
    snapshot.can_interrupt = false;
    session.apply(Event::Snapshot(snapshot));
    session.insert(DRAFT);
    ctrl_s(&mut session, &client);
    let sent = host.try_recv().unwrap();
    refused(&mut session, &sent, busy(), NOW);
    started(&mut session);
    ctrl_s(&mut session, &client);
    assert!(session.sends_after_interruption());
    assert!(session.notice().is_empty(), "{}", session.notice());
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

fn screen(session: &Session) -> String {
    let options = UiOptions::default();
    let renderers = Renderers::default();
    render::screen_with_options(session, 120, 30, &renderers, &options, NOW)
        .lines
        .join("\n")
}

#[test]
fn the_row_under_the_composer_names_the_key_only_for_what_is_typed() {
    let (mut session, client, _host) = fixture(true);
    assert!(!screen(&session).contains("Ctrl+S"));
    session.insert("Now");
    assert!(screen(&session).contains("Ctrl+S send now"));
    ctrl_s(&mut session, &client);
    assert!(screen(&session).contains("Sends once the work stops"));
}

/// A view on a host that answers, with `run` live and "handed" handed to it.
fn handed() -> (Session, Client, Host) {
    let (mut session, client, host) = fixture(false);
    let mut snapshot = Snapshot::new("room", "Example", Actor::new("agent", "Agent"));
    snapshot.can_interrupt = true;
    snapshot.can_steer = true;
    snapshot.can_answer = true;
    session.apply(Event::Snapshot(snapshot));
    session.apply(Event::OperationStarted {
        conversation: "room".into(),
        operation: "op".into(),
    });
    started(&mut session);
    hand(&mut session, "handed");
    (session, client, host)
}

fn hand(session: &mut Session, id: &str) {
    session.apply(Event::Item {
        conversation: "room".into(),
        item: Item::Message(Message::new(id, "person:local", "Stop and do this")),
    });
    session.apply(Event::MessageSteered {
        conversation: "room".into(),
        message: id.into(),
        run: "run".into(),
    });
}

fn answer() -> Command {
    Command::Answer {
        agent: "agent".into(),
    }
}

/// RMK-1311: a message handed to a turn waits for that turn's next step, which
/// may be far. Its line says Ctrl+S cuts the turn, as long as the turn runs.
#[test]
fn the_line_of_a_handed_message_names_the_key_while_its_turn_runs() {
    let (mut session, _client, _host) = handed();
    assert!(screen(&session).contains("↳ sent to the running turn · Ctrl+S to cut it now"));
    stopped(&mut session);
    let drawn = screen(&session);
    assert!(drawn.contains("↳ sent to the running turn"));
    assert!(!drawn.contains("Ctrl+S"), "The turn it would cut is over");
}

/// RMK-1311: with an empty composer, Ctrl+S cuts the turn, and asks for a turn
/// that answers the message: nothing new is written, and nothing is duplicated.
#[test]
fn ctrl_s_on_an_empty_composer_cuts_the_turn_and_answers_the_message() {
    let (mut session, client, host) = handed();
    ctrl_s(&mut session, &client);
    assert_eq!(host.try_recv().unwrap().command, interrupt());
    assert!(session.sends_after_interruption());
    assert!(screen(&session).contains("Answers once the work stops"));

    stopped(&mut session);
    assert!(
        !pass(&mut session, &client, NOW),
        "Its operation still runs"
    );
    finished(&mut session);
    assert!(pass(&mut session, &client, NOW));
    let sent = host.try_recv().unwrap();
    assert_eq!(sent.command, answer());

    // Refused as busy, it is asked again, as a draft is.
    refused(&mut session, &sent, busy(), LATER);
    assert!(session.notice().is_empty());
    let retry = LATER + Duration::from_millis(200);
    assert!(pass(&mut session, &client, retry));
    let again = host.try_recv().unwrap();
    assert_eq!(again.command, answer());
    taken(&mut session, &again, retry);
    assert!(!session.sends_after_interruption());
    assert_eq!(session.steering(&"handed".into()), Some(Steering::OwnTurn));
    assert!(!screen(&session).contains("Ctrl+S"));
    assert!(!pass(&mut session, &client, retry * 2));
    assert!(host.try_recv().is_err(), "Asked twice");
}

/// An answer on its way counts as a message awaiting its reply: Enter would
/// send another past it.
#[test]
fn enter_waits_for_an_answer_on_its_way() {
    let (mut session, client, host) = handed();
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    stopped(&mut session);
    finished(&mut session);
    assert!(pass(&mut session, &client, NOW));
    assert_eq!(host.try_recv().unwrap().command, answer());
    session.insert("Another thought");
    press(&mut session, &client, key(KeyCode::Enter), NOW);
    assert!(host.try_recv().is_err(), "Sent past the answer on its way");
    assert_eq!(session.draft(), "Another thought");
}

#[test]
fn escape_or_typing_keeps_the_handed_message_from_being_answered_now() {
    let (mut session, client, host) = handed();
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    press(&mut session, &client, escape(), NOW);
    assert!(!session.sends_after_interruption());
    stopped(&mut session);
    finished(&mut session);
    assert!(!pass(&mut session, &client, NOW));
    assert!(host.try_recv().is_err(), "Esc did not cancel");

    let (mut session, client, host) = handed();
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    session.set_draft("Another thought");
    stopped(&mut session);
    finished(&mut session);
    assert!(!pass(&mut session, &client, NOW));
    assert!(host.try_recv().is_err(), "The typed text was overtaken");
    assert_eq!(session.draft(), "Another thought");
}

/// A draft in the composer takes the key: it leaves, and the next turn reads
/// the handed message with it.
#[test]
fn a_draft_goes_before_the_handed_message() {
    let (mut session, client, host) = handed();
    session.insert(DRAFT);
    ctrl_s(&mut session, &client);
    assert_eq!(host.try_recv().unwrap().command, interrupt());
    stopped(&mut session);
    finished(&mut session);
    left(&mut session, &client, &host, NOW);
}

/// Review of aa33d7c: RoomKit may give the message its own turn instead of
/// handing it over. That turn is the answer: the wait must not cut it.
#[test]
fn a_turn_of_its_own_for_the_message_ends_the_wait() {
    let (mut session, client, host) = handed();
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    stopped(&mut session);
    session.apply(Event::RunStarted {
        conversation: "room".into(),
        run: "own".into(),
        answers: Some("handed".into()),
    });
    assert!(!session.sends_after_interruption());
    assert!(!pass(&mut session, &client, NOW));
    assert!(host.try_recv().is_err(), "The message's own turn was cut");
}

/// Review of aa33d7c: a turn that ends by itself read what it was handed:
/// answering again would answer twice, and tell the model it was cut.
#[test]
fn a_turn_that_completes_ends_the_wait() {
    let (mut session, client, host) = handed();
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    session.apply(Event::RunFinished {
        conversation: "room".into(),
        run: "run".into(),
        outcome: RunOutcome::Completed,
    });
    finished(&mut session);
    assert!(!pass(&mut session, &client, NOW));
    assert!(host.try_recv().is_err(), "Answered a turn that read it");
}

/// Light review of 6194726: another run's end says nothing of the message: the
/// wait goes on, and a turn of its own still answers it.
#[test]
fn another_run_that_completes_leaves_the_wait() {
    let (mut session, client, host) = handed();
    run_started(&mut session, "another");
    ctrl_s(&mut session, &client);
    assert_eq!(host.try_recv().unwrap().command, interrupt());
    session.apply(Event::RunFinished {
        conversation: "room".into(),
        run: "another".into(),
        outcome: RunOutcome::Completed,
    });
    assert!(session.sends_after_interruption());
    stopped(&mut session);
    finished(&mut session);
    assert!(pass(&mut session, &client, NOW));
    assert_eq!(host.try_recv().unwrap().command, answer());
}

/// Review of aa33d7c: every message handed to the cut turn is answered, and
/// says so, not only the last.
#[test]
fn every_message_handed_to_the_cut_turn_is_answered() {
    let (mut session, client, host) = handed();
    hand(&mut session, "also");
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    stopped(&mut session);
    finished(&mut session);
    assert!(pass(&mut session, &client, NOW));
    let sent = host.try_recv().unwrap();
    taken(&mut session, &sent, NOW);
    for message in ["handed", "also"] {
        assert_eq!(session.steering(&message.into()), Some(Steering::OwnTurn));
    }
}

/// The turn's round of calls ends, and the model answers again.
fn passes_a_stop(session: &mut Session) {
    for state in [ToolState::Running, ToolState::Succeeded] {
        session.apply(Event::Item {
            conversation: "room".into(),
            item: Item::Tool {
                actor: "agent".into(),
                run: "run".into(),
                call: ToolCall::new("call", "lookup", serde_json::json!({}), state),
            },
        });
    }
    session.apply(Event::TextDelta {
        conversation: "room".into(),
        message: "draft".into(),
        actor: "agent".into(),
        run: "run".into(),
        text: "Going on".into(),
    });
}

/// RMK-1317: the cut answers the message it was asked for and what no stop
/// read. What a stop read stays read.
#[test]
fn a_cut_leaves_what_the_turn_read_as_read() {
    let (mut session, client, host) = handed();
    passes_a_stop(&mut session);
    assert_eq!(session.steering(&"handed".into()), Some(Steering::Read));
    hand(&mut session, "also");
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    stopped(&mut session);
    finished(&mut session);
    assert!(pass(&mut session, &client, NOW));
    let sent = host.try_recv().unwrap();
    taken(&mut session, &sent, NOW);
    assert_eq!(session.steering(&"handed".into()), Some(Steering::Read));
    assert_eq!(session.steering(&"also".into()), Some(Steering::OwnTurn));
}

/// A cut speaks for the turn it cuts: a message an earlier turn left unread
/// keeps its line.
#[test]
fn a_cut_leaves_a_message_of_an_earlier_turn_alone() {
    let (mut session, client, host) = handed();
    stopped(&mut session);
    run_started(&mut session, "next");
    session.apply(Event::Item {
        conversation: "room".into(),
        item: Item::Message(Message::new("later", "person:local", "Now this")),
    });
    session.apply(Event::MessageSteered {
        conversation: "room".into(),
        message: "later".into(),
        run: "next".into(),
    });
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    run_stopped(&mut session, "next");
    finished(&mut session);
    assert!(pass(&mut session, &client, NOW));
    let sent = host.try_recv().unwrap();
    taken(&mut session, &sent, NOW);
    assert_eq!(session.steering(&"handed".into()), Some(Steering::Handed));
    assert_eq!(session.steering(&"later".into()), Some(Steering::OwnTurn));
}

/// A message the turn read can still be cut for: the model may go on without
/// answering it.
#[test]
fn a_cut_answers_a_message_the_turn_read() {
    let (mut session, client, host) = handed();
    passes_a_stop(&mut session);
    assert!(screen(&session).contains("read by the turn · Ctrl+S"));
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    stopped(&mut session);
    finished(&mut session);
    assert!(pass(&mut session, &client, NOW));
    let sent = host.try_recv().unwrap();
    taken(&mut session, &sent, NOW);
    assert_eq!(session.steering(&"handed".into()), Some(Steering::OwnTurn));
}

/// Review of aa33d7c: typing ends a wait to answer, even if the composer then
/// empties again: its text was sent, or dropped.
#[test]
fn typing_ends_a_wait_to_answer_for_good() {
    let (mut session, client, host) = handed();
    ctrl_s(&mut session, &client);
    host.try_recv().unwrap();
    session.set_draft("foo");
    assert!(!pass(&mut session, &client, NOW));
    session.set_draft("");
    stopped(&mut session);
    finished(&mut session);
    assert!(!pass(&mut session, &client, NOW));
    assert!(host.try_recv().is_err(), "The ended wait came back");
}

/// Review of aa33d7c: a host that cannot answer offers nothing to cut for: the
/// turn would be cut, and nothing would answer.
#[test]
fn a_host_that_cannot_answer_offers_no_cut() {
    let (mut session, client, host) = handed();
    let mut snapshot = Snapshot::new("room", "Example", Actor::new("agent", "Agent"));
    snapshot.can_interrupt = true;
    snapshot.can_steer = true;
    snapshot.active_run = Some("run".into());
    session.apply(Event::Snapshot(snapshot));
    assert!(!screen(&session).contains("Ctrl+S"));
    ctrl_s(&mut session, &client);
    assert!(host.try_recv().is_err(), "The turn was cut for nothing");
    assert_eq!(session.notice(), "Enter a message");
}

#[test]
fn an_empty_composer_with_nothing_handed_has_nothing_to_send() {
    let (mut session, client, host) = fixture(true);
    ctrl_s(&mut session, &client);
    assert!(host.try_recv().is_err());
    assert_eq!(session.notice(), "Enter a message");
}
