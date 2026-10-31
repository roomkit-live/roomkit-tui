//! The conversation screen: header, history, activity row, composer, footer.

use unicode_width::UnicodeWidthStr;

use super::chrome;
use super::text::{fit, input_gutter, pad, sanitize, wrap};
use super::timeline::{Origin, Timeline, timeline};
use super::{
    Frame, Line, TimelineRows, Tone, ToolTarget, View, approval, footer, form, frame, line, menus,
    panel, span, welcome,
};
use crate::session::Focus;

#[derive(Clone, Copy)]
pub(super) enum Layout {
    Conversation { sidebar: bool },
    Welcome,
}

/// The draft wrapped to the composer, with its caret as `(column, row)`.
struct Input {
    lines: Vec<String>,
    caret: (usize, usize),
    gutter: String,
    width: usize,
    hidden_prefix: usize,
}

/// Rows given to the composer once the rest of the screen is placed.
struct Rows {
    input: usize,
    padding: usize,
    gap: usize,
    /// Blank rows centering a short draft in the composer.
    centering: usize,
    /// First draft line shown, keeping the caret visible.
    first: usize,
}

pub(super) fn screen(view: &View<'_>, width: u16, height: u16, layout: Layout) -> Frame {
    let (cols, rows) = (usize::from(width), usize::from(height));
    let session = view.session;
    let overlay = session.focus() != Focus::Composer && session.focus() != Focus::Sidebar
        || session.menu.visible;
    if cols < 12 || rows < 8 || (overlay && (cols < 32 || rows < 16)) {
        let lines = wrap("Enlarge terminal", cols).into_iter().take(rows);
        return frame(
            lines.map(|text| line(text, Tone::Muted)).collect(),
            cols,
            None,
        );
    }
    match session.focus() {
        Focus::Permission => return approval::screen(view, cols, rows),
        Focus::Panel => return panel::screen(session, cols, rows, view.elapsed),
        Focus::Form => return form::screen(session, cols, rows, view.elapsed),
        _ => {}
    }
    let welcome = matches!(layout, Layout::Welcome);
    let header = if welcome {
        Vec::new()
    } else {
        chrome::header(view, cols, rows)
    };
    let input = input(view, cols);
    let footer = if welcome {
        vec![welcome::footer(session, cols, view.elapsed)]
    } else {
        let sidebar = matches!(layout, Layout::Conversation { sidebar: true });
        footer::footer(session, cols, sidebar, view.elapsed)
    };
    let attachments = chrome::attachments(view, cols);
    let selector_rows = if session.picker.is_some() {
        5
    } else if session.menu.visible {
        4
    } else {
        0
    };
    let fixed = header.len() + footer.len() + 2 + selector_rows + attachments.len();
    let allocation = allocate(view, &input, rows.saturating_sub(fixed).max(1));
    let available = rows.saturating_sub(
        header.len()
            + allocation.input
            + footer.len()
            + 1
            + allocation.gap
            + 2 * allocation.padding
            + attachments.len(),
    );
    let mut bottom = selector(view, cols, available);
    let activity_row = bottom.len();
    if !welcome {
        bottom.push(chrome::activity(view));
        bottom.extend((0..allocation.gap).map(|_| Vec::new()));
    }
    bottom.extend(attachments);
    let input_row = bottom.len() + allocation.padding;
    bottom.extend(composer(view, &input, &allocation, cols));
    bottom.extend(footer);
    let parts = Parts {
        header,
        bottom,
        activity_row: (!welcome).then_some(activity_row),
        input_row: input_row + allocation.centering,
    };
    assemble(view, parts, &input, &allocation, (cols, rows))
}

/// Laid-out pieces of the conversation screen, before the history fills it.
struct Parts {
    header: Vec<Line>,
    bottom: Vec<Line>,
    activity_row: Option<usize>,
    /// Row of the first draft line within `bottom`.
    input_row: usize,
}

fn assemble(
    view: &View<'_>,
    mut parts: Parts,
    input: &Input,
    allocation: &Rows,
    (cols, rows): (usize, usize),
) -> Frame {
    let session = view.session;
    let space = rows.saturating_sub(parts.header.len() + parts.bottom.len());
    let history = match parts.activity_row {
        Some(_) => timeline(view, cols, space, session.scroll),
        None => Timeline::default(),
    };
    let (start, end) = visible_range(view, &history, space);
    let bottom_start = rows.saturating_sub(parts.bottom.len());
    let mut result_targets = (None, Vec::new());
    if let Some(row) = parts.activity_row {
        let more_below = end < history.lines.len();
        result_targets = chrome::controls(
            view,
            &mut parts.bottom[row],
            cols,
            bottom_start + row,
            more_below,
        );
    }
    let offset = parts.header.len();
    let targets = shown_targets(history.targets, (start, end), offset);
    let unseen_below = history.lines.len() - end;
    let conversation =
        conversation_rows(view, &history.origins, (start, end), (offset, space), cols);
    let mut lines = parts.header;
    lines.extend(history.lines.into_iter().skip(start).take(end - start));
    lines.resize_with(
        rows.saturating_sub(parts.bottom.len()).max(lines.len()),
        Vec::new,
    );
    let cursor_row = lines.len() + parts.input_row + input.caret.1.saturating_sub(allocation.first);
    lines.extend(parts.bottom);
    lines.truncate(rows);
    let mut result = frame(
        lines,
        cols,
        cursor(view, input, cols, cursor_row, rows, bottom_start),
    );
    result.tool_targets = targets;
    (result.back_to_bottom, result.action_targets) = result_targets;
    result.scroll = (!session.has_tool_focus()).then_some(unseen_below);
    result.timeline = parts.activity_row.and(conversation);
    result
}

