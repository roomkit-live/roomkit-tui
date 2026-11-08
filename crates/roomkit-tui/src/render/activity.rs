//! A presentation of consecutive calls; the underlying history stays intact.

use std::time::Duration;

use roomkit_tui_protocol::{ActorId, ConversationId, Item, RunId, ToolCall, ToolState};
use unicode_width::UnicodeWidthStr;

use super::text::fit;
use super::{Line, Renderers, Tone, ToolTarget, span, spinner};
use crate::{Session, UiOptions};

pub(super) struct Group<'a> {
    pub len: usize,
    pub actor: &'a ActorId,
    pub run: &'a RunId,
    calls: Vec<&'a ToolCall>,
}

impl<'a> Group<'a> {
    pub fn from_items(items: &'a [Item]) -> Option<Self> {
        let Item::Tool { actor, run, .. } = items.first()? else {
            return None;
        };
        let mut group = Self {
            len: 0,
            actor,
            run,
            calls: Vec::new(),
        };
        for item in items {
            match item {
                Item::Tool {
                    actor: next_actor,
                    run: next_run,
                    call,
                } if next_actor == actor && next_run == run => group.calls.push(call),
                Item::Message(message) if message.text.trim().is_empty() => {}
                _ => break,
            }
            group.len += 1;
        }
        Some(group)
    }

    pub fn focused(&self, session: &Session) -> bool {
        self.calls
            .iter()
            .any(|call| session.tool_focused(self.actor, self.run, &call.id))
    }

    fn count(&self, state: ToolState) -> usize {
        self.calls.iter().filter(|call| call.state == state).count()
    }

    pub(super) fn current(&self) -> &ToolCall {
        // Completion order cannot make unfinished parallel work disappear.
        self.calls
            .iter()
            .rev()
            .find(|call| call.state == ToolState::Running)
            .or_else(|| {
                self.calls
                    .iter()
                    .rev()
                    .find(|call| call.state == ToolState::Pending)
            })
            .or_else(|| self.calls.last())
            .copied()
            .unwrap_or_else(|| unreachable!("A group has at least one call"))
    }

    pub fn render(
        &self,
        cols: usize,
        options: &UiOptions,
        renderers: &Renderers,
        elapsed: Duration,
        focused: bool,
    ) -> Line {
        let running = self.count(ToolState::Running);
        let pending = self.count(ToolState::Pending);
        let failed = self.count(ToolState::Failed);
        let denied = self.count(ToolState::Denied);
        let cancelled = self.count(ToolState::Cancelled);
        let (symbol, tone) = if running > 0 {
            (spinner(options, elapsed), Tone::Accent)
        } else if pending > 0 {
            ("◇", Tone::Muted)
        } else if failed > 0 {
            ("×", Tone::Error)
        } else if denied > 0 {
            ("⊘", Tone::Warning)
        } else if cancelled > 0 {
            ("−", Tone::Muted)
        } else {
            ("✓", Tone::Success)
        };
        let mut status = String::new();
        for (count, label) in [
            (failed, "failed"),
            (denied, "refused"),
            (cancelled, "cancelled"),
        ] {
            if count > 0 {
                status.push_str(&format!(" · {count} {label}"));
            }
        }
        if running + pending > 1 {
            status.push_str(&format!(" · {} ongoing", running + pending));
        }
        let hint = if cols >= 48 { " · F2 details" } else { "" };
        // Reserve the outcome before clipping the public label on small screens.
        let status = fit(&status, cols.saturating_sub(6));
        let label = fit(
            &self.label(renderers),
            cols.saturating_sub(6 + status.width() + hint.width()),
        );
        vec![
            span(if focused { " › " } else { "   " }, Tone::Accent),
            span(format!("{symbol} "), tone),
            span(label, Tone::Muted),
            span(
                status,
                if failed > 0 {
                    Tone::Error
                } else {
                    Tone::Warning
                },
            ),
            span(hint, Tone::Muted),
        ]
    }

    /// What the row says of its current call: its intention, what its
    /// renderer reads from its arguments, or its tool's name.
    fn label(&self, renderers: &Renderers) -> String {
        let said = |text: &String| !text.trim().is_empty();
        let current = self.current();
        current
            .intent
            .clone()
            .filter(said)
            .or_else(|| renderers.activity(current).filter(said))
            .unwrap_or_else(|| current.name.clone())
    }

    pub fn target(&self, conversation: &ConversationId, row: usize, line: &Line) -> ToolTarget {
        ToolTarget {
            conversation: conversation.clone(),
            row,
            start: 3,
            end: line.iter().map(|span| span.text.width()).sum(),
            actor: self.actor.clone(),
            run: self.run.clone(),
            call: self.current().id.clone(),
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "a broken fixture must fail the test")]
mod tests {
    use roomkit_tui_protocol::{ActorId, Item, RunId, ToolCall, ToolState};
    use serde_json::json;

    use super::*;
    use crate::render::ToolRenderer;

    struct Reading;

    impl ToolRenderer for Reading {
        fn render(&self, _call: &ToolCall) -> Vec<String> {
            Vec::new()
        }

        fn activity(&self, call: &ToolCall) -> Option<String> {
            call.arguments["path"]
                .as_str()
                .map(|path| format!("Read {path}"))
        }
    }

    struct Blank;

    impl ToolRenderer for Blank {
        fn render(&self, _call: &ToolCall) -> Vec<String> {
            Vec::new()
        }

        fn activity(&self, _call: &ToolCall) -> Option<String> {
            Some("   ".to_owned())
        }
    }

    fn row(call: ToolCall, renderers: &Renderers) -> String {
        let items = [Item::Tool {
            actor: ActorId::from("agent"),
            run: RunId::from("run"),
            call,
        }];
        let group = Group::from_items(&items).unwrap();
        let line = group.render(100, &UiOptions::default(), renderers, Duration::ZERO, false);
        line.iter().map(|part| part.text.as_str()).collect()
    }

    #[test]
    fn a_call_reads_as_its_intention_then_its_renderer_then_its_name() {
        let mut renderers = Renderers::default();
        renderers.register("read", Reading);
        let read = || {
            ToolCall::new(
                "c1",
                "read",
                json!({ "path": "src/main.rs" }),
                ToolState::Running,
            )
        };

        assert!(row(read(), &renderers).contains("Read src/main.rs"));
        let intended = read().with_intent("Je lis le point d'entrée");
        assert!(row(intended, &renderers).contains("Je lis le point d'entrée"));
        let plain = ToolCall::new(
            "c2",
            "shell",
            json!({ "command": "ls" }),
            ToolState::Running,
        );
        assert!(row(plain, &renderers).contains("shell"));
        let blank = ToolCall::new("c3", "read", json!({ "path": " " }), ToolState::Running);
        renderers.register("read", Blank);
        assert!(row(blank, &renderers).contains("read"));
    }
}
