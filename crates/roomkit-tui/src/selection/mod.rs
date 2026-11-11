//! Mouse text selection. A press on a history line anchors the selection in
//! the conversation's content, so it survives scrolling and copies more than
//! the screen shows; a press elsewhere selects within the frame it hit.

mod conversation;
mod screen;
#[cfg(test)]
mod tests;

use std::ops::{Range, RangeInclusive};

use base64::{Engine, engine::general_purpose::STANDARD};
use roomkit_tui_protocol::Event;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::Session;
use crate::render::{
    ActionTarget, BackToBottomTarget, Frame, PermissionTarget, Span, Tone, ToolTarget,
};

/// Largest selection written to the clipboard, newlines included.
const CLIPBOARD_LIMIT: usize = 65_536;

/// Which way a held conversation selection's pointer lies past its area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Edge {
    /// Above the first history row: towards older content.
    Above,
    /// Below the last history row: towards newer content.
    Below,
}

/// A press, then the drag that follows it while the button is held.
pub(crate) struct TextSelection {
    /// The frame of the press; click targets are checked against it.
    frame: Frame,
    press: (u16, u16),
    extent: Extent,
    /// Pointer position while the button is held.
    pointer: Option<(u16, u16)>,
    /// The conversation scrolled under the held pointer since the end moved.
    scrolled: bool,
    dragged: bool,
}

enum Extent {
    Screen(screen::Extent),
    /// Boxed: its anchors carry their items, the screen's two points do not.
    Conversation(Box<conversation::Extent>),
}

impl TextSelection {
    pub(crate) fn new(frame: Frame, column: u16, row: u16, session: &Session) -> Self {
        let extent = conversation::Extent::at(&frame, column, row, session).map_or_else(
            || Extent::Screen(screen::Extent::new(column, row)),
            |extent| Extent::Conversation(Box::new(extent)),
        );
        Self {
            frame,
            press: (column, row),
            extent,
            pointer: Some((column, row)),
            scrolled: false,
            dragged: false,
        }
    }

    /// Move the selection's end under the held pointer, once the pointer
    /// moved or the conversation scrolled under it: content that only
    /// arrives never turns a click into a selection. A conversation
    /// selection reads the content through `frame`, the frame on screen.
    pub(crate) fn hold(&mut self, column: u16, row: u16, frame: &Frame, session: &Session) {
        let scrolled = std::mem::take(&mut self.scrolled);
        if self.pointer == Some((column, row)) && !scrolled {
            return;
        }
        self.pointer = Some((column, row));
        self.dragged |= match &mut self.extent {
            Extent::Screen(extent) => extent.extend(&self.frame, column, row),
            Extent::Conversation(extent) => extent.extend(frame, column, row, session),
        };
    }

    /// Extend to the content now under a held pointer, after it scrolled.
    pub(crate) fn follow(&mut self, frame: &Frame, session: &Session) {
        if let Some((column, row)) = self.pointer {
            self.hold(column, row, frame, session);
        }
    }

    /// The conversation scrolled under the pointer: while the button is
    /// held, the next frame moves the end to what the pointer then shows.
    pub(crate) fn content_scrolled(&mut self) {
        self.scrolled |= self.held();
    }

    /// Keep the ends on their text across a host event, before the session
    /// applies it: a streamed draft that becomes a message moves them along.
    pub(crate) fn observe(&mut self, event: &Event) {
        if let (
            Extent::Conversation(extent),
            Event::MessageFinalized {
                conversation,
                draft,
                message,
            },
        ) = (&mut self.extent, event)
        {
            extent.finalized(conversation, draft, message);
        }
    }

    /// Let go of the button; what is selected stays selected.
    pub(crate) fn release(&mut self) {
        self.pointer = None;
    }

    pub(crate) fn held(&self) -> bool {
        self.pointer.is_some()
    }

    /// Whether the end ever left the press: a drag never activates a target.
    pub(crate) fn dragged(&self) -> bool {
        self.dragged
    }

    /// Whether the selection is anchored in the conversation, so scrolling
    /// moves it with its text instead of discarding it.
    pub(crate) fn scrolls(&self) -> bool {
        matches!(self.extent, Extent::Conversation(_))
    }

    /// Where a held conversation selection's pointer lies past its area.
    pub(crate) fn edge(&self, frame: &Frame) -> Option<Edge> {
        let (_, row) = self.pointer.filter(|_| self.scrolls())?;
        let area = frame.timeline.as_ref()?;
        let row = usize::from(row);
        if row < area.top {
            (!area.starts_history).then_some(Edge::Above)
        } else if row >= area.top + area.height {
            Some(Edge::Below)
        } else {
            None
        }
    }

