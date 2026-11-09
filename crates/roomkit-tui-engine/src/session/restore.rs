//! A bounded projection of a room, restored from the engine for one view.

use std::collections::BTreeMap;

use roomkit::model::{
    Access, ChannelBinding, ChannelCategory, EventId, EventType, RoomContext, RoomEvent,
};
use roomkit::{
    ConversationStore, GetRoomOptions, GetTimelineOptions, ListBindingsOptions, RoomKit,
};
use roomkit_tui_protocol::{Actor, ActorId, ContextUsage, Item, Snapshot, TokenUsage};

use super::{AdapterError, SessionConfig, steer};
use crate::{ConversationChannel, approval_history, measurements, projection};

/// Rows restored into a snapshot; older history stays in the engine.
const RESTORED_ROWS: u64 = 500;

pub(crate) async fn snapshot<S: ConversationStore>(
    kit: &RoomKit<S>,
    channel: &ConversationChannel,
    config: &SessionConfig,
) -> Result<Snapshot, AdapterError> {
    let organization = config.organization.clone();
    let room = kit
        .get_room(
            &config.room,
            GetRoomOptions {
                organization_id: organization.clone(),
            },
        )
        .await?;
    let options = ListBindingsOptions {
        organization_id: organization.clone(),
    };
    let bindings = kit.list_bindings(&config.room, options).await?;
    let readable = can_read(&bindings, channel);
    let agent = bindings
        .iter()
        .any(|b| b.channel_id == config.agent && b.category == ChannelCategory::Intelligence);
    if !readable || !agent || config.sender.is_empty() {
        return Err(AdapterError::InvalidBinding);
    }
    // A closed room refuses new rows. Decisions that cannot be stored stay
    // retryable and are still shown, so the view never fails to load.
    let _ = approval_history::persist(kit, channel, config).await;
    let timeline = GetTimelineOptions {
        newest_first: true,
        limit: RESTORED_ROWS,
        organization_id: organization,
        ..Default::default()
    };
    let rows = kit.get_timeline(&config.room, timeline).await?;
    let mut context = RoomContext::new(room);
    context.bindings = bindings;
    context.recent_events = rows;
    let mut history = History::default();
    let visible = roomkit::visible_events(&context, channel.id());
    for row in approval_history::replay_order(visible) {
        history.add(&row, config);
    }
    for receipt in channel.approvals.receipts(config.conversation()) {
        projection::upsert(&mut history.items, Item::Approval(receipt.decision));
    }
    for binding in &context.bindings {
        let id = binding.channel_id.as_str();
        history
            .names
            .entry(id.into())
            .or_insert_with(|| id.to_owned());
    }
    let mut snapshot = history.into_snapshot(config);
    // Only a streaming console follows the turn a message goes to.
    snapshot.can_steer = channel.streams() && steer::agent_steers(kit, config);
    snapshot.can_answer = snapshot.can_steer;
    Ok(snapshot)
}

/// Whether `channel` may read the room.
fn can_read(bindings: &[ChannelBinding], channel: &ConversationChannel) -> bool {
    bindings.iter().any(|b| {
        &b.channel_id == channel.id() && matches!(b.access, Access::ReadOnly | Access::ReadWrite)
    })
}

/// What a turn's own record measured, on the model on screen, the row it rides
/// found by the event the turn answers: another turn's record says nothing of
/// this one. None when the turn wrote no record.
pub(crate) async fn turn_measure<S: ConversationStore>(
    kit: &RoomKit<S>,
    channel: &ConversationChannel,
    config: &SessionConfig,
    answers: &EventId,
) -> Option<(Option<ContextUsage>, TokenUsage)> {
    let organization = config.organization.clone();
    let room = GetRoomOptions {
        organization_id: organization.clone(),
    };
    let room = kit.get_room(&config.room, room).await.ok()?;
    let bindings = ListBindingsOptions {
        organization_id: organization.clone(),
    };
    let bindings = kit.list_bindings(&config.room, bindings).await.ok()?;
    if !can_read(&bindings, channel) {
        return None;
    }
    let timeline = GetTimelineOptions {
        newest_first: true,
        limit: RESTORED_ROWS,
        organization_id: organization,
        ..Default::default()
    };
    let mut context = RoomContext::new(room);
    context.bindings = bindings;
    context.recent_events = kit.get_timeline(&config.room, timeline).await.ok()?;
    roomkit::visible_events(&context, channel.id())
        .iter()
        .filter(|row| row.responds_to.as_ref() == Some(answers))
        .find_map(|row| {
            let (context, usage) = measurements::restore(row, config);
            Some((context, usage?))
        })
}

/// The projection of visible rows: items, actor names and the latest usage.
#[derive(Default)]
struct History {
    items: Vec<Item>,
    names: BTreeMap<ActorId, String>,
    context: Option<ContextUsage>,
    usage: Option<TokenUsage>,
}

impl History {
    fn add(&mut self, row: &RoomEvent, config: &SessionConfig) {
        let actor = projection::actor(row);
        self.names.entry(actor).or_insert_with(|| {
            row.source
                .participant_id
                .as_ref()
                .map_or_else(|| row.source.channel_id.to_string(), ToString::to_string)
        });
        // A person's message says nothing of the measure; an agent's replaces
        // it, even with none, and so does any row of its that holds a turn's
        // record, one that wrote no message: no stale measure reads as current.
        let measures = matches!(row.r#type, EventType::Message)
            || row.metadata.contains_key("loop_end_reason");
        if measures && row.source.channel_id == config.agent {
            (self.context, self.usage) = measurements::restore(row, config);
        }
        if let Some(item) = projection::item(row) {
            projection::upsert(&mut self.items, item);
        }
    }

    fn into_snapshot(self, config: &SessionConfig) -> Snapshot {
        let agent = Actor::new(config.agent.as_str(), config.agent.as_str());
        let mut snapshot = Snapshot::new(config.conversation(), config.title.clone(), agent);
        snapshot.actors = self
            .names
            .into_iter()
            .map(|(id, name)| {
                let name = config.actor_names.get(&id).cloned().unwrap_or(name);
                Actor::new(id, name)
            })
            .collect();
        snapshot.local_actor = Some(ActorId::new(format!("person:{}", config.sender)));
        snapshot.model.clone_from(&config.model);
        snapshot.context = self.context;
        snapshot.usage = self.usage;
        snapshot.commands.clone_from(&config.commands);
        snapshot.input_actions.clone_from(&config.input_actions);
        snapshot.cycle_command.clone_from(&config.cycle_command);
        snapshot.can_attach_images = config.can_attach_images;
        snapshot.mode.clone_from(&config.mode);
        snapshot.items = self.items;
        snapshot.can_interrupt = true;
        snapshot
    }
}
