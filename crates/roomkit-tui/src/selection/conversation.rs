//! A selection anchored in the conversation's content rather than in screen
//! rows: it survives scrolling and new messages, and copies lines that are
//! no longer on screen by laying them out again.

use std::ops::{Range, RangeInclusive};

use roomkit_tui_protocol::{ActorId, CallId, ConversationId, Item, MessageId, PermissionId, RunId};

use super::{Copied, paint};
use crate::Session;
use crate::render::{Frame, TimelineRows};

/// The identity [`Item::same_identity`] compares, without the item's content,
/// so that an anchor never copies a long message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ItemKey {
    Approval(ActorId, RunId, PermissionId),
    Message(ActorId, MessageId),
    Tool(ActorId, RunId, CallId),
}

impl ItemKey {
    pub(super) fn of(item: &Item) -> Option<Self> {
        match item {
            Item::Approval(record) => {
                let permission = &record.permission;
                Some(Self::Approval(
                    permission.actor.clone(),
                    permission.run.clone(),
                    permission.id.clone(),
                ))
            }
            Item::Message(message) => {
                Some(Self::Message(message.actor.clone(), message.id.clone()))
            }
            Item::Tool { actor, run, call } => {
                Some(Self::Tool(actor.clone(), run.clone(), call.id.clone()))
            }
            _ => None,
        }
    }

    pub(super) fn matches(&self, item: &Item) -> bool {
        match (self, item) {
            (Self::Approval(actor, run, id), Item::Approval(record)) => {
                let permission = &record.permission;
                permission.actor == *actor && permission.run == *run && permission.id == *id
            }
            (Self::Message(actor, id), Item::Message(message)) => {
                message.actor == *actor && message.id == *id
            }
            (
                Self::Tool(actor, run, id),
                Item::Tool {
                    actor: a,
                    run: r,
                    call,
                },
            ) => a == actor && r == run && call.id == *id,
            _ => false,
        }
    }
}

/// A history position: the first item of a segment, a line of its block and
/// a column.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Anchor {
    item: ItemKey,
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
            item: ItemKey::of(items.get(item)?)?,
            line,
            column: usize::from(column).min(area.width.saturating_sub(1)),
        })
    }

    fn position(&self, items: &[Item]) -> Option<Position> {
        let index = items.iter().rposition(|item| self.item.matches(item))?;
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
        if let Some(area) = frame
            .timeline
            .as_ref()
            .filter(|area| area.conversation == self.conversation)
            && let Some(head) = Anchor::under(area, column, row, items(session))
        {
            self.head = head;
            self.width = area.width;
        }
        self.head != self.anchor
    }

    /// Both ends in document order, while the conversation still has them.
    fn ends(&self, session: &Session) -> Option<(Position, Position)> {
        if self.anchor == self.head {
            return None;
        }
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
        let Some(area) = frame
            .timeline
            .as_ref()
            .filter(|area| area.conversation == self.conversation)
        else {
            return;
        };
        let Some(ends) = self.ends(session) else {
            return;
        };
        for (offset, &(item, line)) in area.origins.iter().enumerate() {
            if let Some(spans) = frame.spans.get_mut(area.top + offset) {
                paint(spans, columns((item, line), ends, area.width));
            }
        }
    }

    pub(super) fn text<I>(
        &self,
        session: &Session,
        blocks: impl FnOnce(RangeInclusive<usize>, usize) -> I,
    ) -> Result<Option<String>, &'static str>
    where
        I: Iterator<Item = (usize, Vec<String>)>,
    {
        let Some(ends) = self.ends(session) else {
            return Ok(None);
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