    /// A screen selection's highlighted frozen frame. `None` for a
    /// conversation selection, which is painted over the live screen.
    pub(crate) fn frozen(&self) -> Option<Frame> {
        match &self.extent {
            Extent::Screen(extent) => Some(extent.highlighted(&self.frame)),
            Extent::Conversation(_) => None,
        }
    }

    /// Highlight a conversation selection on the live `frame`.
    pub(crate) fn paint(&self, frame: &mut Frame, session: &Session) {
        if self.held() {
            frame.cursor = None;
        }
        if let Extent::Conversation(extent) = &self.extent {
            extent.paint(frame, session);
        }
    }

    pub(crate) fn permission_click(&self) -> Option<&PermissionTarget> {
        self.frame
            .permission_targets
            .iter()
            .find(|target| self.clicked(target.row, target.start..target.end))
    }

    pub(crate) fn action_click(&self) -> Option<&ActionTarget> {
        self.frame
            .action_targets
            .iter()
            .find(|target| self.clicked(target.row, target.start..target.end))
    }

    pub(crate) fn tool_click(&self) -> Option<&ToolTarget> {
        self.frame
            .tool_targets
            .iter()
            .find(|target| self.clicked(target.row, target.start..target.end))
    }

    pub(crate) fn back_to_bottom_click(&self) -> Option<&BackToBottomTarget> {
        self.frame
            .back_to_bottom
            .as_ref()
            .filter(|target| self.clicked(target.row, target.start..target.end))
    }

    /// Whether this is a click, not a drag, on `columns` of screen `row`.
    fn clicked(&self, row: usize, columns: Range<usize>) -> bool {
        !self.dragged
            && row == usize::from(self.press.1)
            && columns.contains(&usize::from(self.press.0))
    }

    /// The selected text; `None` for a click. A conversation selection lays
    /// its lines out again through `blocks`, given item indexes and a width.
    pub(crate) fn text<I>(
        &self,
        session: &Session,
        blocks: impl FnOnce(RangeInclusive<usize>, usize) -> I,
    ) -> Result<Option<String>, &'static str>
    where
        I: Iterator<Item = (usize, Vec<String>)>,
    {
        let text = match &self.extent {
            Extent::Screen(extent) => extent.text(&self.frame)?,
            Extent::Conversation(extent) => extent.text(session, blocks)?,
        };
        Ok(text.filter(|text| !text.trim().is_empty()))
    }
}

/// Selected lines in order, refused beyond the clipboard limit.
#[derive(Default)]
struct Copied {
    lines: Vec<String>,
    bytes: usize,
}

impl Copied {
    /// Add the columns of `range` in `line`, without trailing padding.
    fn push(&mut self, line: &str, range: Range<usize>) -> Result<(), &'static str> {
        let mut selected = String::new();
        let mut column = 0;
        for grapheme in line.graphemes(true) {
            let end = column + grapheme.width();
            if column < range.end && end > range.start {
                selected.push_str(grapheme);
            }
            column = end;
        }
        let selected = selected.trim_end().to_string();
        self.bytes += selected.len() + 1;
        if self.bytes > CLIPBOARD_LIMIT {
            return Err("Selection exceeds the 64 KiB clipboard limit");
        }
        self.lines.push(selected);
        Ok(())
    }

    fn text(self) -> String {
        self.lines.join("\n")
    }
}

/// Show the columns of `range` in one row as selected, keeping every
/// grapheme cluster whole.
fn paint(spans: &mut Vec<Span>, range: Range<usize>) {
    let mut column = 0;
    let mut highlighted: Vec<Span> = Vec::new();
    for span in spans.iter() {
        for grapheme in span.text.graphemes(true) {
            let end = column + grapheme.width();
            let tone = if column < range.end && end > range.start {
                Tone::Selected
            } else {
                span.tone
            };
            if let Some(last) = highlighted.last_mut().filter(|last| {
                last.tone == tone
                    && last.emphasis == span.emphasis
                    && last.hyperlink == span.hyperlink
            }) {
                last.text.push_str(grapheme);
            } else {
                highlighted.push(Span {
                    text: grapheme.into(),
                    tone,
                    emphasis: span.emphasis,
                    hyperlink: span.hyperlink.clone(),
                });
            }
            column = end;
        }
    }
    *spans = highlighted;
}

/// Only an explicit mouse release can call this path; it never reads a clipboard.
pub(crate) fn clipboard_sequence(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", STANDARD.encode(text.as_bytes()))
}
