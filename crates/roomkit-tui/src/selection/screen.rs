//! A selection bounded by one frozen frame: the sidebar, a panel, an
//! approval, or any row that is not a line of the conversation's history.

use std::ops::Range;

use super::{Copied, paint};
use crate::render::Frame;

/// Screen positions as `(column, row)`.
pub(super) struct Extent {
    anchor: (u16, u16),
    head: (u16, u16),
}

impl Extent {
    pub(super) fn new(column: u16, row: u16) -> Self {
        Self {
            anchor: (column, row),
            head: (column, row),
        }
    }

    /// Move the head, staying on the anchor's side of the sidebar. Returns
    /// whether the head is away from the anchor.
    pub(super) fn extend(&mut self, frame: &Frame, column: u16, row: u16) -> bool {
        let column = if let Some(sidebar) = &frame.sidebar {
            if usize::from(self.anchor.0) < sidebar.start {
                column.min(sidebar.start.saturating_sub(1) as u16)
            } else {
                column.max(sidebar.start as u16 + 2)
            }
        } else {
            column
        };
        self.head = (column, row);
        self.head != self.anchor
    }

    fn range(&self, frame: &Frame, row: usize) -> Range<usize> {
        let (a, b) = if (self.anchor.1, self.anchor.0) <= (self.head.1, self.head.0) {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        };
        if row < usize::from(a.1) || row > usize::from(b.1) {
            return 0..0;
        }
        let start = if row == usize::from(a.1) {
            usize::from(a.0)
        } else {
            0
        };
        let end = if row == usize::from(b.1) {
            usize::from(b.0) + 1
        } else {
            usize::MAX
        };
        if let Some(sidebar) = &frame.sidebar {
            if usize::from(self.anchor.0) < sidebar.start {
                return start..end.min(sidebar.start);
            }
            return start.max(sidebar.start + 2)..end;
        }
        start..end
    }

    pub(super) fn highlighted(&self, frozen: &Frame) -> Frame {
        let mut frame = frozen.clone();
        frame.cursor = None;
        if self.anchor == self.head {
            return frame;
        }
        for (row, spans) in frame.spans.iter_mut().enumerate() {
            paint(spans, self.range(frozen, row));
        }
        frame
    }

    pub(super) fn text(&self, frame: &Frame) -> Result<Option<String>, &'static str> {
        if self.anchor == self.head {
            return Ok(None);
        }
        let mut copied = Copied::default();
        for (row, line) in frame.lines.iter().enumerate() {
            let range = self.range(frame, row);
            if !range.is_empty() {
                copied.push(line, range)?;
            }
        }
        Ok(Some(copied.text()))
    }
}
