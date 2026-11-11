use roomkit::model::{Metadata, RoomEvent, UsageMarker};
use roomkit_tui_protocol::{ContextUsage, Event, RunId, TokenUsage};
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
    let usage = {
        let mut usage = TokenUsage::default();
        usage.input_tokens = input_of(total);
        usage.output_tokens = total.get("output_tokens").and_then(Value::as_u64);
        usage.cached_input_tokens = counter(total, "cache_read_input_tokens");
        usage.cache_write_input_tokens = counter(total, "cache_creation_input_tokens");
        usage.complete = metadata
            .get("ai_usage_complete")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        usage
    };
    let context = metadata
        .get("ai_last_usage")
        .and_then(Value::as_object)
        .and_then(context_of);
    Some(Event::UsageUpdated {
        conversation: row.room_id.as_str().into(),
        run: crate::projection::run_id(&row.source.channel_id, row.responds_to.as_ref()?),
        provider,
        model,
        context,
        usage,
    })
}

/// What a generation told of itself, as the context it filled: none when it
/// reported no counters, or does not name what it ran on.
pub(crate) fn told(room: &str, run: &RunId, marker: &UsageMarker) -> Option<Event> {
    Some(Event::ContextUpdated {
        conversation: room.into(),
        run: run.clone(),
        provider: marker.provider.clone()?,
        model: marker.model.clone()?,
        usage: context_of(&marker.usage)?,
    })
}

/// The context a request filled, as what it reported: its whole input and
/// its output.
fn context_of(last: &Metadata) -> Option<ContextUsage> {
    Some(ContextUsage::new(
        input_of(last)?.checked_add(last.get("output_tokens")?.as_u64()?)?,
    ))
}

/// A report's whole input: what it billed uncached, what it read from the
/// prompt cache and what it wrote there, which a Chat Completions usage
/// reports apart from its `input_tokens` since roomkit-rs f86cbb1; none when
/// it reported no input.
fn input_of(report: &Metadata) -> Option<u64> {
    let input = report.get("input_tokens")?.as_u64()?;
    Some(
        ["cache_read_input_tokens", "cache_creation_input_tokens"]
            .into_iter()
            .fold(input, |sum, key| sum.saturating_add(counter(report, key))),
    )
}

/// A counter of a report, 0 when it reported none.
fn counter(report: &Metadata, key: &str) -> u64 {
    report.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// Give a context measured or estimated on the configured model the
/// capacity the session configured. One the host gave a capacity keeps it
/// when the session configured none.
pub(crate) fn for_view(event: &mut Event, config: &SessionConfig) {
    let (provider, model, context) = match event {
        Event::UsageUpdated {
            provider,
            model,
            context: Some(context),
            ..
        } => (provider, model, context),
        Event::ContextUpdated {
            provider,
            model,
            usage,
            ..
        }
        | Event::ContextEstimated {
            provider,
            model,
            usage,
            ..
        } => (provider, model, usage),
        _ => return,
    };
    let configured = config.context_capacity.filter(|capacity| *capacity > 0);
    if let Some(capacity) = configured
        && config
            .model
            .as_ref()
            .is_some_and(|selected| selected.provider == *provider && selected.model == *model)
    {
        context.capacity = Some(capacity);
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

    /// RMK-1646: the session states the configured model's capacity; with
    /// none configured, a host's estimate keeps the capacity it gave.
    #[test]
    fn an_estimate_keeps_its_capacity_unless_the_session_configured_one() {
        use roomkit_tui_protocol::ModelInfo;
        let estimate = |capacity| Event::ContextEstimated {
            conversation: "room".into(),
            provider: "local".into(),
            model: "fixture".into(),
            usage: ContextUsage::new(92_000).with_capacity(capacity),
        };
        let mut config = SessionConfig::new(
            RoomId::new("room").unwrap(),
            ChannelId::new("agent").unwrap(),
            "alice",
        );
        config.model = Some(ModelInfo::new("local", "fixture"));
        let mut event = estimate(64_000);
        for_view(&mut event, &config);
        assert_eq!(event, estimate(64_000));
        config.context_capacity = Some(200_000);
        for_view(&mut event, &config);
        assert_eq!(event, estimate(200_000));
    }

    /// RMK-1457: since roomkit-rs f86cbb1 a Chat Completions usage reports the
    /// tokens a prompt wrote to the cache apart from its `input_tokens`, and
    /// the turn's input and the context read them short.
    #[test]
    fn a_prompt_s_cache_writes_count_in_its_input_and_its_context() {
        let report = json!({
            "input_tokens": 5000,
            "cache_read_input_tokens": 2000,
            "cache_creation_input_tokens": 3000,
            "output_tokens": 10,
        });
        let mut row = RoomEvent::new(
            RoomId::new("room").unwrap(),
            EventSource::new(ChannelId::new("agent").unwrap(), ChannelType::AI),
            TextContent::new("Done"),
        );
        row.responds_to = Some(EventId::new("evt_1").unwrap());
        row.metadata.insert("ai_provider".into(), json!("openai"));
        row.metadata.insert("ai_model".into(), json!("model"));
        row.metadata.insert("ai_usage".into(), report.clone());
        row.metadata.insert("ai_last_usage".into(), report.clone());
        let Some(Event::UsageUpdated { context, usage, .. }) = event(&row) else {
            unreachable!("The row reports usage");
        };
        assert_eq!(usage.input_tokens, Some(10_000));
        assert_eq!(usage.cached_input_tokens, 2000);
        assert_eq!(context.map(|context| context.tokens), Some(10_010));
        let marker = UsageMarker {
            provider: Some("openai".into()),
            model: Some("model".into()),
            usage: report.as_object().unwrap().clone(),
        };
        let run = crate::projection::run_id(&row.source.channel_id, "evt_1");
        let Some(Event::ContextUpdated { usage, .. }) = told("room", &run, &marker) else {
            unreachable!("The generation told its counters");
        };
        assert_eq!(usage.tokens, 10_010);
    }
}
