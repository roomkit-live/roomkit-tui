//! A card over the history, as a decision or a question is asked: the
//! latest history above, the card, and the notice below it. A card is shown
//! whole or not at all: a person never answers part of what is asked.

use super::text::wrap;
use super::timeline::timeline;
use super::{Frame, Line, Tone, View, frame, line};

/// A card laid out at the bottom of its screen.
pub(super) struct Placed {
    pub(super) frame: Frame,
    /// The screen row of the card's first row.
    pub(super) top: usize,
}

/// The screen with `card` above the notice, under as much history as fits;
/// `caret` is the cursor's `(column, row)` within the card. `None` when the
/// card and the notice do not fit `rows`.
pub(super) fn card_screen(
    view: &View<'_>,
    cols: usize,
    rows: usize,
    card: Vec<Line>,
    caret: Option<(usize, usize)>,
) -> Option<Placed> {
    let session = view.session;
    let notice = usize::from(!session.notice.is_empty());
    if card.len() + notice > rows {
        return None;
    }
    let top = rows - card.len() - notice;
    let history = timeline(view, cols, top.saturating_sub(1), 0).lines;
    let mut lines: Vec<Line> = history
        .into_iter()
        .rev()
        .take(top.saturating_sub(1))
        .collect();
    lines.reverse();
    lines.resize_with(top, Vec::new);
    lines.extend(card);
    if notice > 0 {
        lines.push(line(format!(" {}", session.notice), Tone::Warning));
    }
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
