//! Keyboard dispatch. Exactly one part of the view has focus; each has its own
//! handler, so a key can never reach a part the person is not looking at.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use roomkit_tui_protocol::{Command, Decision};

use super::{QUIT_TIMEOUT, interrupt_run, send};
use crate::session::Focus;
use crate::{Client, Session};

const MAX_FILTER_BYTES: usize = 256;
const MAX_SCROLL: usize = 65_536;
const PANEL_PAGE: usize = 10;
const DETAIL_PAGE: usize = 3;

/// What a key asks of the terminal loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    /// Keep the view open.
    Stay,
    /// Return control to the application.
    Exit,
}

pub(super) fn is_ctrl(key: KeyEvent, c: char) -> bool {
    key.code == KeyCode::Char(c) && key.modifiers.contains(KeyModifiers::CONTROL)
}

/// Ctrl+C or Ctrl+D: either confirms an armed exit.
pub(super) fn is_quit_key(key: KeyEvent) -> bool {
    is_ctrl(key, 'c') || is_ctrl(key, 'd')
}

/// Ctrl+D detaches at once, unless the host warns that exiting stops work.
/// Then a first press arms the same two-second confirmation as Ctrl+C.
fn detach(session: &mut Session, now: Duration) -> Outcome {
    if session.exit_warning.is_none() || session.quit_until.is_some() {
        return Outcome::Exit;
    }
    session.quit_until = Some(now.saturating_add(QUIT_TIMEOUT));
    session.quit_key = "Ctrl+D";
    Outcome::Stay
}

pub(super) fn is_cancel(key: KeyEvent) -> bool {
    key.code == KeyCode::Esc || is_ctrl(key, 'c')
}

/// Apply one key to the focused part of the view.
pub(super) fn handle_key(
    session: &mut Session,
    client: &Client,
    key: KeyEvent,
    now: Duration,
    tool_activity: bool,
) -> Outcome {
    // A held key must not confirm the exit it has just armed.
    if key.kind == KeyEventKind::Release
        || (key.kind == KeyEventKind::Repeat && (is_quit_key(key) || key.code == KeyCode::Esc))
    {
        return Outcome::Stay;
    }
    session.expire_quit_hint(now);
    if !is_quit_key(key) {
        session.quit_until = None;
    }
    if is_ctrl(key, 'd') {
        return detach(session, now);
    }
    let focus = session.focus();
    if key.code == KeyCode::F(6)
        && matches!(focus, Focus::Composer | Focus::Picker | Focus::Sidebar)
    {
        if let Some(command) = session.background_command() {
            send(session, client, command, false);
        }
        return Outcome::Stay;
    }
    match focus {
        Focus::Permission => permission_key(session, client, key, now),
        Focus::Panel => panel_key(session, client, key),
        Focus::Form => form_key(session, client, key),
        Focus::Picker => picker_key(session, client, key),
        Focus::Sidebar => sidebar_key(session, client, key),
        _ => return super::composer::composer_key(session, client, key, now, tool_activity),
    }
    Outcome::Stay
}

/// A decision is accepted only once the card has been on screen long enough
/// to be read; navigation and interruption stay immediate.
fn permission_key(session: &mut Session, client: &Client, key: KeyEvent, now: Duration) {
    if is_cancel(key) {
        cancel_card(session, client, now);
        return;
    }
    let Some(permission) = session.permissions().first() else {
        return;
    };
    if session.deciding(&permission.id) || key.kind == KeyEventKind::Repeat {
        return;
    }
    let plain = !key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    if plain && key.code == KeyCode::Char('o') && session.approval_armed(now) {
        if let Some(command) = session.open_permission_room() {
            send(session, client, command, false);
        }
        return;
    }
    let decision = card_decision(session, key, plain);
    if let Some(decision) = decision.filter(|_| session.approval_armed(now))
        && let Some(command) = session.decide(decision)
    {
        send(session, client, command, false);
    }
}

/// Esc or Ctrl+C on a card. Where stopping its run would stop more than this
/// action, it refuses the action instead, once the card can have been read.
fn cancel_card(session: &mut Session, client: &Client, now: Duration) {
    if session.card_refuses_on_cancel() {
        if session.approval_armed(now) {
            answer_card(session, client, Decision::Deny);
        }
        return;
    }
    // The card's run, which may be a later one than the view's own work.
    if let Some(run) = session.permissions().first().map(|p| p.run.clone()) {
        interrupt_run(session, client, run);
    }
}

/// What `key` answers on the card, moving the selection or its detail if it
/// answers nothing.
fn card_decision(session: &mut Session, key: KeyEvent, plain: bool) -> Option<Decision> {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Enter if plain => Some(if session.permission_choice == 0 {
            Decision::Allow
        } else {
            Decision::Deny
        }),
        KeyCode::Char('y') if control => Some(Decision::Allow),
        KeyCode::Char('n') if control => Some(Decision::Deny),
        KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::BackTab => {
            session.permission_choice ^= 1;
            None
        }
        KeyCode::Char('1') => {
            session.permission_choice = 0;
            None
        }
        KeyCode::Char('2') => {
            session.permission_choice = 1;
            None
        }
        KeyCode::PageUp => {
            session.permission_scroll = session.permission_scroll.saturating_sub(DETAIL_PAGE);
            None
        }
        KeyCode::PageDown => {
            session.permission_scroll = session.permission_scroll.saturating_add(DETAIL_PAGE);
            None
        }
        _ => None,
    }
}

