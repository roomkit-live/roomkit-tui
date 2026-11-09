//! Center an ordinary composer without creating a second input state machine.
use std::time::Duration;

use roomkit_tui_protocol::RunOutcome;
use unicode_width::UnicodeWidthStr;

use super::composer::{self, Layout};
use super::footer::{hints, settings};
use super::text::{fit, wrap};
use super::{Frame, Line, Tone, View, frame, line, span};
use crate::Session;
use crate::session::Focus;

/// Rows of a notice shown on the welcome screen; longer ones are cut.
const NOTICE_LINES: usize = 3;

pub(super) fn screen(view: &View<'_>, width: u16, height: u16) -> Option<Frame> {
    let (session, options) = (view.session, view.options);
    let welcome = options.welcome.as_ref()?;
    let snapshot = session.snapshot.as_ref()?;
    if width < 60
        || height < 20
        || !snapshot.items.is_empty()
        || session.is_executing()
        || matches!(
            session.execution_outcome(),
            Some(RunOutcome::Failed | RunOutcome::Interrupted)
        )
        || !matches!(session.focus(), Focus::Composer | Focus::Sidebar)
    {
        return None;
    }
    let cols = usize::from(width);
    let rows = usize::from(height);
    let inner = usize::from(welcome.width.clamp(48, 100).min(width - 8));
    let composer = composer::screen(view, inner as u16, height, Layout::Welcome);
    // Blank input padding has styled spans and must remain part of the block.
    let start = composer.spans.iter().position(|row| !row.is_empty())?;
    let mut lines = artwork(welcome, &options.branding.name, inner);
    lines.push(Vec::new());
    lines.push(center(
        &format!("{} · Start a conversation", options.branding.name),
        inner,
        Tone::Muted,
    ));
    if let Some(info) = &session.application_info {
        lines.push(center(info, inner, Tone::Muted));
    }
    lines.push(Vec::new());
    // A notice, such as a setup step the host asks for, stays on the welcome
    // screen above the composer rather than replacing the whole layout.
    if !session.notice.is_empty() {
        let mut notice = wrap(&session.notice, inner);
        if notice.len() > NOTICE_LINES {
            notice.truncate(NOTICE_LINES);
            if let Some(last) = notice.last_mut() {
                let kept = fit(last, inner.saturating_sub(1));
                *last = format!("{kept}…");
            }
        }
        lines.extend(notice.iter().map(|text| center(text, inner, Tone::Warning)));
        lines.push(Vec::new());
    }
    let body_offset = lines.len();
    lines.extend(composer.spans.into_iter().skip(start));
    if lines.len() + 2 > rows {
        return None;
    }
    let top = (rows - lines.len()) / 2;
    let left = (cols - inner) / 2;
    let cursor = composer.cursor.map(|(x, y)| {
        (
            x + left as u16,
            y.saturating_sub(start as u16) + body_offset as u16 + top as u16,
        )
    });
    let mut canvas = vec![Vec::new(); top];
    for mut row in lines {
        row.insert(0, span(" ".repeat(left), Tone::Text));
        canvas.push(row);
    }
    canvas.resize_with(rows, Vec::new);
    Some(frame(canvas, cols, cursor))
}

fn center(text: &str, width: usize, tone: Tone) -> Line {
    let text = fit(text, width);
    line(
        format!(
            "{}{text}",
            " ".repeat(width.saturating_sub(text.width()) / 2)
        ),
        tone,
    )
}

fn artwork(welcome: &crate::WelcomeScreen, name: &str, width: usize) -> Vec<Line> {
    let logo: Vec<_> = welcome.logo.iter().take(8).map(|s| fit(s, 24)).collect();
    let logo_width = logo.iter().map(|s| s.width()).max().unwrap_or(0);
    let gap = usize::from(logo_width > 0) * 3;
    let mut mark: Vec<_> = welcome
        .wordmark
        .iter()
        .take(8)
        .map(|s| fit(s, 64))
        .collect();
    if mark.is_empty()
        || logo_width + gap + mark.iter().map(|s| s.width()).max().unwrap_or(0) > width
    {
        mark = vec![fit(name, width.saturating_sub(logo_width + gap))];
    }
    let mark_width = mark.iter().map(|s| s.width()).max().unwrap_or(0);
    let rows = logo.len().max(mark.len());
    let left = width.saturating_sub(logo_width + gap + mark_width) / 2;
    (0..rows)
        .map(|row| {
            let symbol = row
                .checked_sub((rows - logo.len()) / 2)
                .and_then(|i| logo.get(i));
            let letters = row
                .checked_sub((rows - mark.len()) / 2)
                .and_then(|i| mark.get(i));
            let tiles = row.checked_sub((rows - logo.len()) / 2).and_then(|i| {
                let written = welcome.logo.get(i)?;
                super::chrome::spelling(welcome.logo_tiles.get(i), written)
            });
            let mut line = vec![span(" ".repeat(left), Tone::Text)];
            line.extend(super::chrome::logo_row(
                symbol.map_or("", String::as_str),
                tiles,
                logo_width,
            ));
            line.push(span(" ".repeat(gap), Tone::Text));
            line.push(span(letters.cloned().unwrap_or_default(), Tone::Text));
            line
        })
        .collect()
}

/// Keep one calm footer; transient input modes replace it with relevant help.
pub(super) fn footer(session: &Session, width: usize, elapsed: Duration) -> Line {
    if session.menu.visible
        || session.input_action().is_some()
        || session
            .quit_until
            .is_some_and(|deadline| elapsed < deadline)
    {
        return line(hints(session, width, true, elapsed), Tone::Muted);
    }
    let hint = if session
        .snapshot
        .as_ref()
        .is_some_and(|s| !s.commands.is_empty())
    {
        "Tab commands"
    } else {
        "Enter send"
    };
    let budget = width.saturating_sub(hint.width() + 3);
    let settings = session
        .snapshot
        .as_ref()
        .map(|s| settings(session, s, true, true))
        .unwrap_or_default();
    let mut parts = frame(vec![settings], budget, None)
        .spans
        .pop()
        .unwrap_or_default();
    let used: usize = parts.iter().map(|s| s.text.width()).sum();
    parts.push(span(
        format!(
            "{}{} ",
            " ".repeat(width.saturating_sub(used + hint.width() + 1)),
            hint
        ),
        Tone::Muted,
    ));
    parts
}
