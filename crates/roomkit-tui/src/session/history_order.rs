//! Position visible items using the host's reading order.
use roomkit_tui_protocol::{CallId, Item, MessageId};

use super::steering::Handed;
use super::unread::place;

/// Objects without a host position keep the placement used for unread input.
/// A positioned draft may precede a message delivered before its first word.
pub(super) fn insert(items: &mut Vec<Item>, item: Item, handed: Option<&Handed>) {
    let at = match item.position() {
        Some(position) => items
            .iter()
            .position(|old| old.position().is_some_and(|p| p.key() > position.key()))
            .unwrap_or(items.len()),
        None => place(handed, items),
    };
    items.insert(at, item);
}

/// Reconcile at the existing location unless the committed position corrects
/// a draft's estimate (for example after a tool row hidden from its stream).
pub(super) fn replace(items: &mut Vec<Item>, at: usize, item: Item, handed: Option<&Handed>) {
    if item.position() == items[at].position() {
        items[at] = item;
        return;
    }
    let misplaced = item.position().is_some_and(|pos| {
        items[..at]
            .iter()
            .any(|old| old.position().is_some_and(|p| p.key() > pos.key()))
            || items[at + 1..]
                .iter()
                .any(|old| old.position().is_some_and(|p| p.key() < pos.key()))
    });
    if misplaced {
        items.remove(at);
        insert(items, item, handed);
    } else {
        items[at] = item;
    }
}
/// Reasoning belongs before the prose it introduced, even if that prose
/// acquires its position after a peer has already appeared.
pub(super) fn place_reasoning(items: &mut Vec<Item>, thought: &CallId, reply: &MessageId) {
    let Some(before) = items
        .iter()
        .rposition(|item| matches!(item, Item::Message(m) if m.id == *reply))
    else {
        return;
    };
    if let Some(offset) = items[before + 1..]
        .iter()
        .position(|item| matches!(item, Item::Thinking(t) if t.id == *thought))
    {
        let block = items.remove(before + 1 + offset);
        items.insert(before, block);
    }
}