/// Answers the card on screen with `decision`.
fn answer_card(session: &mut Session, client: &Client, decision: Decision) {
    if let Some(command) = session.decide(decision) {
        send(session, client, command, false);
    }
}

fn panel_key(session: &mut Session, client: &Client, key: KeyEvent) {
    if is_cancel(key) {
        session.quit_until = None;
        if let Some(panel) = session.panel.take() {
            send(session, client, panel.command(&panel.spec.dismiss), false);
        }
        return;
    }
    if key.code == KeyCode::Enter {
        if let Some(command) = session
            .panel
            .as_ref()
            .and_then(crate::PanelState::selected_command)
        {
            send(session, client, command, false);
        }
        return;
    }
    let Some(panel) = &mut session.panel else {
        return;
    };
    match key.code {
        KeyCode::Up | KeyCode::BackTab => panel.navigate(true),
        KeyCode::Down | KeyCode::Tab => panel.navigate(false),
        KeyCode::PageUp => panel.scroll = panel.scroll.saturating_sub(PANEL_PAGE),
        KeyCode::PageDown => panel.scroll = panel.scroll.saturating_add(PANEL_PAGE).min(MAX_SCROLL),
        KeyCode::Home => panel.scroll = 0,
        _ => {}
    }
}

fn form_key(session: &mut Session, client: &Client, key: KeyEvent) {
    session.quit_until = None;
    if session.form_pending() {
        return;
    }
    if is_cancel(key) {
        session.form = None;
        return;
    }
    let Some(form) = &mut session.form else {
        return;
    };
    if !is_ctrl(key, 's') {
        form.key(key);
        return;
    }
    match form.submission() {
        Ok(command) => send(session, client, command, false),
        Err(error) => session.notice = error.to_string(),
    }
}

fn picker_key(session: &mut Session, client: &Client, key: KeyEvent) {
    session.quit_until = None;
    if is_cancel(key) {
        session.picker = None;
        return;
    }
    if key.code == KeyCode::Enter {
        if let Some(command) = session
            .picker
            .as_ref()
            .and_then(crate::ChoicePicker::selected_command)
        {
            send(session, client, command, false);
        }
        return;
    }
    let Some(picker) = &mut session.picker else {
        return;
    };
    let typing = !key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    match key.code {
        KeyCode::Up | KeyCode::BackTab => picker.navigate(true),
        KeyCode::Down | KeyCode::Tab => picker.navigate(false),
        KeyCode::Left => picker.filter.left(),
        KeyCode::Right => picker.filter.right(),
        KeyCode::Home => picker.filter.home(),
        KeyCode::End => picker.filter.end(),
        KeyCode::Backspace => {
            picker.filter.backspace();
            picker.selected = 0;
        }
        KeyCode::Delete => {
            picker.filter.delete();
            picker.selected = 0;
        }
        KeyCode::Char(c)
            if typing && picker.filter.text().len() + c.len_utf8() <= MAX_FILTER_BYTES =>
        {
            picker.filter.insert(&c.to_string());
            picker.selected = 0;
        }
        _ => {}
    }
}

fn sidebar_key(session: &mut Session, client: &Client, key: KeyEvent) {
    if key.code == KeyCode::Enter {
        let command = session
            .sidebar
            .as_ref()
            .and_then(|s| s.actions().nth(s.selected))
            .map(|a| Command::Invoke {
                name: a.command.clone(),
                arguments: a.arguments.clone(),
            });
        if let Some(sidebar) = &mut session.sidebar {
            sidebar.focused = false;
        }
        if let Some(command) = command {
            send(session, client, command, false);
        }
        return;
    }
    let Some(sidebar) = &mut session.sidebar else {
        return;
    };
    match key.code {
        _ if is_cancel(key) => sidebar.focused = false,
        KeyCode::Up | KeyCode::BackTab => sidebar.navigate(true, 1),
        KeyCode::Down | KeyCode::Tab => sidebar.navigate(false, 1),
        KeyCode::PageUp | KeyCode::PageDown => {
            sidebar.scroll = if key.code == KeyCode::PageUp {
                sidebar.scroll.saturating_sub(PANEL_PAGE)
            } else {
                sidebar.scroll.saturating_add(PANEL_PAGE).min(MAX_SCROLL)
            };
            // Paging can browse sections with no actions. Enter must not
            // activate a previously selected link that is now offscreen.
            sidebar.selected = usize::MAX;
        }
        KeyCode::Home => {
            sidebar.selected = 0;
            sidebar.scroll = 0;
        }
        KeyCode::End => sidebar.selected = sidebar.actions().count().saturating_sub(1),
        _ => {}
    }
}
