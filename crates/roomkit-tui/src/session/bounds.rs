//! Validation and bounds for host input. A host is trusted to own the
//! conversation, not to keep the view's memory and screen bounded.

use std::collections::BTreeSet;

use roomkit_tui_protocol::limits::{
    MAX_CHOICES, MAX_LABEL_BYTES, MAX_PENDING_PERMISSIONS, MAX_PERMISSION_DETAIL_BYTES,
    MAX_PERMISSION_SUMMARY_BYTES, MAX_PERMISSION_TITLE_BYTES, MAX_SNAPSHOT_ENTRIES,
};
use roomkit_tui_protocol::{
    CommandSpec, Item, ModeState, ModelInfo, Permission, Snapshot, ToolCall,
};

const MAX_COMMAND_DESCRIPTION_BYTES: usize = 1_024;
const MAX_INPUT_ACTIONS: usize = 8;
const MAX_INPUT_ACTION_DESCRIPTION_BYTES: usize = 120;
const MAX_MODES: usize = 32;
const MAX_MODE_DESCRIPTION_BYTES: usize = 512;
const MAX_MODE_LABEL_BYTES: usize = 80;
const TRUNCATION_MARK: &str = "\n[view truncated]";

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
}

pub(super) fn bound_model(model: &mut ModelInfo) {
    bound_in_place(&mut model.provider, MAX_LABEL_BYTES);
    bound_in_place(&mut model.model, MAX_LABEL_BYTES);
    if let Some(effort) = &mut model.effort {
        bound_in_place(effort, MAX_LABEL_BYTES);
    }
}

pub(super) fn validate_snapshot(snapshot: &Snapshot) -> Result<(), &'static str> {
    if snapshot.conversation.as_str().len() > MAX_LABEL_BYTES
        || snapshot.actors.len() > MAX_SNAPSHOT_ENTRIES
    {
        return Err("Host snapshot exceeds the view's bounds");
    }
    if !snapshot
        .actors
        .iter()
        .any(|actor| actor.id == snapshot.selected_agent)
    {
        return Err("Host snapshot has an unknown selected agent");
    }
    let names = valid_commands(&snapshot.commands)
        .ok_or("Host snapshot has invalid or duplicate commands")?;
    if snapshot
        .mode
        .as_ref()
        .is_some_and(|mode| !valid_mode(mode, &snapshot.commands))
    {
        return Err("Host snapshot has invalid interaction modes");
    }
    let mut prefixes = BTreeSet::new();
    if snapshot.input_actions.len() > MAX_INPUT_ACTIONS
        || snapshot.input_actions.iter().any(|action| {
            action.prefix.is_alphanumeric()
                || action.prefix.is_whitespace()
                || action.prefix.is_control()
                || action.prefix == '\\'
                || action.description.len() > MAX_INPUT_ACTION_DESCRIPTION_BYTES
                || !names.contains(&action.command)
                || !prefixes.insert(action.prefix)
        })
    {
        return Err("Host snapshot has invalid composer actions");
    }
    let mut permissions = BTreeSet::new();
    if snapshot.permissions.len() > MAX_PENDING_PERMISSIONS
        || snapshot.permissions.iter().any(|permission| {
            snapshot.active_run.as_ref() != Some(&permission.run)
                || !permission_fits(permission)
                || !permissions.insert(&permission.id)
        })
    {
        return Err("Host snapshot has invalid or duplicate permissions");
    }
    Ok(())
}

fn valid_commands(commands: &[CommandSpec]) -> Option<BTreeSet<&String>> {
    let mut names = BTreeSet::new();
    if commands.len() > MAX_SNAPSHOT_ENTRIES {
        return None;
    }
    for command in commands {
        let valid = !command.name.is_empty()
            && command.name.len() <= MAX_LABEL_BYTES
            && command
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            && command.description.len() <= MAX_COMMAND_DESCRIPTION_BYTES
            && command.arguments.len() <= MAX_LABEL_BYTES
            && command.choices.len() <= MAX_CHOICES
            && command.choices.iter().all(|c| c.len() <= MAX_LABEL_BYTES)
            && names.insert(&command.name);
        if !valid {
            return None;
        }
    }
    Some(names)
}

pub(super) fn valid_mode(mode: &ModeState, commands: &[CommandSpec]) -> bool {
    let mut values = BTreeSet::new();
    commands.iter().any(|command| command.name == mode.command)
        && !mode.choices.is_empty()
        && mode.choices.len() <= MAX_MODES
        && mode.choices.iter().all(|choice| {
            !choice.value.is_empty()
                && choice.value.len() <= MAX_LABEL_BYTES
                && choice.label.len() <= MAX_MODE_LABEL_BYTES
                && choice.description.len() <= MAX_MODE_DESCRIPTION_BYTES
                && values.insert(&choice.value)
        })
        && values.contains(&mode.selected)
}

/// A person must be able to read everything they approve: an oversized
/// request is refused rather than displayed truncated.
pub(super) fn permission_fits(permission: &Permission) -> bool {
    permission.id.as_str().len() <= MAX_LABEL_BYTES
        && permission.title.len() <= MAX_PERMISSION_TITLE_BYTES
        && permission.detail.len() <= MAX_PERMISSION_DETAIL_BYTES
        && permission
            .summary
            .as_ref()
            .is_none_or(|s| s.len() <= MAX_PERMISSION_SUMMARY_BYTES)
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
}
