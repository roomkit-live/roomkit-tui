//! RMK-1285: Ctrl+S, or Ctrl+Enter, sends the draft now. During a turn it
//! cuts the turn, as Esc does, and the draft leaves once the turn stopped.

#![allow(clippy::unwrap_used, reason = "Test fixtures fail on invalid state")]
use super::*;
use roomkit_tui_protocol::Rejection;

fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

fn stopped(session: &mut Session) {
    session.apply(Event::RunFinished {
        conversation: "room".into(),
        run: "run".into(),
        outcome: RunOutcome::Interrupted,
    });
}

fn submitted(text: &str) -> Command {
    Command::Submit {
        agent: "agent".into(),
        text: text.into(),
    }
}

#[test]
fn ctrl_s_during_a_turn_cuts_it_and_sends_the_draft_once_it_stopped() {
    for send_now in [ctrl(KeyCode::Char('s')), ctrl(KeyCode::Enter)] {
        let (mut session, client, host) = fixture(true);
        session.insert("Use the second ledger");
        assert!(!press(&mut session, &client, send_now, Duration::ZERO));
        assert_eq!(
            host.try_recv().unwrap().command,
            Command::Interrupt { run: "run".into() }
        );
        assert_eq!(session.draft(), "Use the second ledger");
        assert!(session.sends_after_interruption());
        // The turn still runs: the draft waits.
        assert!(!composer::send_waiting(&mut session, &client));
        assert!(host.try_recv().is_err());

        stopped(&mut session);
        assert!(composer::send_waiting(&mut session, &client));
        assert_eq!(
            host.try_recv().unwrap().command,
            submitted("Use the second ledger")
        );
        assert!(session.draft().is_empty());
        assert!(!session.sends_after_interruption());
        assert!(!composer::send_waiting(&mut session, &client));
    }
}

#[test]
fn escape_keeps_a_draft_that_waited_for_the_interruption() {
    let (mut session, client, host) = fixture(true);
    session.insert("Second thoughts");
    press(
        &mut session,
        &client,
        ctrl(KeyCode::Char('s')),
        Duration::ZERO,
    );
    assert!(matches!(
        host.try_recv().unwrap().command,
        Command::Interrupt { .. }
    ));
    press(&mut session, &client, escape(), Duration::ZERO);
    assert!(!session.sends_after_interruption());
    assert!(host.try_recv().is_err(), "Esc sent another request");
    stopped(&mut session);
    assert!(!composer::send_waiting(&mut session, &client));
    assert!(host.try_recv().is_err(), "The kept draft was sent");
    assert_eq!(session.draft(), "Second thoughts");
}

#[test]
fn a_refused_interruption_sends_nothing() {
    let (mut session, client, host) = fixture(true);
    session.insert("Stop and do this");
    press(
        &mut session,
        &client,
        ctrl(KeyCode::Char('s')),
        Duration::ZERO,
    );
    let interruption = host.try_recv().unwrap();
    session.apply(Event::Reply {
        request: interruption.id,
        error: Some(Rejection::refused("This execution cannot be interrupted")),
    });
    assert!(!session.sends_after_interruption());
    assert_eq!(session.notice(), "This execution cannot be interrupted");
    stopped(&mut session);
    assert!(!composer::send_waiting(&mut session, &client));
    assert!(host.try_recv().is_err());
    assert_eq!(session.draft(), "Stop and do this");
}

#[test]
fn ctrl_s_is_enter_without_a_turn_or_for_a_command() {
    let (mut session, client, host) = fixture(false);
    session.insert("Hello");
    press(
        &mut session,
        &client,
        ctrl(KeyCode::Char('s')),
        Duration::ZERO,
    );
    assert_eq!(host.try_recv().unwrap().command, submitted("Hello"));

    let (mut session, client, host) = fixture(true);
    session.insert("/action");
    session.dismiss_completions();
    press(
        &mut session,
        &client,
        ctrl(KeyCode::Char('s')),
        Duration::ZERO,
    );
    assert!(matches!(
        host.try_recv().unwrap().command,
        Command::Invoke { .. }
    ));
    assert!(!session.sends_after_interruption());
}

#[test]
fn ctrl_s_never_waits_on_a_turn_it_cannot_cut_or_with_nothing_to_send() {
    let (mut session, client, host) = fixture(true);
    press(
        &mut session,
        &client,
        ctrl(KeyCode::Char('s')),
        Duration::ZERO,
    );
    assert!(host.try_recv().is_err());
    assert!(!session.sends_after_interruption());

    let mut snapshot = Snapshot::new("room", "Example", Actor::new("agent", "Agent"));
    snapshot.active_run = Some("run".into());
    snapshot.can_interrupt = false;
    session.apply(Event::Snapshot(snapshot));
    session.insert("Now");
    press(
        &mut session,
        &client,
        ctrl(KeyCode::Char('s')),
        Duration::ZERO,
    );
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
    let hints = |session: &Session| {
        let options = UiOptions::default();
        let renderers = Renderers::default();
        render::screen_with_options(session, 120, 30, &renderers, &options, Duration::ZERO)
            .lines
            .join("\n")
    };
    assert!(hints(&session).contains("Ctrl+S send now"));
    session.insert("Now");
    press(
        &mut session,
        &client,
        ctrl(KeyCode::Char('s')),
        Duration::ZERO,
    );
    assert!(hints(&session).contains("Sends once the turn stops"));
}
