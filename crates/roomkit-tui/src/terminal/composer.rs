//! Keys for the composer: editing, history, completions and the timeline.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::keys::{Outcome, is_cancel, is_ctrl};
use super::requests::try_request;
use super::{QUIT_TIMEOUT, interrupt, interrupt_run, send};
use crate::{Client, Session};

const MAX_INPUT_BYTES: usize = roomkit_tui_protocol::limits::MAX_TEXT_BYTES;
const PAGE: usize = 8;
pub(super) fn composer_key(
    session: &mut Session,
    client: &Client,
    key: KeyEvent,
    now: Duration,
    tool_activity: bool,
) -> Outcome {
    if session.images_pending() && !is_cancel(key) {
        return Outcome::Stay;
    }
    if is_ctrl(key, 'c') {
        if session.quit_until.is_some() {
            return Outcome::Exit;
        }
        session.clear_composer();
        session.menu.dismiss();
        interrupt(session, client);
        session.quit_until = Some(now.saturating_add(QUIT_TIMEOUT));
        session.quit_key = "Ctrl+C";
        return Outcome::Stay;
    }
    if !composer_navigation(session, client, key, tool_activity) {
        composer_editing(session, client, key);
    }
    session.refresh_menu();
    Outcome::Stay
}

/// Keys that act on the menu, the history or the timeline rather than the
/// draft. Returns whether the key was handled.
fn composer_navigation(
    session: &mut Session,
    client: &Client,
    key: KeyEvent,
    tool_activity: bool,
) -> bool {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let menu = session.menu.visible;
    let multiline = session.editor.text().contains('\n');
    match key.code {
        KeyCode::Right | KeyCode::Left if alt && session.editor.text().is_empty() => {
            if let Some(command) = session.cycle_room(key.code == KeyCode::Right) {
                send(session, client, command, false);
            }
        }
        KeyCode::Up if alt => session.focus_tool(true),
        KeyCode::Down if alt => session.focus_tool(false),
        KeyCode::Up | KeyCode::BackTab if menu => session.menu.navigate(true),
        KeyCode::Down if menu => session.menu.navigate(false),
        KeyCode::BackTab => {
            if let Some(command) = session.cycle_mode() {
                send(session, client, command, false);
            }
        }
        KeyCode::Up if multiline => session.editor.vertical(true),
        KeyCode::Down if multiline => session.editor.vertical(false),
        KeyCode::Up => session.editor.history_previous(),
        KeyCode::Down => session.editor.history_next(),
        KeyCode::Char('p') if control => session.editor.history_previous(),
        KeyCode::Char('n') if control => session.editor.history_next(),
        KeyCode::Tab => session.complete(),
        KeyCode::Esc if menu => session.menu.dismiss(),
        KeyCode::Esc if session.sends_after_interruption() => session.keep_draft(),
        KeyCode::Esc => interrupt(session, client),
        KeyCode::End if control => session.scroll_to_bottom(),
        KeyCode::PageUp => session.scroll_back(PAGE),
        KeyCode::PageDown => session.scroll_forward(PAGE),
        KeyCode::F(2) if tool_activity => session.toggle_activity(),
        KeyCode::F(2) if shift => session.toggle_all_details(),
        KeyCode::F(2) => session.toggle_tool(),
        _ => return false,
    }
    true
}

fn composer_editing(session: &mut Session, client: &Client, key: KeyEvent) {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let room = |extra: usize| session.editor.text().len() + extra <= MAX_INPUT_BYTES;
    match key.code {
        KeyCode::Enter
            if key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
        {
            if room(1) {
                session.editor.insert("\n");
            }
        }
        KeyCode::Char('j') if control => {
            if room(1) {
                session.editor.insert("\n");
            }
        }
        KeyCode::Enter | KeyCode::Char('s') if control => send_now(session, client),
        KeyCode::Enter => submit(session, client),
        KeyCode::Char(c) if !control && !alt => {
            if room(c.len_utf8()) {
                session.editor.insert(&c.to_string());
            }
        }
        KeyCode::Backspace
            if alt || (session.editor.text().is_empty() && !session.images.is_empty()) =>
        {
            session.remove_last_image();
        }
        KeyCode::Backspace => session.editor.backspace(),
        KeyCode::Delete => session.editor.delete(),
        KeyCode::Left => session.editor.left(),
        KeyCode::Right => session.editor.right(),
        // Ctrl+A and Ctrl+E as in a shell: the start and end of the line.
        KeyCode::Home => session.editor.home(),
        KeyCode::Char('a') if control => session.editor.home(),
        KeyCode::End => session.editor.end(),
        KeyCode::Char('e') if control => session.editor.end(),
        _ => {}
    }
}

/// Ctrl+S, or Ctrl+Enter where the terminal tells it from Enter: Enter for a
/// command. A message cuts whatever runs, as Esc does, and leaves once
/// nothing runs any more, however the host holds the room ([`send_waiting`]).
/// With an empty composer, it does so for the message handed to the turn.
fn send_now(session: &mut Session, client: &Client) {
    let command = session
        .draft()
        .trim_start()
        .starts_with(session.command_prefix)
        || session.input_action().is_some();
    let can_cut = session
        .snapshot
        .as_ref()
        .is_none_or(|snapshot| snapshot.can_cut_to_send);
    if command || !can_cut {
        submit(session, client);
        return;
    }
    match session.send_after_interruption() {
        Ok(()) => {
            send_waiting(session, client, Duration::ZERO);
        }
        Err(error) => session.notice = error.to_string(),
    }
}

/// Move the waiting draft on: cut a run that started since, or send the
/// draft once nothing runs any more. It stays in the composer until the host
/// takes it. Whether it acted.
pub(super) fn send_waiting(session: &mut Session, client: &Client, now: Duration) -> bool {
    if let Some(run) = session.run_to_cut() {
        interrupt_run(session, client, run);
        return true;
    }
    if !session.draft_may_leave(now) {
        return false;
    }
    match session.waited_command() {
        Ok(command) => match try_request(session, client, command) {
            Some(id) => session.sent_waited(id),
            None => session.keep_draft(),
        },
        Err(error) => {
            session.notice = error.to_string();
            session.keep_draft();
        }
    }
    true
}

/// Enter sends the draft. With completions open, an exact command without
/// choices is sent; anything else accepts the highlighted completion.
fn submit(session: &mut Session, client: &Client) {
    // This draft already leaves as soon as it can.
    if session.sends_after_interruption() {
        return;
    }
    if session.menu.visible && !session.menu.entries.is_empty() {
        match session
            .input_command()
            .ok()
            .filter(|command| session.is_complete_invocation(command))
        {
            Some(command) => send(session, client, command, true),
            None => {
                session.accept_completion();
            }
        }
        return;
    }
    match session.input_command() {
        Ok(command) => send(session, client, command, true),
        Err(error) => session.notice = error.to_string(),
    }
}
