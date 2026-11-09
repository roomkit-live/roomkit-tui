//! The frame around the history: the header and the activity row with its
//! right-aligned controls.

use roomkit_tui_protocol::RunOutcome;
use unicode_width::UnicodeWidthStr;

use super::text::{fit, format_duration, pad};
use super::{BackToBottomTarget, Line, Tone, View, frame, line, span, spinner};
use crate::LogoTile;
/// A logo row's colored pieces, when they spell `written`, the row as the
/// application set it: a row written after its pieces is drawn as text.
pub(super) fn spelling<'t>(
    tiles: Option<&'t Vec<LogoTile>>,
    written: &str,
) -> Option<&'t Vec<LogoTile>> {
    tiles.filter(|tiles| tiles.iter().map(|t| t.text.as_str()).collect::<String>() == written)
}

/// A logo's row, `width` columns wide: its colored pieces when it has some,
/// else `text` in the branding tone.
pub(super) fn logo_row(text: &str, tiles: Option<&Vec<LogoTile>>, width: usize) -> Line {
    let Some(tiles) = tiles else {
        return vec![span(pad(text, width), Tone::Primary)];
    };
    let mut left = width;
    let mut row = Line::with_capacity(tiles.len() + 1);
    for tile in tiles {
        let piece = fit(&tile.text, left);
        left = left.saturating_sub(piece.width());
        row.push(span(piece, tile.color.map_or(Tone::Text, Tone::Mark)));
    }
    row.push(span(" ".repeat(left), Tone::Text));
    row
}

pub(super) fn header(view: &View<'_>, cols: usize, rows: usize) -> Vec<Line> {
    let brand = &view.options.branding;
    let title = view
        .session
        .snapshot
        .as_ref()
        .map_or("Connecting…", |s| s.title.as_str());
    let mut lines = Vec::new();
    if cols >= 56 && rows >= 20 && !brand.logo.is_empty() {
        let logo_rows = brand.logo.len().min(5);
        let logo = &brand.logo[..logo_rows];
        let logo_width = logo.iter().map(|s| fit(s, 16).width()).max().unwrap_or(0);
        let title_row = logo_rows.saturating_sub(2) / 2;
        for (index, mark) in logo.iter().enumerate() {
            let (detail, tone) = match index.checked_sub(title_row) {
                Some(0) => (format!("{}  ·  {title}", brand.name), Tone::Primary),
                Some(1) => (brand.subtitle.clone(), Tone::Muted),
                _ => (String::new(), Tone::Muted),
            };
            let mut row = vec![span(" ", Tone::Primary)];
            let tiles = spelling(brand.logo_tiles.get(index), mark);
            row.extend(logo_row(mark, tiles, logo_width));
            row.push(span(format!("   {detail}"), tone));
            lines.push(row);
        }
    } else {
        lines.push(vec![
            span(format!(" {}", brand.name), Tone::Primary),
            span(format!(" · {title}"), Tone::Muted),
        ]);
    }
    lines.push(line("─".repeat(cols), Tone::Border));
    lines
}

pub(super) fn activity(view: &View<'_>) -> Line {
    let session = view.session;
    let (text, tone) = if session.interrupting() {
        let spinner = spinner(view.options, view.elapsed);
        (
            format!("{spinner} Stopping · waiting for the host"),
            Tone::Warning,
        )
    } else if session.is_executing() {
        (
            format!("{} Working", spinner(view.options, view.elapsed)),
            Tone::Accent,
        )
    } else {
        match session.execution_outcome() {
            Some(RunOutcome::Completed) => ("✓ Execution Completed".into(), Tone::Success),
            Some(RunOutcome::Interrupted) => ("− Execution Interrupted".into(), Tone::Warning),
            Some(RunOutcome::Failed) => ("× Execution Failed".into(), Tone::Error),
            Some(_) => ("• Execution Finished".into(), Tone::Muted),
            None => ("○ Ready".into(), Tone::Muted),
        }
    };
    let mut row = vec![span(format!(" {text}"), tone)];
    if session.active_run().is_some() || session.run_outcome().is_some() {
        let finished = session.run_outcome().is_some();
        let duration = session.run_elapsed(view.elapsed).map_or_else(
            || "elapsed unknown".into(),
            |duration| format_duration(duration, finished),
        );
        row.push(span(format!(" · {duration}"), Tone::Muted));
    }
    if !session.notice.is_empty() {
        row.push(span(format!(" · {}", session.notice), Tone::Muted));
    }
    row
}

