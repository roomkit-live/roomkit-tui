//! Optional host information beside the conversation, with its own scrolling
//! and keyboard navigation between actions.

use roomkit_tui_protocol::{SidebarAction, SidebarContent, SidebarEntry, SidebarTone};
use unicode_width::UnicodeWidthStr;

use super::text::wrap_words;
use super::{ActionTarget, Frame, Line, Span, Tone, frame, line, span};
use crate::session::Focus;
use crate::{Session, SidebarState, UiOptions};

/// Geometry of the visible sidebar; used for independent wheel scrolling.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SidebarArea {
    /// First sidebar column, including its separator.
    pub start: usize,
    /// Visible row count.
    pub height: usize,
    /// Clamped top content line after keyboard navigation.
    pub scroll: usize,
}

/// One sidebar row and the action it belongs to, if any.
struct Row {
    line: Line,
    action: Option<SidebarAction>,
    index: Option<usize>,
}

impl Row {
    fn text(line: Line) -> Self {
        Self {
            line,
            action: None,
            index: None,
        }
    }
}

pub(super) fn width(session: &Session, cols: u16, rows: u16, options: &UiOptions) -> Option<u16> {
    let width = options.sidebar_width.clamp(28, 48);
    (session.sidebar.is_some()
        && session.sidebar_visible.unwrap_or(options.sidebar_visible)
        && cols >= options.sidebar_min_width.max(width + 72)
        && rows >= 18
        && matches!(session.focus(), Focus::Composer | Focus::Sidebar))
    .then_some(width)
}

pub(super) fn compose(
    main: Frame,
    session: &Session,
    cols: usize,
    rows: usize,
    width: usize,
    monochrome: bool,
) -> Frame {
    let Some(state) = &session.sidebar else {
        return main;
    };
    let left = cols - width;
    let content = content(session, state, width - 4, monochrome);
    let space = rows - 4;
    let scroll = scroll(state, &content, space);
    let mut side = vec![line(&state.spec.title, Tone::Primary), Vec::new()];
    let mut targets = Vec::new();
    let mut cursor = None;
    for row in content.iter().skip(scroll).take(space) {
        let selected = state.focused && row.index == Some(state.selected);
        if let (Some(action), Some(snapshot)) = (&row.action, &session.snapshot) {
            targets.push(ActionTarget {
                conversation: snapshot.conversation.clone(),
                command: action.command.clone(),
                arguments: action.arguments.clone(),
                panel: None,
                sidebar: true,
                row: side.len(),
                start: left + 2,
                end: cols - 1,
            });
        }
        if selected {
            cursor.get_or_insert((left as u16 + 2, side.len() as u16));
        }
        side.push(highlight(&row.line, selected));
    }
    side.resize_with(rows - 2, Vec::new);
    side.extend(footer(state, content.len(), scroll, space));
    let mut result = join(main, frame(side, width - 4, None), left, rows);
    result.action_targets.extend(targets);
    result.sidebar = Some(SidebarArea {
        start: left,
        height: rows,
        scroll,
    });
    if state.focused {
        result.cursor = cursor.or(Some((left as u16 + 2, 0)));
    }
    result
}

fn content(session: &Session, state: &SidebarState, inner: usize, monochrome: bool) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut actions = 0;
    let mut next_action = |action: Option<&SidebarAction>| {
        action.map(|_| {
            actions += 1;
            actions - 1
        })
    };
    for section in &state.spec.sections {
        if matches!(section.content, SidebarContent::Unknown) {
            continue;
        }
        let marker = if section.action.is_some() { "› " } else { "" };
        rows.push(Row {
            line: vec![span(format!("{marker}{}", section.title), Tone::Primary).bold()],
            action: section.action.clone(),
            index: next_action(section.action.as_ref()),
        });
        let body = match &section.content {
            SidebarContent::Entries { entries } => {
                for entry in entries {
                    let index = next_action(entry.action.as_ref());
                    rows.extend(entry_rows(entry, index, inner));
                }
                Vec::new()
            }
            content => body(session, content, inner, monochrome),
        };
        rows.extend(
            crate::content::wrap(&body, inner, false)
                .into_iter()
                .map(Row::text),
        );
        rows.push(Row::text(Vec::new()));
    }
    rows
}

