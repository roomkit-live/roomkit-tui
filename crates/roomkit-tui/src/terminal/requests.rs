//! Requests leaving the view, and text pasted into it.

use std::sync::mpsc::TrySendError;

use roomkit_tui_protocol::{Command, RequestId, RunId};

use crate::session::Focus;
use crate::{Client, Session};
/// Pasted text goes to the focused editor; it never answers a decision or
/// activates a panel or sidebar action.
pub(super) fn paste(session: &mut Session, text: &str) {
    if session.images_pending() {
        return;
    }
    match session.focus() {
        Focus::Question => {
            if let Some(card) = session.question_mut() {
                card.insert(text);
            }
        }
        Focus::Form => {
            if !session.form_pending()
                && let Some(form) = &mut session.form
            {
                form.insert(text);
            }
        }
        Focus::Picker => {
            if let Some(picker) = &mut session.picker
                && picker.filter.text().len().saturating_add(text.len()) <= 256
            {
                picker.filter.insert(&text.replace(['\n', '\r'], " "));
                picker.selected = 0;
            }
        }
        Focus::Composer => {
            if session.editor.expanded_len().saturating_add(text.len())
                <= roomkit_tui_protocol::limits::MAX_TEXT_BYTES
            {
                session.paste(text);
            } else {
                session.notice = "Input exceeds the 64 KiB composer limit".into();
            }
        }
        _ => {}
    }
}

pub(super) fn interrupt(session: &mut Session, client: &Client) {
    if let Some(run) = session.active_run().cloned() {
        interrupt_run(session, client, run);
    }
}

/// Interrupt `run`, unless an interruption is already on its way.
pub(super) fn interrupt_run(session: &mut Session, client: &Client, run: RunId) {
    if !session.interrupting() {
        send(session, client, Command::Interrupt { run }, false);
    }
}

pub(super) fn send(session: &mut Session, client: &Client, command: Command, clear_input: bool) {
    let keep_images = matches!(&command, Command::SubmitImages { .. });
    if try_request(session, client, command).is_none() {
        return;
    }
    if clear_input && !keep_images {
        session.editor.submitted();
        session.draft_epoch = session.draft_epoch.wrapping_add(1);
        session.scroll_to_bottom();
    }
    session.notice.clear();
}

/// Send `command`, the view as it is: its id once the connection took it.
pub(super) fn try_request(
    session: &mut Session,
    client: &Client,
    command: Command,
) -> Option<RequestId> {
    let request = match session.request(command) {
        Ok(request) => request,
        Err(error) => {
            session.notice = error.to_string();
            return None;
        }
    };
    match client.try_send(request.clone()) {
        Ok(()) => {
            session.sent(&request);
            Some(request.id)
        }
        Err(TrySendError::Full(_)) => {
            session.notice = "Host is busy; input preserved, retry shortly".into();
            None
        }
        Err(TrySendError::Disconnected(_)) => {
            session.notice = "Host disconnected; input preserved".into();
            None
        }
    }
}
