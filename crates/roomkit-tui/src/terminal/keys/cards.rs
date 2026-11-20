//! Keys on the cards that ask the person something: a decision, and the
//! questions an agent asks. A card takes an answer only once it can have
//! been read, and never one the screen does not show.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use roomkit_tui_protocol::{Command, Decision};

use super::{is_cancel, is_ctrl, is_quit_key};
use crate::render::Frame;
use crate::session::Focus;
use crate::terminal::{interrupt_run, send};
use crate::{Client, Session};

const DETAIL_PAGE: usize = 3;

/// Whether `key` would act on something `frame` does not show. An undersized
/// screen draws no input, approval or question: a key must not answer what
/// the person cannot see. A question card shown whole has neither an input
/// cursor nor approval targets, and still takes its keys.
pub(in crate::terminal) fn acts_unseen(frame: &Frame, session: &Session, key: KeyEvent) -> bool {
    let question = session.focus() == Focus::Question;
    if frame.cursor.is_some()
        || !frame.permission_targets.is_empty()
        || (question && frame.question.is_some())
    {
        return false;
    }
    // Every key answers a question card, Esc included; only quitting stays.
    // The card's arming refuses them too: this keeps them from reaching it.
    if question {
        return !is_quit_key(key);
    }
    matches!(
        key.code,
        KeyCode::Enter | KeyCode::Tab | KeyCode::Up | KeyCode::Down
    ) || ['y', 'n', 's'].into_iter().any(|c| is_ctrl(key, c))
        || (session.card_refuses_on_cancel()
            && !session.permissions().is_empty()
            && matches!(key.code, KeyCode::Esc | KeyCode::Char('o')))
}

/// A decision is accepted only once the card has been on screen long enough
/// to be read; navigation and interruption stay immediate.
pub(super) fn permission_key(session: &mut Session, client: &Client, key: KeyEvent, now: Duration) {
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

/// Keys to the questions on screen, once they can have been read: a key
/// pressed as they appear answers nothing. Ctrl+C acts as on a decision
/// card ([`cancel_questions`]).
pub(super) fn question_key(session: &mut Session, client: &Client, key: KeyEvent, now: Duration) {
    session.quit_until = None;
    if is_ctrl(key, 'c') {
        cancel_questions(session, client, now);
        return;
    }
    if key.kind == KeyEventKind::Repeat || !session.question_armed(now) {
        return;
    }
    let Some(card) = session.question_mut() else {
        return;
    };
    let command = match card.key(key) {
        crate::question::CardStep::Stay => return,
        crate::question::CardStep::Answer(command)
        | crate::question::CardStep::Decline(command) => command,
    };
    send_answers(session, client, command);
}

/// Ctrl+C on the questions, as on a decision card: where stopping their run
/// would stop more than the turn, it declines them, once they can have been
/// read; else it interrupts the run that asks.
fn cancel_questions(session: &mut Session, client: &Client, now: Duration) {
    if session.card_refuses_on_cancel() {
        let decline = session
            .question()
            .filter(|card| !card.sent)
            .map(crate::QuestionCard::decline);
        if let Some(command) = decline.filter(|_| session.question_armed(now)) {
            send_answers(session, client, command);
        }
        return;
    }
    if let Some(run) = session.question().map(|card| card.question().run.clone()) {
        interrupt_run(session, client, run);
    }
}

/// Send the card's answers, or its refusal: the card then takes no key
/// until the host replies.
fn send_answers(session: &mut Session, client: &Client, command: Command) {
    if crate::terminal::requests::try_request(session, client, command).is_some() {
        if let Some(card) = session.question_mut() {
            card.sent = true;
        }
        session.notice.clear();
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
