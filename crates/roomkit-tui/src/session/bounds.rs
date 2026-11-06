//! Bounds for host input. A host is trusted to own the conversation, not to
//! keep the view's memory and screen bounded. What is refused outright is the
//! protocol's rule (`Snapshot::validate`, `Permission::fits`), shared with hosts.

use roomkit_tui_protocol::limits::{
    MAX_LABEL_BYTES, MAX_PERMISSION_SUMMARY_BYTES, MAX_PERMISSION_TITLE_BYTES,
};
use roomkit_tui_protocol::{Item, ModelInfo, Snapshot, ToolCall};

const TRUNCATION_MARK: &str = "\n[view truncated]";
const TRUNCATED_START_MARK: &str = "[view truncated]\n";

/// Whether a label event fits. A longer one is refused, not shortened, so the
/// view keeps the last label; only a snapshot, which cannot be half applied,
/// shortens its labels.
pub(super) fn fits_label(text: &str) -> bool {
    text.len() <= MAX_LABEL_BYTES
}

/// Shorten display labels; identities are never truncated.
pub(super) fn bound_labels(snapshot: &mut Snapshot) {
    bound_in_place(&mut snapshot.title, MAX_LABEL_BYTES);
    for actor in &mut snapshot.actors {
        bound_in_place(&mut actor.name, MAX_LABEL_BYTES);
    }
    if let Some(model) = &mut snapshot.model {
        bound_model(model);
    }
    if let Some(cost) = &mut snapshot.cost {
        bound_in_place(&mut cost.currency, MAX_LABEL_BYTES);
    }
}

pub(super) fn bound_model(model: &mut ModelInfo) {
    bound_in_place(&mut model.provider, MAX_LABEL_BYTES);
    bound_in_place(&mut model.model, MAX_LABEL_BYTES);
    if let Some(effort) = &mut model.effort {
        bound_in_place(effort, MAX_LABEL_BYTES);
    }
}

/// Drop the oldest items beyond the limit. Returns whether any were dropped.
pub(super) fn trim_items(items: &mut Vec<Item>, limit: usize) -> bool {
    if items.len() <= limit {
        return false;
    }
    items.drain(..items.len() - limit);
    true
}

pub(super) fn bound_item(item: &mut Item, limit: usize) {
    match item {
        Item::Approval(record) => {
            bound_in_place(&mut record.permission.title, MAX_PERMISSION_TITLE_BYTES);
            bound_in_place(&mut record.permission.detail, limit);
            if let Some(summary) = &mut record.permission.summary {
                bound_in_place(summary, MAX_PERMISSION_SUMMARY_BYTES);
            }
        }
        Item::Message(message) => bound_in_place(&mut message.text, limit),
        Item::Tool { call, .. } => bound_tool(call, limit),
        Item::Thinking(block) => bound_end_in_place(&mut block.text, limit),
        _ => {}
    }
}

fn bound_tool(call: &mut ToolCall, limit: usize) {
    bound_in_place(&mut call.text, limit);
    if let Some(intent) = &mut call.intent {
        bound_in_place(intent, limit);
    }
    if call
        .data
        .as_ref()
        .is_some_and(|data| data.to_string().len() > limit)
    {
        call.data = None;
        call.text = bounded(
            &format!("[structured output omitted from this view]\n{}", call.text),
            limit,
        );
    }
    if call.arguments.to_string().len() > limit {
        call.arguments = serde_json::Value::String("[arguments omitted from this view]".into());
    }
}

/// A copy of `text` within `limit` bytes, marked when truncated.
pub(crate) fn bounded(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    if limit < TRUNCATION_MARK.len() {
        return text[..text.floor_char_boundary(limit)].to_owned();
    }
    let end = text.floor_char_boundary(limit - TRUNCATION_MARK.len());
    format!("{}{TRUNCATION_MARK}", &text[..end])
}

/// Truncate `text` to `limit` bytes without copying when it already fits.
pub(super) fn bound_in_place(text: &mut String, limit: usize) {
    if text.len() <= limit {
        return;
    }
    if limit < TRUNCATION_MARK.len() {
        text.truncate(text.floor_char_boundary(limit));
        return;
    }
    text.truncate(text.floor_char_boundary(limit - TRUNCATION_MARK.len()));
    text.push_str(TRUNCATION_MARK);
}

/// Keep the end of `text` within `limit` bytes, marked when truncated. It drops a
/// quarter more than it must, so a text that keeps growing is not moved whole for
/// each piece.
pub(super) fn bound_end_in_place(text: &mut String, limit: usize) {
    if text.len() <= limit {
        return;
    }
    let mark = if limit / 2 < TRUNCATED_START_MARK.len() {
        ""
    } else {
        TRUNCATED_START_MARK
    };
    let kept = (limit - limit / 4).saturating_sub(mark.len());
    let cut = text.ceil_char_boundary(text.len() - kept);
    text.replace_range(..cut, mark);
    text.shrink_to(limit);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounding_keeps_char_boundaries_and_marks_truncation() {
        assert_eq!(bounded("short", 64), "short");
        let long = "é".repeat(100);
        let result = bounded(&long, 40);
        assert!(result.len() <= 40);
        assert!(result.ends_with(TRUNCATION_MARK));
        assert_eq!(bounded("abcdef", 3), "abc");
        let mut text = "x".repeat(10);
        bound_in_place(&mut text, 10);
        assert_eq!(text.len(), 10, "Text within the limit is untouched");
    }

    #[test]
    fn bounding_the_end_keeps_char_boundaries_and_one_mark() {
        let mut text = "é".repeat(100);
        bound_end_in_place(&mut text, 200);
        assert_eq!(text, "é".repeat(100), "Text within the limit is untouched");
        text.push_str("end");
        bound_end_in_place(&mut text, 200);
        assert!(text.len() <= 200);
        assert!(text.starts_with(TRUNCATED_START_MARK) && text.ends_with("éend"));
        text.push_str(&"x".repeat(100));
        bound_end_in_place(&mut text, 200);
        assert_eq!(text.matches("[view truncated]").count(), 1);

        let mut small = "abcdef".to_owned();
        bound_end_in_place(&mut small, 4);
        assert_eq!(small, "def", "No room for the mark");

        let mut huge = "x".repeat(1 << 20);
        bound_end_in_place(&mut huge, 200);
        assert!(huge.capacity() < 1024, "{}", huge.capacity());
    }
}
