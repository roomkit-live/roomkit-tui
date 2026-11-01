use roomkit::model::RoomEvent;
use roomkit_tui_protocol::{ContextUsage, Event, TokenUsage};
use serde_json::Value;

use crate::SessionConfig;

pub(crate) fn event(row: &RoomEvent) -> Option<Event> {
    let metadata = &row.metadata;
    let total = metadata.get("ai_usage")?.as_object()?;
    if total.is_empty() {
        return None;
    }
    let provider = metadata.get("ai_provider")?.as_str()?.to_owned();
    let model = metadata.get("ai_model")?.as_str()?.to_owned();
    let cached = total
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let usage = {
        let mut usage = TokenUsage::default();
        usage.input_tokens = total
            .get("input_tokens")
            .and_then(Value::as_u64)
            .map(|input| input.saturating_add(cached));
        usage.output_tokens = total.get("output_tokens").and_then(Value::as_u64);
        usage.cached_input_tokens = cached;
        usage.complete = metadata
            .get("ai_usage_complete")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        usage
    };
    let context = metadata
        .get("ai_last_usage")
        .and_then(Value::as_object)
        .and_then(|last| {
            let input = last.get("input_tokens")?.as_u64()?;
            let cached = last
                .get("cache_read_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            Some(ContextUsage::new(
                input
                    .checked_add(cached)?
                    .checked_add(last.get("output_tokens")?.as_u64()?)?,
            ))
        });
    Some(Event::UsageUpdated {
        conversation: row.room_id.as_str().into(),
        run: crate::projection::run_id(&row.source.channel_id, row.responds_to.as_ref()?),
        provider,
        model,
        context,
        usage,
    })
}

pub(crate) fn for_view(event: &mut Event, config: &SessionConfig) {
    if let Event::UsageUpdated {
        provider,
        model,
        context: Some(context),
        ..
    } = event
        && config
            .model
            .as_ref()
            .is_some_and(|selected| selected.provider == *provider && selected.model == *model)
    {
        context.capacity = config.context_capacity.filter(|capacity| *capacity > 0);
    }
}

pub(crate) fn restore(
    row: &RoomEvent,
    config: &SessionConfig,
) -> (Option<ContextUsage>, Option<TokenUsage>) {
    let Some(mut event) = event(row) else {
        return (None, None);
    };
    for_view(&mut event, config);
    if let Event::UsageUpdated {
        provider,
        model,
        context,
        usage,
        ..
    } = event
        && row.source.channel_id == config.agent
        && config
            .model
            .as_ref()
            .is_some_and(|selected| selected.provider == provider && selected.model == model)
    {
        return (context, Some(usage));
    }
    (None, None)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "Test fixture failures are assertions")]
    use super::*;
    use roomkit::model::{ChannelId, ChannelType, EventId, EventSource, RoomId, TextContent};
    use roomkit_tui_protocol::limits::MAX_LABEL_BYTES;
    use serde_json::json;

    /// RMK-1079: usage built its run id by hand, so with a long agent id it
    /// named a run the terminal never knew and was dropped.
    #[test]
    fn usage_names_the_same_bounded_run_as_the_turn() {
        let agent = ChannelId::new("a".repeat(250)).unwrap();
        let mut row = RoomEvent::new(
            RoomId::new("room").unwrap(),
            EventSource::new(agent.clone(), ChannelType::AI),
            TextContent::new("Done"),
        );
        row.responds_to = Some(EventId::new("evt_1").unwrap());
        row.metadata.insert("ai_provider".into(), json!("openai"));
        row.metadata.insert("ai_model".into(), json!("model"));
        row.metadata.insert(
            "ai_usage".into(),
            json!({"input_tokens": 3, "output_tokens": 2}),
        );
        let Some(Event::UsageUpdated { run, .. }) = event(&row) else {
            unreachable!("The row reports usage");
        };
        assert_eq!(run, crate::projection::run_id(&agent, "evt_1"));
        assert!(run.as_str().len() <= MAX_LABEL_BYTES);
    }
}