/// Lines of a section that projects conversation state or host measures.
/// Without colors, meter parts are told apart by glyphs.
fn body(session: &Session, content: &SidebarContent, inner: usize, monochrome: bool) -> Vec<Line> {
    match content {
        SidebarContent::Agent => agent(session),
        SidebarContent::Context => super::context::context(session, inner),
        SidebarContent::ContextBreakdown { parts, remainder } => {
            super::context::breakdown(session, parts, remainder, inner, monochrome)
        }
        SidebarContent::Meter { segments, capacity } => {
            super::meter::meter(segments, *capacity, inner, monochrome)
        }
        _ => Vec::new(),
    }
}

fn agent(session: &Session) -> Vec<Line> {
    let Some(snapshot) = &session.snapshot else {
        return Vec::new();
    };
    let mut lines = vec![line(
        session.actor_name(&snapshot.selected_agent),
        Tone::Text,
    )];
    if let Some(model) = &snapshot.model {
        lines.push(line(&model.model, Tone::Accent));
        let effort = model.effort.as_deref().unwrap_or("n/a");
        lines.push(line(format!("Effort: {effort}"), Tone::Muted));
    }
    lines
}

fn entry_rows(entry: &SidebarEntry, index: Option<usize>, inner: usize) -> Vec<Row> {
    let tone = match entry.tone {
        SidebarTone::Muted => Tone::Muted,
        SidebarTone::Accent => Tone::Accent,
        SidebarTone::Success => Tone::Success,
        SidebarTone::Warning => Tone::Warning,
        SidebarTone::Error => Tone::Error,
        _ => Tone::Text,
    };
    let marker = if entry.action.is_some() { "› " } else { "" };
    let details = wrap_words(&entry.detail, inner)
        .into_iter()
        .filter(|line| !line.is_empty())
        .take(3)
        .map(|detail| line(detail, Tone::Muted));
    std::iter::once(line(format!("{marker}{}", entry.label), tone))
        .chain(details)
        .map(|line| Row {
            line,
            action: entry.action.clone(),
            index,
        })
        .collect()
}

/// Keep the highlighted action visible while navigating with the keyboard.
fn scroll(state: &SidebarState, content: &[Row], space: usize) -> usize {
    let mut scroll = state.scroll.min(content.len().saturating_sub(space));
    if state.focused
        && let Some(row) = content.iter().position(|r| r.index == Some(state.selected))
    {
        scroll = scroll.min(row).max((row + 1).saturating_sub(space));
    }
    scroll
}

fn highlight(line: &Line, selected: bool) -> Line {
    if !selected {
        return line.clone();
    }
    line.iter()
        .map(|part| Span {
            tone: Tone::Selected,
            ..part.clone()
        })
        .collect()
}

fn footer(state: &SidebarState, total: usize, scroll: usize, space: usize) -> [Line; 2] {
    let position = if total > space {
        let last = (scroll + space).min(total);
        format!("{}–{last} / {total} · scroll", scroll + 1)
    } else {
        String::new()
    };
    let hint = if state.focused {
        "↑↓ choose · Enter · Esc back"
    } else {
        "F7 hide · F8 focus"
    };
    [line(position, Tone::Muted), line(hint, Tone::Muted)]
}

/// Place the sidebar after column `left` of every main row.
fn join(mut main: Frame, side: Frame, left: usize, rows: usize) -> Frame {
    main.spans.resize_with(rows.max(main.spans.len()), Vec::new);
    for (i, row) in main.spans.iter_mut().enumerate() {
        let used: usize = row.iter().map(|span| span.text.width()).sum();
        row.push(span(" ".repeat(left.saturating_sub(used)), Tone::Text));
        row.push(span("│ ", Tone::Border));
        row.extend(side.spans.get(i).cloned().unwrap_or_default());
    }
    main.lines = main
        .spans
        .iter()
        .map(|row| row.iter().map(|span| span.text.as_str()).collect())
        .collect();
    main
}
