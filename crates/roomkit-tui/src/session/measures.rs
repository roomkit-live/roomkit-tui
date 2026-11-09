//! Model settings and usage measurements, only for the model on screen.

use roomkit_tui_protocol::limits::MAX_LABEL_BYTES;
use roomkit_tui_protocol::{
    ContextUsage, ConversationCost, ConversationId, ModelInfo, RunId, TokenUsage,
};

use super::Session;
use super::bounds::{bound_in_place, bound_model, bounded};

impl Session {
    pub(super) fn on_model_changed(&mut self, conversation: &ConversationId, mut model: ModelInfo) {
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
    }

    fn measures_current_model(
        &self,
        conversation: &ConversationId,
        run: &RunId,
        (provider, model): (&str, &str),
    ) -> bool {
        self.is_current(conversation)
            && self.last_run.as_ref() == Some(run)
            && self
                .snapshot
                .as_ref()
                .and_then(|s| s.model.as_ref())
                .is_some_and(|m| {
                    // Labels in the view were shortened the same way.
                    m.provider == bounded(provider, MAX_LABEL_BYTES)
                        && m.model == bounded(model, MAX_LABEL_BYTES)
                })
    }

    pub(super) fn on_usage(
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
        if let Some(snapshot) = &mut self.snapshot {
            if let Some(previous) = snapshot.context.take() {
                self.previous_context = Some(previous);
            }
            snapshot.context = context;
            snapshot.usage = Some(usage);
        }
    }

    pub(super) fn on_context(
        &mut self,
        conversation: &ConversationId,
        run: &RunId,
        measured: (&str, &str),
        usage: ContextUsage,
    ) {
        if self.measures_current_model(conversation, run, measured)
            && let Some(snapshot) = &mut self.snapshot
        {
            snapshot.context = Some(usage);
        }
    }

    /// A refusal past the context window, kept only for the latest run,
    /// like its measurements.
    pub(super) fn on_context_exceeded(&mut self, conversation: &ConversationId, run: &RunId) {
        if self.is_current(conversation) && self.last_run.as_ref() == Some(run) {
            self.context_exceeded = true;
        }
    }

    /// The conversation's cost, which a new execution keeps: it sums every
    /// turn of the conversation, not the latest one.
    pub(super) fn on_cost(&mut self, conversation: &ConversationId, mut cost: ConversationCost) {
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
    pub(super) fn on_related_cost(
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
