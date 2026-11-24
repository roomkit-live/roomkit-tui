//! Store confirmed decisions after delivery settles, outside model context.
//! Keeping writes out of the decision loop also keeps interruption responsive
//! while the engine owns the room lock. Unwritten receipts remain retryable.
use crate::approvals::Receipt;
use crate::{AdapterError, ConversationChannel, SessionConfig};
use roomkit::model::{
    EventId, EventSource, EventType, ParticipantId, RoomEvent, TextContent, Timestamp, Visibility,
};
use roomkit::{CommitEventOptions, ConversationStore, RoomKit};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

/// Metadata key holding a stored decision receipt.
pub(crate) const RECEIPT_KEY: &str = "roomkit_tui_approval";
/// Metadata key linking a stored receipt to its tool call.
pub(crate) const RECEIPT_TOOL_KEY: &str = "roomkit_tui_approval_tool_id";
/// Prefix of a stored receipt's event identity.
const RECEIPT_ID_PREFIX: &str = "tui-";

/// The writers of one kind of row in each room, as a room's decisions: one
/// at a time in a room, since two would store the same row, and each room
/// apart, since a store waits for its room's lock, which a turn may hold
/// long.
#[derive(Clone, Debug, Default)]
pub(crate) struct Writers(Arc<Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>>);

impl Writers {
    /// Wait for the turn to write in `room`.
    pub(crate) async fn of(&self, room: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let writer = {
            let mut rooms = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            // A room no writer holds nor waits for is forgotten.
            rooms.retain(|_, writer| Arc::strong_count(writer) > 1);
            rooms.entry(room.to_owned()).or_default().clone()
        };
        writer.lock_owned().await
    }
}

/// Store the decisions taken in the room of `config` that are not stored
/// yet, each as a receipt row, linked to its call when it has one.
pub(crate) async fn persist<S: ConversationStore>(
    kit: &RoomKit<S>,
    channel: &ConversationChannel,
    config: &SessionConfig,
) -> Result<(), AdapterError> {
    let _storing = channel.storing_decisions.of(config.room.as_str()).await;
    for saved in channel.approvals.receipts(config.room.as_str()) {
        let permission = saved.decision.permission.id.clone();
        let row = receipt_row(channel, config, saved)?;
        let options = CommitEventOptions {
            organization_id: config.organization.clone(),
        };
        kit.commit_event(&config.room, row, options).await?;
        let room = config.room.as_str();
        channel.approvals.recorded(room, &permission);
    }
    Ok(())
}

/// The row `saved` is stored as: an observation only this channel sees,
/// by the person who decided, named by the request it answers.
fn receipt_row(
    channel: &ConversationChannel,
    config: &SessionConfig,
    saved: Receipt,
) -> Result<RoomEvent, AdapterError> {
    let receipt = saved.decision;
    let mut source = EventSource::new(channel.id().clone(), roomkit::model::ChannelType::WEBSOCKET);
    let person = receipt.decided_by.as_str();
    let person = person.strip_prefix("person:").unwrap_or(person);
    source.participant_id =
        Some(ParticipantId::new(person).map_err(|_| AdapterError::InvalidBinding)?);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AdapterError::InvalidBinding)?
        .as_millis();
    let timestamp =
        Timestamp::from_unix_millis(i64::try_from(now).map_err(|_| AdapterError::InvalidBinding)?)
            .map_err(|_| AdapterError::InvalidBinding)?;
    let content = TextContent::new("Human permission decision");
    let mut row = RoomEvent::new(config.room.clone(), source, content);
    row.id = EventId::new(format!("{RECEIPT_ID_PREFIX}{}", receipt.permission.id))
        .map_err(|_| AdapterError::InvalidBinding)?;
    row.r#type = EventType::Observation;
    row.created_at = timestamp;
    row.visibility =
        Visibility::channels([channel.id().clone()]).map_err(|_| AdapterError::InvalidBinding)?;
    row.metadata
        .insert(RECEIPT_KEY.into(), serde_json::json!(receipt));
    if let Some(tool_id) = saved.tool_call_id {
        row.metadata.insert(RECEIPT_TOOL_KEY.into(), tool_id.into());
    }
    Ok(row)
}

/// Persistence is deferred until the room lock is released. Present receipts
/// beside their calls without rewriting the audit timeline or its timestamps.
/// Call only with the authorized, visible rows of one room.
pub(crate) fn replay_order(mut rows: Vec<RoomEvent>) -> Vec<RoomEvent> {
    rows.sort_by_key(|row| {
        roomkit_tui_protocol::RowPosition::stored(row.index, crate::projection::follows_index(row))
            .key()
    });
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
    fn writing_order_uses_local_rows_and_preserves_unanchored_history() {
        let mut question = row("question", "person", "turn", None);
        let mut peer = row("peer", "screen", "turn", None);
        let mut first = row("waiting", "agent", "turn", None);
        let mut second = row("answer", "agent", "turn", None);
        question.index = 0;
        peer.index = 1;
        first.index = 2;
        second.index = 3;
        let rows = || {
            vec![
                question.clone(),
                peer.clone(),
                first.clone(),
                second.clone(),
            ]
        };
        assert_eq!(ids(rows()), ["question", "peer", "waiting", "answer"]);
        first.metadata.insert("follows_index".into(), json!(0));
        second.metadata.insert("follows_index".into(), json!(2));
        assert_eq!(
            ids(vec![question, peer, first, second]),
            ["question", "waiting", "peer", "answer"]
        );
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
