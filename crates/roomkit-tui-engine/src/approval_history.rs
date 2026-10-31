//! Store confirmed decisions after delivery settles, outside model context.
//! Keeping writes out of the decision loop also keeps interruption responsive
//! while the engine owns the room lock. Unwritten receipts remain retryable.
use crate::{AdapterError, ConversationChannel, SessionConfig};
use roomkit::model::{
    EventId, EventSource, EventType, ParticipantId, RoomEvent, TextContent, Timestamp, Visibility,
};
use roomkit::{CommitEventOptions, ConversationStore, RoomKit};
use std::time::{SystemTime, UNIX_EPOCH};

/// Metadata key holding a stored decision receipt.
pub(crate) const RECEIPT_KEY: &str = "roomkit_tui_approval";
/// Metadata key linking a stored receipt to its tool call.
pub(crate) const RECEIPT_TOOL_KEY: &str = "roomkit_tui_approval_tool_id";
/// Prefix of a stored receipt's event identity.
const RECEIPT_ID_PREFIX: &str = "tui-";

pub(crate) async fn persist<S: ConversationStore>(
    kit: &RoomKit<S>,
    channel: &ConversationChannel,
    config: &SessionConfig,
) -> Result<(), AdapterError> {
    for saved in channel.approvals.receipts(config.room.as_str()) {
        let receipt = saved.decision;
        let mut source =
            EventSource::new(channel.id().clone(), roomkit::model::ChannelType::WEBSOCKET);
        source.participant_id = Some(
            ParticipantId::new(
                receipt
                    .decided_by
                    .as_str()
                    .strip_prefix("person:")
                    .unwrap_or(receipt.decided_by.as_str()),
            )
            .map_err(|_| AdapterError::InvalidBinding)?,
        );
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| AdapterError::InvalidBinding)?
            .as_millis();
        let timestamp = Timestamp::from_unix_millis(
            i64::try_from(now).map_err(|_| AdapterError::InvalidBinding)?,
        )
        .map_err(|_| AdapterError::InvalidBinding)?;
        let mut row = RoomEvent::new(
            config.room.clone(),
            source,
            TextContent::new("Human permission decision"),
        );
        row.id = EventId::new(format!("{RECEIPT_ID_PREFIX}{}", receipt.permission.id))
            .map_err(|_| AdapterError::InvalidBinding)?;
        row.r#type = EventType::Observation;
        row.created_at = timestamp;
        row.visibility = Visibility::channels([channel.id().clone()])
            .map_err(|_| AdapterError::InvalidBinding)?;
        row.metadata
            .insert(RECEIPT_KEY.into(), serde_json::json!(receipt));
        if let Some(tool_id) = saved.tool_call_id {
            row.metadata.insert(RECEIPT_TOOL_KEY.into(), tool_id.into());
        }
        kit.commit_event(
            &config.room,
            row,
            CommitEventOptions {
                organization_id: config.organization.clone(),
            },
        )
        .await?;
        channel
            .approvals
            .recorded(config.room.as_str(), &receipt.permission.id);
    }
    Ok(())
}

/// Persistence is deferred until the room lock is released. Present receipts
/// beside their calls without rewriting the audit timeline or its timestamps.
/// Call only with the authorized, visible rows of one room.
pub(crate) fn replay_order(rows: Vec<RoomEvent>) -> Vec<RoomEvent> {
    let positions: Vec<_> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| replay_position(&rows, index, row))
        .collect();
    let mut ordered: Vec<_> = positions.into_iter().zip(rows).collect();
    // Stable sorting preserves decision order when multiple receipts share an anchor.
    ordered.sort_by_key(|(position, _)| *position);
    ordered.into_iter().map(|(_, row)| row).collect()
}

