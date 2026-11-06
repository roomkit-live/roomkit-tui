//! What a conversation cost at the list price its host states.

use std::collections::BTreeMap;

use roomkit::model::{EventType, RoomEvent};
use roomkit::{ConversationStore, GetTimelineOptions, RoomKit};
use roomkit_ai::ModelPricing;
use roomkit_tui_protocol::{ContextUsage, ConversationCost, Event, RunId, TokenUsage};
use serde_json::json;

use crate::{AdapterError, SessionConfig};

/// Rows read at a time while pricing a conversation's history.
const PAGE: u64 = 500;

/// The cost of one turn's reported usage at `pricing`, in its currency; none
/// when the turn did not report both its input and its output.
///
/// A turn's report sums its requests and measures its last one. A price that
/// changes past a request's input, such as OpenAI's long-context rate,
/// applies to the whole turn when its last request went past it, and to none
/// of it otherwise: a turn with requests on both sides is an estimate.
pub fn turn_cost(
    usage: &TokenUsage,
    last: Option<&ContextUsage>,
    pricing: &ModelPricing,
) -> Option<f64> {
    let (input, output) = (usage.input_tokens?, usage.output_tokens?);
    let (read, written) = (usage.cached_input_tokens, usage.cache_write_input_tokens);
    let counters = json!({
        "input_tokens": input.saturating_sub(read).saturating_sub(written),
        "output_tokens": output,
        "cache_read_input_tokens": read,
        "cache_creation_input_tokens": written,
    });
    let mut pricing = pricing.clone();
    if let Some(threshold) = pricing.long_context_threshold_tokens {
        // The kit applies the rate past its threshold, which it refuses at 0.
        let past = last.is_some_and(|last| last.tokens > threshold);
        pricing.long_context_threshold_tokens = past.then_some(1);
    }
    pricing.cost_for(counters.as_object()?).ok()
}

/// The cost of the conversation `config` shows at `config.pricing`, every
/// turn read from the store: what the exit of an application reports. None
/// when the host states no price.
///
/// # Errors
///
/// [`AdapterError`] when the conversation's history cannot be read.
pub async fn conversation_cost<S: ConversationStore>(
    kit: &RoomKit<S>,
    config: &SessionConfig,
) -> Result<Option<ConversationCost>, AdapterError> {
    Ok(restore(kit, config).await?.total(config))
}

/// The turns of one conversation, each priced or not, by the run that
/// answered: a later report of a run replaces its earlier one.
#[derive(Debug, Default)]
pub(crate) struct Costs {
    runs: BTreeMap<RunId, Option<u64>>,
}

impl Costs {
    /// Price the usage `event` reports for its run, when the host states a
    /// price: the conversation's new cost. A turn another model answered, or
    /// one with no whole report, has none, and leaves the cost partial.
    pub(crate) fn record(&mut self, event: &Event, config: &SessionConfig) -> Option<Event> {
        let Event::UsageUpdated {
            conversation,
            run,
            provider,
            model,
            context,
            usage,
        } = event
        else {
            return None;
        };
        let pricing = config.pricing.as_ref()?;
        let shown = config
            .model
            .as_ref()
            .is_some_and(|shown| shown.provider == *provider && shown.model == *model);
        let cost = shown
            .then(|| turn_cost(usage, context.as_ref(), pricing))
            .flatten();
        self.runs.insert(run.clone(), cost.map(micros));
        Some(Event::CostUpdated {
            conversation: conversation.clone(),
            cost: self.total(config)?,
        })
    }

    /// The conversation's cost so far, when the host states a price.
    pub(crate) fn total(&self, config: &SessionConfig) -> Option<ConversationCost> {
        let pricing = config.pricing.as_ref()?;
        let amount = self
            .runs
            .values()
            .flatten()
            .fold(0_u64, |sum, run| sum.saturating_add(*run));
        let complete = self.runs.values().all(Option::is_some);
        Some(ConversationCost::new(
            amount,
            pricing.currency.clone(),
            complete,
        ))
    }
}

/// A cost in millionths of its currency; `as` saturates a cost past `u64`.
fn micros(cost: f64) -> u64 {
    (cost * 1_000_000.0).round().max(0.0) as u64
}

/// Every priced turn of the conversation `config` shows, its whole history
/// read; none read when the host states no price.
pub(crate) async fn restore<S: ConversationStore>(
    kit: &RoomKit<S>,
    config: &SessionConfig,
) -> Result<Costs, AdapterError> {
    let mut costs = Costs::default();
    if config.pricing.is_none() {
        return Ok(costs);
    }
    let mut after = None;
    loop {
        let options = GetTimelineOptions {
            limit: PAGE,
            after_index: after,
            organization_id: config.organization.clone(),
            ..Default::default()
        };
        let page = kit.get_timeline(&config.room, options).await?;
        let Some(last) = page.last() else {
            return Ok(costs);
        };
        after = Some(last.index);
        for row in page.iter().filter(|row| holds_a_turn(row, config)) {
            if let Some(event) = crate::measurements::event(row) {
                costs.record(&event, config);
            }
        }
    }
}

