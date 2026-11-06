//! Effective model settings and the host's usage measurements.

use serde::{Deserialize, Serialize};

/// Effective model settings, confirmed by the host after applying a selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelInfo {
    /// Provider identity; part of the identity of this model. It must match
    /// the provider the engine reports in usage measurements.
    pub provider: String,
    /// Provider-specific model identity.
    pub model: String,
    /// Effective effort, if supported and known. `None` displays as unavailable.
    pub effort: Option<String>,
}

impl ModelInfo {
    /// A provider and model, with unknown effort.
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            effort: None,
        }
    }

    /// Set the effective effort.
    #[must_use]
    pub fn with_effort(mut self, effort: impl Into<String>) -> Self {
        self.effort = Some(effort.into());
        self
    }
}

/// Context occupancy of a single request, not cumulative billed token usage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ContextUsage {
    /// Tokens occupying the measured context, including cached input tokens.
    /// Hosts report their measurement boundary (for example input plus output
    /// of the last completed generation), rather than cumulative turn usage.
    pub tokens: u64,
    /// Model context capacity when known; zero is treated as unknown.
    pub capacity: Option<u64>,
    /// Whether the host estimated this measurement.
    pub estimated: bool,
}

impl ContextUsage {
    /// A measured occupancy with unknown capacity.
    pub fn new(tokens: u64) -> Self {
        Self {
            tokens,
            capacity: None,
            estimated: false,
        }
    }

    /// Set the model's context capacity.
    #[must_use]
    pub fn with_capacity(mut self, capacity: u64) -> Self {
        self.capacity = Some(capacity);
        self
    }

    /// Mark the measurement as a host estimate.
    #[must_use]
    pub fn estimated(mut self) -> Self {
        self.estimated = true;
        self
    }

    /// Occupancy percentage, absent when the capacity is unknown or zero.
    /// Values above 100 are retained rather than hiding overflow.
    pub fn percent(&self) -> Option<f64> {
        self.capacity
            .filter(|capacity| *capacity != 0)
            .map(|capacity| self.tokens as f64 / capacity as f64 * 100.0)
    }
}

/// Provider-reported consumption over the latest execution, not occupancy.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TokenUsage {
    /// All input tokens, including what was read from or written to the
    /// prompt cache, across reported generations.
    pub input_tokens: Option<u64>,
    /// Output tokens across reported generations.
    pub output_tokens: Option<u64>,
    /// Cached subset of input tokens reported by the provider.
    pub cached_input_tokens: u64,
    /// Subset of input tokens the provider wrote to its prompt cache, which
    /// some providers bill apart; 0 when it reported none. Older hosts omit it.
    #[serde(default)]
    pub cache_write_input_tokens: u64,
    /// False when at least one generation lacked counts or failed.
    pub complete: bool,
}

impl TokenUsage {
    /// Complete counts for every generation of the execution.
    pub fn complete(input_tokens: u64, output_tokens: u64, cached_input_tokens: u64) -> Self {
        Self {
            input_tokens: Some(input_tokens),
            output_tokens: Some(output_tokens),
            cached_input_tokens,
            cache_write_input_tokens: 0,
            complete: true,
        }
    }
}

/// What a conversation's turns cost at the list price the host states for
/// its model: an estimate, never an invoice.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ConversationCost {
    /// The amount, in millionths of `currency`.
    pub micros: u64,
    /// The ISO 4217 code of the price's currency, such as `USD`.
    pub currency: String,
    /// False when a turn of the conversation has no price: another model
    /// answered it, or its usage was not reported.
    pub complete: bool,
}

impl ConversationCost {
    /// `micros` millionths of `currency`, every turn priced or not.
    pub fn new(micros: u64, currency: impl Into<String>, complete: bool) -> Self {
        Self {
            micros,
            currency: currency.into(),
            complete,
        }
    }
}
