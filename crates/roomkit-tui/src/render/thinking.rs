//! A model's reasoning: its last lines while it thinks, one line once it ended,
//! opened and closed as a call's details are. Opened, it shows its newest lines
//! too: past the bound a block holds only its end, and it does not jump as it ends.

use roomkit_tui_protocol::{Snapshot, Thinking};
use unicode_width::UnicodeWidthStr;

use super::text::{format_duration, sanitize, wrap_words};
use super::timeline::Block;
use super::{Line, Tone, ToolTarget, View, line, span};
use crate::session::ThoughtState;

/// The lines of a block the model is still writing.
const LIVE_LINES: usize = 3;
/// The lines an opened block shows, as a call's details are limited.
const OPEN_LINES: usize = 100;
/// The most bytes a character of a row takes.
const MAX_CHAR_BYTES: usize = 4;
const INDENT: &str = "     ";

pub(super) fn block(view: &View<'_>, snapshot: &Snapshot, block: &Thinking, cols: usize) -> Block {
    let session = view.session;
    let (actor, run, id) = (&block.actor, &block.run, &block.id);
    let focused = session.tool_focused(actor, run, id);
    let state = session.thought_state(block, view.elapsed);
    let expanded = session.tool_expanded(actor, run, id);
    let width = cols.saturating_sub(INDENT.width()).max(1);
    let shown = shown_lines(block, state, expanded, width);
    // F2 opens and closes everything in the activity view, what has the focus else.
    let f2_reaches = focused || (expanded && view.options.tool_activity);
    let hint = hint(state, expanded, f2_reaches, shown.truncated);
    let mut laid = Block::default();
    laid.lines.push(header(state, focused, hint));
    if view.options.selection == crate::SelectionMode::CopyOnSelect {
        let end = columns_of(&laid.lines[0]).min(cols);
        laid.targets.push(ToolTarget {
            conversation: snapshot.conversation.clone(),
            row: 0,
            start: 3,
            end,
            actor: actor.clone(),
            run: run.clone(),
            call: id.clone(),
        });
    }
    laid.lines.extend(
        shown
            .lines
            .into_iter()
            .map(|text| line(format!("{INDENT}{text}"), Tone::Muted)),
    );
    laid.focused_end = focused.then_some(laid.lines.len());
    laid
}

struct Shown {
    lines: Vec<String>,
    truncated: bool,
}

/// What a block shows under its header: its newest lines, more of them once
/// opened, and nothing once it ended folded.
fn shown_lines(block: &Thinking, state: ThoughtState, expanded: bool, width: usize) -> Shown {
    if matches!(state, ThoughtState::Ended(_)) && !expanded {
        return Shown {
            lines: Vec::new(),
            truncated: false,
        };
    }
    let rows = if expanded { OPEN_LINES } else { LIVE_LINES };
    let end = tail(&block.text, rows, width);
    let lines = wrapped(end, width);
    let skip = lines.len().saturating_sub(rows);
    Shown {
        truncated: expanded && (skip > 0 || end.len() < block.text.len()),
        lines: lines.into_iter().skip(skip).collect(),
    }
}

fn wrapped(text: &str, width: usize) -> Vec<String> {
    sanitize(text.trim())
        .lines()
        .flat_map(|paragraph| wrap_words(paragraph, width))
        .collect()
}

/// As much of the end of `text` as `rows` rows of `width` columns can show: a
/// long reasoning costs what the screen shows.
fn tail(text: &str, rows: usize, width: usize) -> &str {
    let bytes = rows.saturating_mul(width.saturating_mul(MAX_CHAR_BYTES) + 1);
    &text[text.ceil_char_boundary(text.len().saturating_sub(bytes))..]
}

/// What the header offers: the F2 that really answers, then the view's limit.
fn hint(state: ThoughtState, expanded: bool, f2_reaches: bool, truncated: bool) -> &'static str {
    let ended = matches!(state, ThoughtState::Ended(_));
    match (expanded, f2_reaches, truncated) {
        (true, true, true) => " · F2 collapse · view limited to the last 100 lines",
        (true, true, false) => " · F2 collapse",
        (true, false, true) => " · view limited to the last 100 lines",
        // While it is written its lines already show: nothing to offer yet.
        (false, true, _) if ended => " · F2 expand",
        _ => "",
    }
}

fn header(state: ThoughtState, focused: bool, hint: &str) -> Line {
    let label = match state {
        ThoughtState::Open(Some(took)) => format!("∴ Thinking · {}", format_duration(took, false)),
        ThoughtState::Open(None) => "∴ Thinking".to_owned(),
        ThoughtState::Ended(Some(took)) => {
            format!("∴ Thought for {}", format_duration(took, true))
        }
        ThoughtState::Ended(None) => "∴ Thought".to_owned(),
    };
    vec![
        span(if focused { " › " } else { "   " }, Tone::Accent),
        span(label, Tone::Muted),
        span(hint, Tone::Muted),
    ]
}

fn columns_of(row: &Line) -> usize {
    row.iter().map(|part| part.text.width()).sum()
}