/// Whether `row` holds one of the agent's turns: its message, or the last
/// row of a turn that wrote none, which carries its record.
fn holds_a_turn(row: &RoomEvent, config: &SessionConfig) -> bool {
    row.source.channel_id == config.agent
        && (matches!(row.r#type, EventType::Message)
            || row.metadata.contains_key("loop_end_reason"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "Test fixture failures are assertions")]
    use super::*;
    use roomkit::model::{ChannelId, RoomId};
    use roomkit_ai::Date;
    use roomkit_tui_protocol::ModelInfo;

    /// OpenAI's shape of price: cache reads and writes apart, and a
    /// long-context rate past 272,000 input tokens of a request.
    fn pricing() -> ModelPricing {
        let mut pricing = ModelPricing::new(2.0, 10.0, Date::new(2026, 9, 30).unwrap()).unwrap();
        pricing.cache_read_per_million = Some(0.1);
        pricing.cache_write_per_million = Some(2.5);
        pricing.long_context_threshold_tokens = Some(272_000);
        pricing.long_context_input_multiplier = 2.0;
        pricing.long_context_output_multiplier = 1.5;
        pricing
    }

    /// 10,000 input tokens, 2,000 of them read from the cache and 3,000
    /// written there, and 100 output tokens.
    fn usage() -> TokenUsage {
        let mut usage = TokenUsage::complete(10_000, 100, 2_000);
        usage.cache_write_input_tokens = 3_000;
        usage
    }

    fn config() -> SessionConfig {
        let room = RoomId::new("room").unwrap();
        let mut config = SessionConfig::new(room, ChannelId::new("agent").unwrap(), "person");
        config.model = Some(ModelInfo::new("openai", "model"));
        config.pricing = Some(pricing());
        config
    }

    fn report(run: &str, model: &str, usage: TokenUsage) -> Event {
        Event::UsageUpdated {
            conversation: "room".into(),
            run: run.into(),
            provider: "openai".into(),
            model: model.into(),
            context: Some(ContextUsage::new(10_100)),
            usage,
        }
    }

    fn cost(event: Option<Event>) -> ConversationCost {
        let Some(Event::CostUpdated { cost, .. }) = event else {
            unreachable!("A priced report updates the cost");
        };
        cost
    }

    #[test]
    fn a_turn_bills_its_uncached_input_its_cache_reads_and_writes_and_its_output() {
        // 5,000 × 2.0 + 2,000 × 0.1 + 3,000 × 2.5 + 100 × 10.0, per million.
        let cost = turn_cost(&usage(), Some(&ContextUsage::new(10_100)), &pricing()).unwrap();
        assert!((cost - 0.0187).abs() < 1e-12, "{cost}");
    }

    #[test]
    fn the_long_context_rate_follows_the_turn_s_last_request() {
        // A turn of many requests can sum past the threshold with each one
        // below it: its last request decides.
        let mut many = usage();
        many.input_tokens = Some(300_000);
        let below = turn_cost(&many, Some(&ContextUsage::new(30_000)), &pricing()).unwrap();
        let flat = (295_000.0 * 2.0 + 2_000.0 * 0.1 + 3_000.0 * 2.5 + 100.0 * 10.0) / 1e6;
        assert!((below - flat).abs() < 1e-12, "{below}");
        let past = turn_cost(&usage(), Some(&ContextUsage::new(300_000)), &pricing()).unwrap();
        assert!(
            (past - (17_700.0 * 2.0 + 1_000.0 * 1.5) / 1e6).abs() < 1e-12,
            "{past}"
        );
    }

    #[test]
    fn a_turn_with_no_whole_report_has_no_cost() {
        let mut usage = usage();
        usage.output_tokens = None;
        assert_eq!(turn_cost(&usage, None, &pricing()), None);
    }

    #[test]
    fn a_later_report_of_a_run_replaces_its_cost() {
        let (config, mut costs) = (config(), Costs::default());
        costs.record(&report("r1", "model", usage()), &config);
        let twice = cost(costs.record(&report("r1", "model", usage()), &config));
        assert_eq!(twice, ConversationCost::new(18_700, "USD", true));
        let second = cost(costs.record(&report("r2", "model", usage()), &config));
        assert_eq!(second.micros, 37_400);
    }

    #[test]
    fn a_turn_of_another_model_leaves_the_cost_partial() {
        let (config, mut costs) = (config(), Costs::default());
        costs.record(&report("r1", "model", usage()), &config);
        let other = cost(costs.record(&report("r2", "another", usage()), &config));
        assert_eq!(other, ConversationCost::new(18_700, "USD", false));
    }

    #[test]
    fn no_price_states_no_cost() {
        let mut config = config();
        config.pricing = None;
        let mut costs = Costs::default();
        assert_eq!(costs.record(&report("r1", "model", usage()), &config), None);
        assert_eq!(costs.total(&config), None);
    }
}
