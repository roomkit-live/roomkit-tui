use roomkit::model::{EventContent, EventType, RoomEvent, ToolCallOutcome, ToolCallStatus};
use roomkit_tui_protocol::{ActorId, Item, Message, MessageId, RunId, ToolCall, ToolState};
use serde_json::Value;

/// The run answering `trigger` by `agent`: the identity of `RunStarted`,
/// tool items and permissions alike.
pub(crate) fn run_id(agent: impl std::fmt::Display, trigger: impl std::fmt::Display) -> RunId {
    RunId::new(format!("{agent}:{trigger}"))
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
            None => RunId::new(
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
