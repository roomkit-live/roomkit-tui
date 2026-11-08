//! Optional tool presentations registered by the application, and the caches
//! of rendered bodies they share.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;

use roomkit_tui_protocol::{ActorId, CallId, RunId, ToolCall};

use super::{Line, Tone, line, span, tool_cache};
use crate::UiOptions;

/// A host-registered tool presentation. Return plain text, never terminal codes.
/// The terminal still sanitizes and bounds every returned line.
/// Presentations must be deterministic for a given call: unchanged results are
/// cached. Register the renderer again when its presentation settings change.
pub trait ToolRenderer: Send + Sync {
    /// Render details from structured tool output.
    fn render(&self, call: &ToolCall) -> Vec<String>;
    /// Render expanded details. Defaults to the same presentation; the terminal
    /// never adds raw fields that a custom presenter chose to omit.
    fn render_expanded(&self, call: &ToolCall) -> Vec<String> {
        self.render(call)
    }
    /// Optional semantic styling, with the same sanitation and layout bounds as
    /// plain presentations. Existing implementations need not implement this.
    fn render_styled(&self, call: &ToolCall, expanded: bool) -> Vec<Line> {
        let details = if expanded {
            self.render_expanded(call)
        } else {
            self.render(call)
        };
        details
            .into_iter()
            .map(|text| line(text, Tone::Text))
            .collect()
    }
    /// Whether the expanded presentation adds content beyond the preview.
    /// The default compares presentations. Override with a cheap schema-aware
    /// check for large results. Width-related preview clipping is handled by
    /// the terminal separately and always remains expandable.
    fn has_expanded_details(&self, call: &ToolCall) -> bool {
        self.render_styled(call, false) != self.render_styled(call, true)
    }
    /// The call in a few words, read from its arguments, for the activity line
    /// of a call that carries no public intention. `None` or a blank text shows
    /// the tool's name. Called on each frame, uncached: keep it cheap.
    fn activity(&self, _call: &ToolCall) -> Option<String> {
        None
    }
}

/// Optional presentations keyed by tool name. Unknown tools use generic output.
#[derive(Default)]
pub struct Renderers {
    tools: BTreeMap<String, Box<dyn ToolRenderer>>,
    markdown: Mutex<crate::markdown::Cache>,
    tool_cache: Mutex<tool_cache::Cache>,
}

impl fmt::Debug for Renderers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Renderers")
            .field("tools", &self.tools.keys())
            .finish()
    }
}

impl Renderers {
    /// Register or replace one tool's presentation.
    pub fn register(&mut self, name: impl Into<String>, renderer: impl ToolRenderer + 'static) {
        self.tools.insert(name.into(), Box::new(renderer));
        *self
            .tool_cache
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = tool_cache::Cache::default();
    }

    /// What the activity line shows of a call without a public intention.
    pub(super) fn activity(&self, call: &ToolCall) -> Option<String> {
        self.tools.get(&call.name)?.activity(call)
    }

    pub(super) fn tool_details(
        &self,
        key: (&ActorId, &RunId, &CallId),
        call: &ToolCall,
        cols: usize,
        options: &UiOptions,
        expand: bool,
    ) -> tool_cache::Details {
        let layout = tool_cache::Layout {
            width: cols.saturating_sub(6),
            preview_lines: options.tool_preview_lines.clamp(1, 8),
            expand,
        };
        // Expanded and collapsed bodies are kept apart: toggling is common.
        let key = format!("{}\u{0}{}\u{0}{}\u{0}{expand}", key.0, key.1, key.2);
        let mut details = self
            .tool_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .render(
                &key,
                call,
                layout,
                self.tools.get(&call.name).map(AsRef::as_ref),
            );
        for line in &mut details.lines {
            line.insert(0, span("   │ ", Tone::Border));
        }
        details
    }

    pub(super) fn markdown(
        &self,
        key: &str,
        source: &str,
        width: usize,
        markdown: bool,
    ) -> Vec<Line> {
        self.markdown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .render(key, source, width, markdown)
    }
}