fn replay_position(rows: &[RoomEvent], index: usize, row: &RoomEvent) -> usize {
    use roomkit::model::EventContent;
    use roomkit_tui_protocol::Item;

    let original = index * 2 + 1;
    if row.r#type != EventType::Observation {
        return original;
    }
    let Some(Item::Approval(receipt)) = crate::projection::item(row) else {
        return original;
    };
    let owns = |candidate: &&RoomEvent| {
        crate::projection::actor(candidate) == receipt.permission.actor
            && candidate.responds_to.as_ref().is_some_and(|trigger| {
                receipt.permission.run
                    == crate::projection::run_id(&candidate.source.channel_id, trigger)
            })
    };
    let mut candidates = rows.iter().enumerate().filter(|(_, r)| owns(r));
    if let Some(tool_id) = row
        .metadata
        .get(RECEIPT_TOOL_KEY)
        .and_then(serde_json::Value::as_str)
    {
        // Scope by agent AND triggering message: providers may reuse call IDs.
        candidates
            .filter(|(_, r)| matches!(&r.content, EventContent::ToolCall(call) if call.tool_id == tool_id))
            .map(|(i, _)| i * 2 + 2)
            .next()
            .unwrap_or(original)
    } else {
        // Older receipts lack a call identity. Keep them in their own turn,
        // before its final answer, without guessing which tool was approved.
        candidates
            .rfind(|(_, r)| {
                matches!(r.r#type, EventType::Message | EventType::System)
                    || matches!(r.content, EventContent::ToolCall(_))
            })
            .map_or(original, |(i, r)| {
                if r.r#type == EventType::Message {
                    i * 2
                } else {
                    i * 2 + 2
                }
            })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "Test fixture failures are assertions")]
    use super::*;
    use roomkit::model::{ChannelId, ChannelType, EventContent, RoomId, ToolCallContent};
    use serde_json::json;

    fn row(id: &str, agent: &str, turn: &str, call: Option<&str>) -> RoomEvent {
        let mut row = RoomEvent::new(
            RoomId::new("room").unwrap(),
            EventSource::new(ChannelId::new(agent).unwrap(), ChannelType::AI),
            TextContent::new("Message"),
        );
        row.id = EventId::new(id).unwrap();
        row.responds_to = Some(EventId::new(turn).unwrap());
        if let Some(call) = call {
            row.r#type = EventType::ToolCallStart;
            row.content = EventContent::ToolCall(ToolCallContent::new("action", call));
        }
        row
    }

    fn receipt(id: &str, agent: &str, turn: &str, call: Option<&str>) -> RoomEvent {
        let mut row = row(id, "console", turn, None);
        row.r#type = EventType::Observation;
        row.responds_to = None;
        row.metadata.insert(
            RECEIPT_KEY.into(),
            json!({
                "permission": {
                    "id": id, "actor": agent, "run": format!("{agent}:{turn}"),
                    "title": "Action?", "detail": "Concrete action"
                },
                "decision": "allow", "decided_by": "person:alice"
            }),
        );
        if let Some(call) = call {
            row.metadata.insert(RECEIPT_TOOL_KEY.into(), call.into());
        }
        row
    }

    fn ids(rows: Vec<RoomEvent>) -> Vec<String> {
        replay_order(rows)
            .iter()
            .map(|r| r.id.to_string())
            .collect()
    }

    #[test]
    fn multiple_decisions_follow_their_calls_without_reordering_other_rows() {
        assert_eq!(
            ids(vec![
                row("start1", "agent", "turn", Some("one")),
                row("end1", "agent", "turn", Some("one")),
                row("intermediate", "agent", "turn", None),
                row("start2", "agent", "turn", Some("two")),
                row("end2", "agent", "turn", Some("two")),
                row("answer", "agent", "turn", None),
                receipt("allow1", "agent", "turn", Some("one")),
                receipt("allow2", "agent", "turn", Some("two")),
            ]),
            [
                "start1",
                "allow1",
                "end1",
                "intermediate",
                "start2",
                "allow2",
                "end2",
                "answer"
            ]
        );
    }

    #[test]
    fn missing_calls_or_turns_preserve_receipts_without_using_another_agents_call() {
        assert_eq!(
            ids(vec![
                row("foreign", "other", "turn", Some("one")),
                row("answer", "agent", "turn", None),
                receipt("missing-call", "agent", "turn", Some("one")),
                receipt("missing-turn", "agent", "older-turn", None),
            ]),
            ["foreign", "answer", "missing-call", "missing-turn"]
        );
    }

    #[test]
    fn legacy_receipts_preserve_decision_order_even_without_a_final_answer() {
        for final_answer in [false, true] {
            let mut rows = vec![row("call", "agent", "turn", Some("one"))];
            if final_answer {
                rows.push(row("answer", "agent", "turn", None));
            }
            rows.extend([
                row("other-turn", "agent", "next-turn", None),
                receipt("first", "agent", "turn", None),
                receipt("second", "agent", "turn", None),
            ]);
            let mut expected = vec!["call", "first", "second"];
            if final_answer {
                expected.push("answer");
            }
            expected.push("other-turn");
            assert_eq!(ids(rows), expected);
        }
    }
}
