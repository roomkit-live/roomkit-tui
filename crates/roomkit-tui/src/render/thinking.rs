//! A model's reasoning: its last lines while it thinks, one line once it ended,
//! opened and closed as a call's details are.

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
    let mut laid = Block::default();
    laid.lines
        .push(header(state, expanded, focused, shown.truncated));
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

/// What a block shows under its header: while it is written its newest lines,
/// more of them once opened; ended, its first lines once opened; else nothing.
fn shown_lines(block: &Thinking, state: ThoughtState, expanded: bool, width: usize) -> Shown {
    let live = matches!(state, ThoughtState::Open(_));
    if live {
        let rows = if expanded { OPEN_LINES } else { LIVE_LINES };
        let lines = wrapped(tail(&block.text, rows, width), width);
        let skip = lines.len().saturating_sub(rows);
        return Shown {
            truncated: skip > 0 && expanded,
            lines: lines.into_iter().skip(skip).collect(),
        };
    }
    if !expanded {
        return Shown {
            lines: Vec::new(),
            truncated: false,
        };
    }
    let lines = wrapped(&block.text, width);
    Shown {
        truncated: lines.len() > OPEN_LINES,
        lines: lines.into_iter().take(OPEN_LINES).collect(),
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

fn header(state: ThoughtState, expanded: bool, focused: bool, truncated: bool) -> Line {
    let label = match state {
        ThoughtState::Open(Some(took)) => format!("∴ Thinking · {}", format_duration(took, false)),
        ThoughtState::Open(None) => "∴ Thinking".to_owned(),
        ThoughtState::Ended(Some(took)) => {
            format!("∴ Thought for {}", format_duration(took, true))
        }
        ThoughtState::Ended(None) => "∴ Thought".to_owned(),
    };
    let ended = matches!(state, ThoughtState::Ended(_));
    let hint = match (expanded, truncated) {
        (true, true) => " · F2 collapse · view limited to 100 lines",
        (true, false) => " · F2 collapse",
        // While it is written its lines already show: nothing to offer yet.
        (false, _) if focused && ended => " · F2 expand",
        (false, _) => "",
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
