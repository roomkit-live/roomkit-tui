use roomkit::model::{EventContent, EventType, RoomEvent, ToolCallOutcome, ToolCallStatus};
use roomkit_tui_protocol::limits::MAX_LABEL_BYTES;
use roomkit_tui_protocol::{
    ActorId, Item, Message, MessageId, PermissionId, RunId, ToolCall, ToolState,
};
use serde_json::Value;

/// What a permission identity adds to its run's: `:approval:` and a `u64`.
const PERMISSION_SUFFIX_BYTES: usize = ":approval:".len() + "18446744073709551615".len();
/// Longest run identity, leaving room for the permissions derived from it:
/// terminals refuse an identity over `MAX_LABEL_BYTES`.
const MAX_RUN_ID_BYTES: usize = MAX_LABEL_BYTES - PERMISSION_SUFFIX_BYTES;

/// The run answering `trigger` by `agent`: the identity of `RunStarted`,
/// tool items, usage and permissions alike.
pub(crate) fn run_id(agent: impl std::fmt::Display, trigger: impl std::fmt::Display) -> RunId {
    bounded_run_id(format!("{agent}:{trigger}"))
}

/// `id` as a run identity a terminal accepts with its permissions. RoomKit
/// bounds no identity, so a longer one keeps a readable prefix and a digest
/// of the whole.
pub(crate) fn bounded_run_id(id: String) -> RunId {
    if id.len() <= MAX_RUN_ID_BYTES {
        return RunId::new(id);
    }
    let digest = format!("~{:016x}", fnv1a(&id));
    let prefix = &id[..id.floor_char_boundary(MAX_RUN_ID_BYTES - digest.len())];
    RunId::new(format!("{prefix}{digest}"))
}

/// The `sequence`-th permission asked during `run`.
pub(crate) fn permission_id(run: &RunId, sequence: u64) -> PermissionId {
    PermissionId::new(format!("{run}:approval:{sequence}"))
}

/// FNV-1a, 64 bits: the same for an identity after a restart and across Rust
/// releases, unlike the standard hasher, so restored history keeps its runs.
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

pub(crate) fn actor(row: &RoomEvent) -> ActorId {
    match &row.source.participant_id {
        Some(person) => ActorId::new(format!("person:{}", person.as_str())),
        None => ActorId::new(row.source.channel_id.as_str()),
    }
}

pub(crate) fn item(row: &RoomEvent) -> Option<Item> {
    if row.r#type == EventType::Observation
        && let Some(record) = row.metadata.get(crate::approval_history::RECEIPT_KEY)
    {
        return serde_json::from_value(record.clone())
            .ok()
            .map(Item::Approval);
    }
    if let EventContent::ToolCall(call) = &row.content {
        let state = match (call.status, call.outcome) {
            (ToolCallStatus::Pending, _) => ToolState::Pending,
            (_, Some(ToolCallOutcome::Cancelled)) => ToolState::Cancelled,
            (_, Some(ToolCallOutcome::Refused)) if !call.refused_but_ran => ToolState::Denied,
            (ToolCallStatus::Completed, Some(ToolCallOutcome::Served)) => ToolState::Succeeded,
            _ => ToolState::Failed,
        };
        // The answered trigger identifies the turn, so repeated provider call
        // IDs in later turns cannot overwrite each other, including after a
        // restart. It is the same identity as the run's start and approvals.
        let run = match &row.responds_to {
            Some(trigger) => run_id(&row.source.channel_id, trigger),
            None => bounded_run_id(
                row.correlation_id
                    .clone()
                    .unwrap_or_else(|| row.id.to_string()),
            ),
        };
        let text = match &call.result {
            Value::Null => String::new(),
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        let arguments = Value::Object(call.arguments.clone());
        let mut tool = ToolCall::new(
            call.tool_id.as_str(),
            call.tool_name.clone(),
            arguments,
            state,
        )
        .with_text(text);
        tool.intent.clone_from(&call.intent);
        // Preserve only authoritative structured content; never recover it by
        // parsing model text or observing pre-policy tool output.
        tool.data = call.structured_content.clone().map(Value::Object);
        tool.duration_ms = call.duration_ms;
        return Some(Item::Tool {
            actor: actor(row),
            run,
            call: tool,
        });
    }
    matches!(row.r#type, EventType::Message | EventType::System).then(|| {
        Item::Message(Message::new(
            MessageId::new(row.id.to_string()),
            actor(row),
            roomkit::text_of(&row.content),
        ))
    })
}

pub(crate) fn upsert(items: &mut Vec<Item>, item: Item) {
    match items.iter_mut().find(|old| old.same_identity(&item)) {
        Some(old) => *old = item,
        None => items.push(item),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_run_id_is_kept_as_written() {
        assert_eq!(run_id("agent", "evt_1").as_str(), "agent:evt_1");
    }

    #[test]
    fn a_long_run_id_fits_with_its_permissions_and_stays_distinct() {
        let agent = "é".repeat(200);
        let trigger = "0123456789abcdef0123456789abcdef";
        let run = run_id(&agent, trigger);
        assert!(run.as_str().len() <= MAX_RUN_ID_BYTES);
        assert!(permission_id(&run, u64::MAX).as_str().len() <= MAX_LABEL_BYTES);
        assert!(run.as_str().starts_with("éé"), "A readable prefix is kept");
        assert_eq!(run, run_id(&agent, trigger), "The same after a restart");
        assert_ne!(run, run_id(&agent, "fedcba9876543210fedcba9876543210"));
    }

    #[test]
    fn the_digest_matches_published_fnv1a_vectors() {
        assert_eq!(fnv1a(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a("a"), 0xaf63_dc4c_8601_ec8c);
    }
}
