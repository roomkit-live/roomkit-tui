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
/// How much of the end of a block is laid out while it is written, in bytes: a
/// long reasoning costs what the screen shows.
const LIVE_TAIL_BYTES: usize = 4096;
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

/// What a block shows under its header: its last lines while it is written, all
/// of them up to the bound once opened, nothing otherwise.
fn shown_lines(block: &Thinking, state: ThoughtState, expanded: bool, width: usize) -> Shown {
    let live = matches!(state, ThoughtState::Open(_));
    if !live && !expanded {
        return Shown {
            lines: Vec::new(),
            truncated: false,
        };
    }
    let text = if live && !expanded {
        tail(&block.text)
    } else {
        block.text.as_str()
    };
    let lines: Vec<String> = sanitize(text.trim())
        .lines()
        .flat_map(|paragraph| wrap_words(paragraph, width))
        .collect();
    if live && !expanded {
        let skip = lines.len().saturating_sub(LIVE_LINES);
        return Shown {
            lines: lines.into_iter().skip(skip).collect(),
            truncated: false,
        };
    }
    let truncated = lines.len() > OPEN_LINES;
    Shown {
        lines: lines.into_iter().take(OPEN_LINES).collect(),
        truncated,
    }
}

/// The end of `text`, on a character boundary.
fn tail(text: &str) -> &str {
    let mut start = text.len().saturating_sub(LIVE_TAIL_BYTES);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
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
    let hint = match (focused, expanded, truncated) {
        (_, true, true) => " · F2 collapse · view limited to 100 lines",
        (_, true, false) => " · F2 collapse",
        (true, false, _) => " · F2 expand",
        (false, false, _) => "",
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
