//! Model settings and usage measurements, only for the model on screen.

use roomkit_tui_protocol::limits::MAX_LABEL_BYTES;
use roomkit_tui_protocol::{
    ContextUsage, ConversationCost, ConversationId, Event, ModelInfo, RunId, TokenUsage,
};

use super::Session;
use super::bounds::{bound_in_place, bound_model, bounded};

impl Session {
    /// Apply a model, measurement or cost event; any other event is ignored.
    pub(super) fn apply_measurement(&mut self, event: Event) {
        match event {
            Event::ModelChanged {
                conversation,
                model,
            } => self.on_model_changed(&conversation, model),
            Event::UsageUpdated {
                conversation,
                run,
                provider,
                model,
                context,
                usage,
            } => self.on_usage(&conversation, &run, (&provider, &model), context, usage),
            Event::ContextUpdated {
                conversation,
                run,
                provider,
                model,
                usage,
            } => self.on_context(&conversation, &run, (&provider, &model), usage),
            Event::ContextExceeded { conversation, run } => {
                self.on_context_exceeded(&conversation, &run);
            }
            Event::ContextEstimated {
                conversation,
                provider,
                model,
                usage,
            } => self.on_context_estimated(&conversation, (&provider, &model), usage),
            Event::CostUpdated { conversation, cost } => self.on_cost(&conversation, cost),
            Event::RelatedCostUpdated { conversation, cost } => {
                self.on_related_cost(&conversation, cost);
            }
            _ => {}
        }
    }

    fn on_model_changed(&mut self, conversation: &ConversationId, mut model: ModelInfo) {
        if !self.is_current(conversation) {
            return;
        }
        bound_model(&mut model);
        if let Some(snapshot) = &mut self.snapshot {
            snapshot.model = Some(model);
            snapshot.context = None;
            snapshot.usage = None;
        }
        self.previous_context = None;
        self.context_exceeded = false;
        self.known_estimate = None;
        self.held_estimate = None;
    }

    fn measures_current_model(
        &self,
        conversation: &ConversationId,
        run: &RunId,
        measured: (&str, &str),
    ) -> bool {
        self.is_current(conversation)
            && self.last_run.as_ref() == Some(run)
            && self.shows_model(measured)
    }

    /// Whether `provider` and `model` are those on screen.
    fn shows_model(&self, (provider, model): (&str, &str)) -> bool {
        self.snapshot
            .as_ref()
            .and_then(|s| s.model.as_ref())
            .is_some_and(|m| {
                // Labels in the view were shortened the same way.
                m.provider == bounded(provider, MAX_LABEL_BYTES)
                    && m.model == bounded(model, MAX_LABEL_BYTES)
            })
    }

    fn on_usage(
        &mut self,
        conversation: &ConversationId,
        run: &RunId,
        measured: (&str, &str),
        context: Option<ContextUsage>,
        usage: TokenUsage,
    ) {
        if !self.measures_current_model(conversation, run, measured) {
            return;
        }
        if context.is_some() {
            self.measured_over_estimate();
        }
        if let Some(snapshot) = &mut self.snapshot {
            if let Some(previous) = snapshot.context.take() {
                self.previous_context = Some(previous);
            }
            snapshot.context = context;
            snapshot.usage = Some(usage);
        }
    }

    fn on_context(
        &mut self,
        conversation: &ConversationId,
        run: &RunId,
        measured: (&str, &str),
        usage: ContextUsage,
    ) {
        if !self.measures_current_model(conversation, run, measured) {
            return;
        }
        self.measured_over_estimate();
        if let Some(snapshot) = &mut self.snapshot {
            snapshot.context = Some(usage);
        }
    }

    /// A refusal past the context window, kept only for the latest run,
    /// like its measurements.
    fn on_context_exceeded(&mut self, conversation: &ConversationId, run: &RunId) {
        if self.is_current(conversation) && self.last_run.as_ref() == Some(run) {
            self.measured_over_estimate();
            self.context_exceeded = true;
        }
    }

    /// Take the host's estimate as the conversation's context, unless a
    /// newer measure replaced it: the estimate it already knows, sent again
    /// with the host's state, replaces nothing. A live run's measures win, so
    /// an estimate told while it goes waits for the room to be idle.
    fn on_context_estimated(
        &mut self,
        conversation: &ConversationId,
        estimated: (&str, &str),
        mut usage: ContextUsage,
    ) {
        usage.estimated = true;
        if !self.is_current(conversation)
            || !self.shows_model(estimated)
            || self.known_estimate.as_ref() == Some(&usage)
        {
            return;
        }
        if self.active_run().is_some() {
            self.held_estimate = Some(usage);
        } else {
            self.show_estimate(usage);
        }
    }

    /// Show `usage`, the host's estimate, as the conversation's context.
    fn show_estimate(&mut self, usage: ContextUsage) {
        if let Some(snapshot) = &mut self.snapshot {
            snapshot.context = Some(usage.clone());
        }
        // The estimate postdates the refused request: the refusal no longer
        // describes what the gauge shows.
        self.context_exceeded = false;
        self.known_estimate = Some(usage);
    }

    /// A run measured, or was refused, after the estimate held: the estimate
    /// is older, and known, so it is not shown when sent again.
    fn measured_over_estimate(&mut self) {
        if let Some(held) = self.held_estimate.take() {
            self.known_estimate = Some(held);
        }
    }

    /// Once the room is idle, show the estimate told while a run went that
    /// measured nothing since.
    pub(super) fn show_held_estimate(&mut self) {
        if let Some(held) = self.held_estimate.take() {
            self.show_estimate(held);
        }
    }

    /// The conversation's cost, which a new execution keeps: it sums every
    /// turn of the conversation, not the latest one.
    fn on_cost(&mut self, conversation: &ConversationId, mut cost: ConversationCost) {
        if !self.is_current(conversation) {
            return;
        }
        bound_in_place(&mut cost.currency, MAX_LABEL_BYTES);
        if let Some(snapshot) = &mut self.snapshot {
            snapshot.cost = Some(cost);
        }
    }

    /// What the work related to the conversation cost, which the next total
    /// adds to its own.
    fn on_related_cost(
        &mut self,
        conversation: &ConversationId,
        mut cost: Option<ConversationCost>,
    ) {
        if !self.is_current(conversation) {
            return;
        }
        if let Some(cost) = &mut cost {
            bound_in_place(&mut cost.currency, MAX_LABEL_BYTES);
        }
        if let Some(snapshot) = &mut self.snapshot {
            snapshot.related_cost = cost;
        }
    }
}
