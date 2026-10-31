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
    /// All input tokens, including cached input, across reported generations.
    pub input_tokens: Option<u64>,
    /// Output tokens across reported generations.
    pub output_tokens: Option<u64>,
    /// Cached subset of input tokens reported by the provider.
    pub cached_input_tokens: u64,
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
            complete: true,
        }
    }
}
