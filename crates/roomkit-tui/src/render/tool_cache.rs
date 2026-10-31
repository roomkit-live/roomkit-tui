//! Cache only tool bodies. State, focus, timers and scoped click targets are
//! always derived from the current session, never from a cached screen row.

use super::cache::{self, line_bytes};
use super::{Line, ToolCall, ToolRenderer, generic_tool, tool_detail_lines};

const EXPANDED_LINES: usize = 100;

/// Rendered tool bodies, one entry per call identity.
#[derive(Default)]
pub(super) struct Cache(cache::Cache<Entry>);

struct Entry {
    call: ToolCall,
    layout: Layout,
    details: Details,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Layout {
    pub width: usize,
    pub preview_lines: usize,
    pub expand: bool,
}

#[derive(Clone)]
pub(super) struct Details {
    pub lines: Vec<Line>,
    pub expandable: bool,
    pub expanded: bool,
    pub truncated: bool,
}

impl Cache {
    pub(super) fn render(
        &mut self,
        key: &str,
        call: &ToolCall,
        layout: Layout,
        renderer: Option<&dyn ToolRenderer>,
    ) -> Details {
        if let Some(entry) = self.0.get(key)
            && entry.layout == layout
            && entry.call == *call
        {
            return entry.details.clone();
        }
        let details = render(call, layout, renderer);
        // Account for source data too, including structured JSON. A small
        // preview must not retain an arbitrarily large tool result in cache.
        let bytes = call.name.len()
            + call.intent.as_ref().map_or(0, String::len)
            + call.text.len()
            + json_bytes(&call.arguments)
            + call.data.as_ref().map_or(0, json_bytes)
            + line_bytes(&details.lines);
        let entry = Entry {
            call: call.clone(),
            layout,
            details: details.clone(),
        };
        self.0.insert(key.into(), entry, bytes);
        details
    }
}

fn render(call: &ToolCall, layout: Layout, renderer: Option<&dyn ToolRenderer>) -> Details {
    let styled = |expanded| {
        renderer.map_or_else(
            || generic_tool(call, expanded),
            |renderer| renderer.render_styled(call, expanded),
        )
    };
    let (preview, clipped) = tool_detail_lines(&styled(false), layout.width, layout.preview_lines);
    let expandable = clipped
        || renderer.map_or_else(
            || call.data.is_some() && !call.text.is_empty(),
            |renderer| renderer.has_expanded_details(call),
        );
    let expanded = expandable && layout.expand;
    let (lines, truncated) = if expanded {
        tool_detail_lines(&styled(true), layout.width, EXPANDED_LINES)
    } else {
        (preview, clipped)
    };
    Details {
        lines,
        expandable,
        expanded,
        truncated,
    }
}

fn json_bytes(value: &serde_json::Value) -> usize {
    use serde_json::Value;
    std::mem::size_of::<Value>()
        + match value {
            Value::String(text) => text.len(),
            Value::Array(values) => values.iter().map(json_bytes).sum(),
            Value::Object(values) => values
                .iter()
                .map(|(key, value)| key.len() + json_bytes(value))
                .sum(),
            _ => 0,
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roomkit_tui_protocol::ToolState;
    use serde_json::json;

    struct Preview;
    impl ToolRenderer for Preview {
        fn render(&self, _: &ToolCall) -> Vec<String> {
            vec!["Small preview".into()]
        }
        fn has_expanded_details(&self, _: &ToolCall) -> bool {
            false
        }
    }

    const LAYOUT: Layout = Layout {
        width: 80,
        preview_lines: 4,
        expand: false,
    };

    fn call(data: serde_json::Value) -> ToolCall {
        ToolCall::new("call", "fixture", json!({}), ToolState::Succeeded).with_data(data)
    }

    #[test]
    fn large_source_data_is_not_retained_just_because_its_preview_is_small() {
        let mut cache = Cache::default();
        let large = call(json!({"large": "x".repeat(4 * 1024 * 1024)}));
        cache.render("call", &large, LAYOUT, Some(&Preview));
        assert_eq!(cache.0.len(), 0);
        assert_eq!(cache.0.bytes(), 0);
    }

    #[test]
    fn a_changed_call_replaces_its_own_entry() {
        let mut cache = Cache::default();
        for revision in 0..10 {
            let details = cache.render("call", &call(json!({"revision": revision})), LAYOUT, None);
            assert!(
                details
                    .lines
                    .iter()
                    .flatten()
                    .any(|s| s.text.contains(&revision.to_string()))
            );
        }
        assert_eq!(cache.0.len(), 1);
    }
}