/// The tool targets on the history lines from `start` to `end`, moved to
/// their screen rows below a header `offset` rows high.
fn shown_targets(
    targets: Vec<ToolTarget>,
    (start, end): (usize, usize),
    offset: usize,
) -> Vec<ToolTarget> {
    targets
        .into_iter()
        .filter(|target| (start..end).contains(&target.row))
        .map(|mut target| {
            target.row = offset + target.row - start;
            target
        })
        .collect()
}

/// The conversation area shown from screen row `top`, `height` rows high, and
/// the origins of the history lines from `start` to `end` displayed in it.
fn conversation_rows(
    view: &View<'_>,
    origins: &[Origin],
    (start, end): (usize, usize),
    (top, height): (usize, usize),
    width: usize,
) -> Option<TimelineRows> {
    let snapshot = view.session.snapshot.as_ref()?;
    Some(TimelineRows {
        conversation: snapshot.conversation.clone(),
        top,
        height,
        width,
        origins: origins.get(start..end).unwrap_or_default().to_vec(),
    })
}

/// Rows of the history to show: following the latest activity, browsing
/// back by the session's scroll, or keeping the focused tool in view.
fn visible_range(view: &View<'_>, history: &Timeline, space: usize) -> (usize, usize) {
    let len = history.lines.len();
    let end = if view.session.has_tool_focus() && view.session.scroll == 0 {
        history.focused_end.unwrap_or(len).max(space).min(len)
    } else {
        len - view.session.scroll.min(len.saturating_sub(space))
    };
    (end.saturating_sub(space), end)
}

fn cursor(
    view: &View<'_>,
    input: &Input,
    cols: usize,
    row: usize,
    rows: usize,
    bottom_start: usize,
) -> Option<(u16, u16)> {
    let session = view.session;
    if let Some(picker) = &session.picker {
        let filter = fit(&picker.filter.text()[..picker.filter.cursor()], cols - 12);
        return Some(((10 + filter.width()) as u16, (bottom_start + 1) as u16));
    }
    (row < rows).then_some((
        if input.hidden_prefix > 0 && session.editor.cursor() == 0 {
            2
        } else {
            (input.caret.0 + 2 + input.gutter.width()).min(cols - 1) as u16
        },
        row as u16,
    ))
}

fn input(view: &View<'_>, cols: usize) -> Input {
    let session = view.session;
    let action = session.input_action();
    let prompt = action.map_or_else(
        || view.options.input_prompt.clone(),
        |action| action.prefix.to_string(),
    );
    let gutter = input_gutter(&prompt);
    let width = cols - 4 - gutter.width();
    // Keep the prefix in the editable draft/history, but show it once as the
    // mode marker. Layout and caret positions use the remaining visible text.
    let hidden_prefix = action.map_or(0, |action| action.prefix.len_utf8());
    let text = &session.editor.text()[hidden_prefix..];
    let cursor = session.editor.cursor().saturating_sub(hidden_prefix);
    let before = wrap(&sanitize(text.get(..cursor).unwrap_or_default()), width);
    let mut lines = wrap(&sanitize(text), width);
    let mut caret = (
        before.last().map_or(0, |line| line.width()),
        before.len().saturating_sub(1),
    );
    if caret.0 == width {
        caret = (0, caret.1 + 1);
        if lines.len() <= caret.1 {
            lines.push(String::new());
        }
    }
    Input {
        lines,
        caret,
        gutter,
        width,
        hidden_prefix,
    }
}

/// Keep activity, status, hints, selectors and one history row before any
/// optional padding. The caret always remains visible.
fn allocate(view: &View<'_>, input: &Input, budget: usize) -> Rows {
    let options = view.options;
    let count = input
        .lines
        .len()
        .clamp(1, 5)
        .max(usize::from(options.input_min_lines.clamp(1, 5)))
        .min(budget);
    let padding = usize::from(options.input_padding.min(2)).min((budget - count) / 2);
    Rows {
        input: count,
        padding,
        gap: usize::from(options.activity_gap).min(budget - count - 2 * padding),
        centering: count.saturating_sub(input.lines.len()) / 2,
        first: input.caret.1.saturating_sub(count - 1),
    }
}

fn selector(view: &View<'_>, cols: usize, available: usize) -> Vec<Line> {
    let session = view.session;
    if let Some(picker) = &session.picker {
        menus::choice_picker(picker, cols, available.min(14))
    } else if session.menu.visible && available >= 4 {
        menus::menu(session, cols, available.min(8))
    } else {
        Vec::new()
    }
}

fn composer(view: &View<'_>, input: &Input, rows: &Rows, cols: usize) -> Vec<Line> {
    let blank = || line(" ".repeat(cols), Tone::Input);
    let placeholder = view.session.editor.text().is_empty();
    let mut lines: Vec<Line> = (0..rows.padding).map(|_| blank()).collect();
    for row in 0..rows.input {
        let index = row.checked_sub(rows.centering).map(|row| rows.first + row);
        let Some(text) = index.and_then(|index| input.lines.get(index)) else {
            lines.push(blank());
            continue;
        };
        let (text, tone) = if placeholder {
            (view.options.input_placeholder.as_str(), Tone::Placeholder)
        } else {
            (text.as_str(), Tone::Input)
        };
        let gutter = if row == rows.centering {
            input.gutter.clone()
        } else {
            " ".repeat(input.gutter.width())
        };
        lines.push(vec![
            span("  ", Tone::Input),
            span(gutter, Tone::InputPrompt),
            span(pad(text, input.width), tone),
            span("  ", Tone::Input),
        ]);
    }
    lines.extend((0..rows.padding).map(|_| blank()));
    lines
}
