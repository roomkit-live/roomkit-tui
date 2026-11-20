//! A card over the history, as a decision or a question is asked: the
//! latest history above, the card, and the notice below it. A card is shown
//! whole or not at all: a person never answers part of what is asked.

use super::chrome::{NOTICE_ROWS, notice_rows};
use super::text::{columns, wrap};
use super::timeline::timeline;
use super::{Frame, Line, Tone, View, frame, line, prose, span};

/// A card laid out at the bottom of its screen.
pub(super) struct Placed {
    pub(super) frame: Frame,
    /// The screen row of the card's first row.
    pub(super) top: usize,
}

/// The screen with `card` above the notice, under as much history as fits;
/// `caret` is the cursor's `(column, row)` within the card. `None` when the
/// card and a row of the notice do not fit `rows`. The notice wraps on the
/// rows the card leaves, at most [`NOTICE_ROWS`].
pub(super) fn card_screen(
    view: &View<'_>,
    cols: usize,
    rows: usize,
    card: Vec<Line>,
    caret: Option<(usize, usize)>,
) -> Option<Placed> {
    let session = view.session;
    let needed = card.len() + usize::from(!session.notice.is_empty());
    if needed > rows {
        return None;
    }
    let notice = if session.notice.is_empty() {
        Vec::new()
    } else {
        let room = (rows - card.len()).min(NOTICE_ROWS);
        notice_rows(&session.notice, cols, room, Tone::Warning)
    };
    let top = rows - card.len() - notice.len();
    let history = timeline(view, cols, top.saturating_sub(1), 0).lines;
    let mut lines: Vec<Line> = history
        .into_iter()
        .rev()
        .take(top.saturating_sub(1))
        .collect();
    lines.reverse();
    lines.resize_with(top, Vec::new);
    lines.extend(card);
    lines.extend(notice);
    let cursor = caret.map(|(column, row)| (column as u16, (top + row) as u16));
    Some(Placed {
        frame: frame(lines, cols, cursor),
        top,
    })
}

/// The screen asking for a larger terminal, saying what for.
pub(super) fn enlarge(text: &str, cols: usize) -> Frame {
    let rows = wrap(text, cols);
    frame(
        rows.into_iter().map(|row| line(row, Tone::Muted)).collect(),
        cols,
        None,
    )
}

/// `text` on the card's surface, wrapped at words within `width` columns. An
/// address keeps the row's `tone`, underlined, so that the surface stays
/// whole, and links its whole destination on every row it wraps onto.
pub(super) fn surface_rows(text: &str, width: usize, tone: Tone) -> Vec<Line> {
    let mut rows = prose::rows(text, width, tone, true);
    for part in rows.iter_mut().flatten() {
        if part.hyperlink.is_some() {
            part.tone = tone;
        }
    }
    rows
}

/// `row` `indent` columns in on the card's surface, which fills `cols` in
/// `tone`.
pub(super) fn filled_row(row: Line, indent: usize, cols: usize, tone: Tone) -> Line {
    let used = indent + row.iter().map(|part| columns(&part.text)).sum::<usize>();
    let mut filled = vec![span(" ".repeat(indent), tone)];
    filled.extend(row);
    filled.push(span(" ".repeat(cols.saturating_sub(used)), tone));
    filled
}