pub(super) fn attachments(view: &View<'_>, cols: usize) -> Vec<Line> {
    let images = &view.session.images;
    if images.is_empty() {
        return Vec::new();
    }
    let labels = images
        .iter()
        .enumerate()
        .map(|(i, image)| format!("[Image {} · {}×{}]", i + 1, image.width, image.height))
        .collect::<Vec<_>>()
        .join(" ");
    let text = fit(&format!(" {labels} · Alt+Backspace remove last"), cols);
    vec![line(text, Tone::Primary)]
}

/// Right-aligned controls on the activity row: background work and, while
/// browsing history, the way back to the latest activity.
pub(super) fn controls(
    view: &View<'_>,
    row: &mut Line,
    cols: usize,
    screen_row: usize,
    more_below: bool,
) -> (Option<BackToBottomTarget>, Vec<super::ActionTarget>) {
    let back = more_below.then_some(match cols {
        80.. => " ↓ Back to bottom · Ctrl+End ",
        36.. => " ↓ Back to bottom ",
        _ => " ↓ Bottom ",
    });
    let mut controls = Vec::new();
    if let Some(status) = &view.session.background {
        let available = cols.saturating_sub(back.map_or(0, str::width) + 1);
        if available >= 8 {
            let label = if available >= 24 {
                format!(" {} · F6 ", status.label)
            } else {
                " Jobs F6 ".into()
            };
            controls.push((fit(&label, available), true));
        }
    }
    controls.extend(back.map(|label| (label.to_string(), false)));
    let reserved: usize = controls.iter().map(|(label, _)| label.width()).sum();
    if reserved == 0 {
        return (None, Vec::new());
    }
    let start = cols.saturating_sub(reserved);
    let mut clipped = frame(vec![std::mem::take(row)], start.saturating_sub(1), None);
    let used = clipped.lines[0].width();
    *row = clipped.spans.remove(0);
    row.push(span(" ".repeat(start - used), Tone::Text));
    place_controls(view, row, controls, start, screen_row)
}

fn place_controls(
    view: &View<'_>,
    row: &mut Line,
    controls: Vec<(String, bool)>,
    mut column: usize,
    screen_row: usize,
) -> (Option<BackToBottomTarget>, Vec<super::ActionTarget>) {
    let session = view.session;
    let clickable = view.options.selection == crate::SelectionMode::CopyOnSelect;
    let (mut back, mut actions) = (None, Vec::new());
    for (label, background) in controls {
        let end = column + label.width();
        row.push(span(
            label,
            if background {
                Tone::Accent
            } else {
                Tone::Selected
            },
        ));
        if let (true, Some(snapshot)) = (clickable, &session.snapshot) {
            let conversation = snapshot.conversation.clone();
            match (&session.background, background) {
                (Some(status), true) => actions.push(super::ActionTarget {
                    conversation,
                    command: status.command.clone(),
                    arguments: status.arguments.clone(),
                    panel: None,
                    sidebar: false,
                    row: screen_row,
                    start: column,
                    end,
                }),
                (_, false) => {
                    back = Some(BackToBottomTarget {
                        conversation,
                        row: screen_row,
                        start: column,
                        end,
                    });
                }
                _ => {}
            }
        }
        column = end;
    }
    (back, actions)
}
