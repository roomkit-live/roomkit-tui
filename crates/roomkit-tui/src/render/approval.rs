//! A focused decision card and host-confirmed history receipts.

use roomkit_tui_protocol::{Decision, Permission, PermissionDecision};

use super::card::{card_screen, enlarge};
use super::footer::quit_hint;
use super::text::{format_duration, pad, wrap_words};
use super::{Frame, Line, PermissionTarget, Tone, View, frame, line, span};
use crate::Session;

const CHOICES: [(&str, Decision); 2] = [
    ("1. Allow once", Decision::Allow),
    ("2. Deny", Decision::Deny),
];

pub(super) fn screen(view: &View<'_>, cols: usize, rows: usize) -> Frame {
    let session = view.session;
    let Some(permission) = session.permissions().first() else {
        return frame(Vec::new(), cols, None);
    };
    let waiting = session.deciding(&permission.id);
    let armed = session.approval_armed(view.elapsed);
    let detail = wrap_words(&permission.detail, cols - 6);
    let mut card = heading(view, permission, cols);
    // Rows beyond a one-line title come from the detail window.
    let title_rows = card.len().saturating_sub(4);
    let count = rows
        .saturating_sub(13 + title_rows)
        .clamp(1, 8)
        .min(detail.len().max(1));
    let start = session
        .permission_scroll
        .min(detail.len().saturating_sub(count));
    let filled = |text: &str| line(pad(&format!("  {text}"), cols), Tone::Input);
    card.extend(
        detail
            .iter()
            .skip(start)
            .take(count)
            .map(|text| filled(text)),
    );
    card.push(filled(""));
    let choice_start = card.len();
    card.extend(choices(session, cols, waiting, armed));
    let shown = (start + count).min(detail.len());
    let help = quit_hint(session, view.elapsed).unwrap_or_else(|| {
        format!(
            "PgUp/PgDn details {shown}/{} · {}",
            detail.len(),
            cancel_hint(session)
        )
    });
    card.push(filled(&help));
    card.push(filled(""));
    // Never show part of what a person is asked to approve.
    let Some(placed) = card_screen(view, cols, rows, card, None) else {
        return enlarge("Enlarge terminal to review this request", cols);
    };
    let mut result = placed.frame;
    result.permission_scroll = Some(start);
    result.approval = Some(permission.id.clone());
    if !waiting {
        result.permission_targets = targets(session, permission, placed.top + choice_start, cols);
    }
    result
}

/// What Esc does on the card, and whether more requests wait behind it.
fn cancel_hint(session: &Session) -> String {
    let cancel = if session.permission_elsewhere().is_some() {
        "Esc refuse · o open its room"
    } else if session.card_refuses_on_cancel() {
        "Esc refuse"
    } else {
        "Esc interrupt"
    };
    let more = if session.permissions().len() > 1 {
        " · more requests pending"
    } else {
        ""
    };
    format!("{cancel}{more}")
}

/// The click targets of the answers, whose first row is screen row `row`.
fn targets(
    session: &Session,
    permission: &Permission,
    row: usize,
    cols: usize,
) -> Vec<PermissionTarget> {
    let Some(snapshot) = &session.snapshot else {
        return Vec::new();
    };
    CHOICES
        .into_iter()
        .enumerate()
        .map(|(index, (_, decision))| PermissionTarget {
            conversation: snapshot.conversation.clone(),
            permission: permission.id.clone(),
            decision,
            row: row + index,
            start: 0,
            end: cols,
        })
        .collect()
}

/// The two answers, highlighted only while an answer would be accepted,
/// and the keys that give one.
fn choices(session: &Session, cols: usize, waiting: bool, armed: bool) -> Vec<Line> {
    let mut rows: Vec<Line> = CHOICES
        .iter()
        .enumerate()
        .map(|(index, (label, _))| {
            let selected = index == session.permission_choice;
            let marker = if selected { "›" } else { " " };
            let tone = if selected && !waiting && armed {
                Tone::Selected
            } else {
                Tone::Input
            };
            let mut row = line(pad(&format!("{marker} {label}"), cols), tone);
            row[0].emphasis.bold = selected;
            row
        })
        .collect();
    let hint = if waiting {
        "Waiting for host decision…"
    } else if !armed {
        "Review the request before deciding"
    } else {
        "↑↓ choose · Enter confirm · Ctrl+Y allow once · Ctrl+N deny"
    };
    rows.push(line(pad(&format!("  {hint}"), cols), Tone::Input));
    rows
}

/// Title, requester and waiting time above the details.
fn heading(view: &View<'_>, permission: &Permission, cols: usize) -> Vec<Line> {
    let session = view.session;
    let filled = |text: &str| line(pad(&format!("  {text}"), cols), Tone::Input);
    // The whole title is shown, wrapped and with its line breaks.
    let title = wrap_words(&permission.title, cols - 4)
        .into_iter()
        .map(|text| {
            let mut row = filled(&text);
            row[0].emphasis.bold = true;
            row
        });
    let duration = session
        .run_elapsed(view.elapsed)
        .map(|d| format!(" · {}", format_duration(d, false)))
        .unwrap_or_default();
    let mode = session
        .snapshot
        .as_ref()
        .and_then(|s| s.mode.as_ref())
        .and_then(|m| m.choices.iter().find(|c| c.value == m.selected))
        .map(|c| format!("[{}] · ", c.label))
        .unwrap_or_default();
    let requester = session.actor_name(&permission.actor);
    let mut lines = vec![filled("")];
    lines.extend(title);
    lines.push(filled(&format!(
        "{mode}{requester} · Waiting for your decision{duration}"
    )));
    lines.push(filled(""));
    lines
}

pub(super) fn receipt(session: &Session, record: &PermissionDecision, cols: usize) -> Vec<Line> {
    let (symbol, action, tone) = match record.decision {
        Decision::Allow => ("✓", "approved", Tone::Success),
        _ => ("⊘", "denied", Tone::Warning),
    };
    let summary = record
        .permission
        .summary
        .as_deref()
        .unwrap_or(&record.permission.title);
    let text = format!(
        "{} {action} {} · {} · this time",
        session.actor_name(&record.decided_by),
        session.actor_name(&record.permission.actor),
        summary
    );
    wrap_words(&text, cols.saturating_sub(4))
        .into_iter()
        .enumerate()
        .map(|(i, text)| {
            let marker = if i == 0 {
                format!(" {symbol} ")
            } else {
                "   ".into()
            };
            vec![span(marker, tone), span(text, tone)]
        })
        .collect()
}
