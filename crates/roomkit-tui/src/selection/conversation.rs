//! A selection anchored in the conversation's content rather than in screen
//! rows: it survives scrolling and new messages, and copies lines that are
//! no longer on screen by laying them out again.

use std::ops::{Range, RangeInclusive};

use roomkit_tui_protocol::{ConversationId, Item, Message, MessageId};

use super::{Copied, paint};
use crate::Session;
use crate::render::{Frame, TimelineRows};

/// A history position: the first item of a segment, a line of its block and
/// a column. The item is kept only for [`Item::same_identity`], the rule by
/// which the session replaces an item with its update.
#[derive(Clone, Debug)]
struct Anchor {
    item: Item,
    line: usize,
    column: usize,
}

/// A resolved anchor as `(item index, line, column)`, in document order.
type Position = (usize, usize, usize);

impl Anchor {
    /// The history under screen `(column, row)`, clamped to the lines shown:
    /// a row above them is the first line, one below them the last.
    fn under(area: &TimelineRows, column: u16, row: u16, items: &[Item]) -> Option<Self> {
        let last = area.origins.len().checked_sub(1)?;
        let shown = usize::from(row).saturating_sub(area.top).min(last);
        let (item, line) = area.origins[shown];
        Some(Self {
            item: items.get(item)?.clone(),
            line,
            column: usize::from(column).min(area.width.saturating_sub(1)),
        })
    }

    fn same(&self, other: &Self) -> bool {
        self.item.same_identity(&other.item)
            && (self.line, self.column) == (other.line, other.column)
    }

    fn position(&self, items: &[Item]) -> Option<Position> {
        let index = items
            .iter()
            .rposition(|item| self.item.same_identity(item))?;
        Some((index, self.line, self.column))
    }
}

pub(super) struct Extent {
    conversation: ConversationId,
    anchor: Anchor,
    head: Anchor,
    /// Columns of the history when last extended.
    width: usize,
}

impl Extent {
    /// A selection starting on a history line of `frame`; `None` elsewhere.
    pub(super) fn at(frame: &Frame, column: u16, row: u16, session: &Session) -> Option<Self> {
        let area = frame.timeline.as_ref()?;
        let shown = area.top..area.top + area.origins.len();
        if !shown.contains(&usize::from(row)) || usize::from(column) >= area.width {
            return None;
        }
        let anchor = Anchor::under(area, column, row, items(session))?;
        Some(Self {
            conversation: area.conversation.clone(),
            head: anchor.clone(),
            anchor,
            width: area.width,
        })
    }

    /// Move the head to the history under `(column, row)` of `frame`. Returns
    /// whether the head is away from the anchor.
    pub(super) fn extend(
        &mut self,
        frame: &Frame,
        column: u16,
        row: u16,
        session: &Session,
    ) -> bool {
        if let Some(area) = shown(&frame.timeline, &self.conversation)
            && let Some(head) = Anchor::under(area, column, row, items(session))
        {
            self.head = head;
            self.width = area.width;
        }
        !self.head.same(&self.anchor)
    }

    /// A streamed draft became `message`: the ends on it move to the message.
    pub(super) fn finalized(
        &mut self,
        conversation: &ConversationId,
        draft: &MessageId,
        message: &Message,
    ) {
        if *conversation != self.conversation {
            return;
        }
        let draft = Item::Message(Message::new(draft.clone(), message.actor.clone(), ""));
        for end in [&mut self.anchor, &mut self.head] {
            if end.item.same_identity(&draft) {
                end.item = Item::Message(message.clone());
            }
        }
    }

    /// Both ends in document order, while the conversation still has them.
    fn ends(&self, session: &Session) -> Option<(Position, Position)> {
        let snapshot = session
            .snapshot
            .as_ref()
            .filter(|snapshot| snapshot.conversation == self.conversation)?;
        let anchor = self.anchor.position(&snapshot.items)?;
        let head = self.head.position(&snapshot.items)?;
        Some(if anchor <= head {
            (anchor, head)
        } else {
            (head, anchor)
        })
    }

    pub(super) fn paint(&self, frame: &mut Frame, session: &Session) {
        if self.anchor.same(&self.head) {
            return;
        }
        let (Some(ends), Some(area)) = (
            self.ends(session),
            shown(&frame.timeline, &self.conversation),
        ) else {
            return;
        };
        for (offset, &(item, line)) in area.origins.iter().enumerate() {
            if let Some(spans) = frame.spans.get_mut(area.top + offset) {
                paint(spans, columns((item, line), ends, area.width));
            }
        }
    }

    /// The selected text, or an error once an end has left the conversation.
    pub(super) fn text<I>(
        &self,
        session: &Session,
        blocks: impl FnOnce(RangeInclusive<usize>, usize) -> I,
    ) -> Result<Option<String>, &'static str>
    where
        I: Iterator<Item = (usize, Vec<String>)>,
    {
        if self.anchor.same(&self.head) {
            return Ok(None);
        }
        let Some(ends) = self.ends(session) else {
            return Err("Selected text is no longer in the conversation");
        };
        let mut copied = Copied::default();
        for (item, lines) in blocks(ends.0.0..=ends.1.0, self.width) {
            for (line, text) in lines.iter().enumerate() {
                let range = columns((item, line), ends, self.width);
                if !range.is_empty() {
                    copied.push(text, range)?;
                }
            }
        }
        Ok(Some(copied.text()))
    }
}

/// The conversation area a frame shows, when it shows `conversation`.
fn shown<'a>(
    timeline: &'a Option<TimelineRows>,
    conversation: &ConversationId,
) -> Option<&'a TimelineRows> {
    timeline
        .as_ref()
        .filter(|area| area.conversation == *conversation)
}

fn items(session: &Session) -> &[Item] {
    session
        .snapshot
        .as_ref()
        .map_or(&[], |snapshot| &snapshot.items)
}

/// Columns selected on the line at `(item, line)` between the ends, within
/// the history's `width`.
fn columns(at: (usize, usize), (first, last): (Position, Position), width: usize) -> Range<usize> {
    if at < (first.0, first.1) || at > (last.0, last.1) {
        return 0..0;
    }
    let start = if at == (first.0, first.1) { first.2 } else { 0 };
    let end = if at == (last.0, last.1) {
        last.2 + 1
    } else {
        width
    };
    start..end.min(width)
}
