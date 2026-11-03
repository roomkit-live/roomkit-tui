#![allow(clippy::unwrap_used, reason = "Test fixtures fail on invalid state")]
mod exit;
mod keys;
mod send_now;

use super::keys::{Outcome, handle_key};
use super::*;
use crate::{Host, connection};
use crossterm::event::KeyModifiers;
use roomkit_tui_protocol::{
    Actor, Choice, Command, CommandSpec, Decision, Event, Item, ModeState, Permission, RunOutcome,
    Snapshot, ToolCall, ToolState,
};
use std::time::Duration;

pub(super) fn fixture(active: bool) -> (Session, Client, Host) {
    let mut session = Session::default();
    let mut snapshot = Snapshot::new("room", "Example", Actor::new("agent", "Agent"));
    snapshot.commands = vec![CommandSpec::new("action", "Example")];
    snapshot.active_run = active.then(|| "run".into());
    snapshot.can_interrupt = true;
    session.apply(Event::Snapshot(snapshot));
    let (client, host) = connection(std::num::NonZeroUsize::new(8).unwrap());
    (session, client, host)
}

pub(super) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub(super) fn ctrl_c() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
}

pub(super) fn escape() -> KeyEvent {
    key(KeyCode::Esc)
}

pub(super) fn press(
    session: &mut Session,
    client: &Client,
    event: KeyEvent,
    now: Duration,
) -> bool {
    handle_key(session, client, event, now, false) == Outcome::Exit
}

pub(super) fn permission(id: &str) -> Permission {
    Permission::new(id, "agent", "run", "Delete?", "rm -rf build")
}

pub(super) fn request_permission(session: &mut Session, id: &str, at: Duration) {
    session.apply_at(
        Event::PermissionRequested {
            conversation: "room".into(),
            permission: permission(id),
        },
        at,
    );
    draw(session, at);
}

/// Render as the terminal does at `at`, recording what the frame showed.
pub(super) fn draw(session: &mut Session, at: Duration) {
    let frame = render::screen_with_options(
        session,
        100,
        30,
        &Renderers::default(),
        &UiOptions::default(),
        at,
    );
    session.sync(&frame, at);
}

/// Review finding 1: the approval card used to accept Enter at once, so a
/// person sending a message as the card appeared approved it unseen.
/// Review finding 2: Shift+Tab cycled the host's mode before any overlay saw
/// it, so it could raise the policy while a decision was on screen, and the
/// completion menu could not move up with it.
#[test]
fn pasted_text_reaches_only_the_focused_editor() {
    let (mut session, _client, _host) = fixture(true);
    request_permission(&mut session, "p", Duration::ZERO);
    paste(&mut session, "y\n");
    assert!(
        session.draft().is_empty(),
        "A paste never reaches a hidden composer"
    );
    session.apply(Event::PermissionResolved {
        conversation: "room".into(),
        permission: "p".into(),
    });
    paste(&mut session, "hello");
    assert_eq!(session.draft(), "hello");
}

#[test]
fn idle_polling_never_schedules_extra_frames_but_changes_are_immediate() {
    let mut redraw = Redraw::default();
    assert!(redraw.due(Duration::ZERO, false));
    redraw.drawn(Duration::ZERO);
    for tick in 1..=400 {
        assert!(!redraw.due(Duration::from_millis(tick * 25), false));
    }
    redraw.dirty = true;
    assert!(redraw.due(Duration::from_millis(10_001), false));
}

#[test]
fn timers_refresh_ten_times_per_second_without_delaying_events() {
    let mut redraw = Redraw::default();
    redraw.drawn(Duration::ZERO);
    let mut frames = 0;
    for tick in 1..=40 {
        let now = Duration::from_millis(tick * 25);
        if redraw.due(now, true) {
            frames += 1;
            redraw.drawn(now);
        }
    }
    assert_eq!(frames, 10);
    redraw.dirty = true;
    assert!(redraw.due(Duration::from_millis(1001), true));
    redraw.drawn(Duration::from_millis(1001));
    // Completion or a frozen text selection stops timed work immediately.
    assert!(!redraw.due(Duration::from_secs(2), false));
}
