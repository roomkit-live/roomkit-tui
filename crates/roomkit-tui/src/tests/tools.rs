#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]

use roomkit_tui_protocol::*;
use serde_json::json;
use unicode_width::UnicodeWidthStr;

use super::*;
use crate::render::{self, Renderers, ToolRenderer};

#[test]
fn tool_views_are_bounded_sanitized_and_fit_narrow_and_wide_terminals() {
    let mut session = session();
    session.apply(tool("run-1", ToolState::Succeeded));
    session.insert("Unicode 世界\n👩‍💻 e\u{301}");
    session.notice = "\u{1b}]52;c;clipboard attack\u{7}".into();
    for (width, height) in [(1, 1), (12, 8), (32, 12), (80, 24), (140, 40)] {
        let frame = render::screen(&session, width, height, &Renderers::default());
        assert!(frame.lines.len() <= usize::from(height));
        for line in &frame.lines {
            assert!(
                line.width() <= usize::from(width),
                "line exceeded {width}: {line:?}"
            );
            assert!(!line.chars().any(char::is_control));
        }
        if let Some((col, row)) = frame.cursor {
            assert!(col < width && row < height);
        }
    }
    let frame = render::screen(&session, 100, 24, &Renderers::default());
    assert!(
        frame
            .lines
            .iter()
            .any(|line| line.contains("unknown-domain-tool"))
    );
    assert!(
        frame
            .lines
            .iter()
            .any(|line| line.contains("Three records"))
    );
}

#[derive(Debug)]
struct DomainRenderer;

impl ToolRenderer for DomainRenderer {
    fn render(&self, call: &ToolCall) -> Vec<String> {
        let rows = call
            .data
            .as_ref()
            .and_then(|data| data.get("rows"))
            .and_then(|value| value.as_array())
            .map_or(0, Vec::len);
        vec![format!("Found {rows} domain records")]
    }
}

#[test]
fn an_application_can_render_structured_results_without_parsing_model_text() {
    let mut session = session();
    session.apply(tool("run-1", ToolState::Succeeded));
    let mut renderers = Renderers::default();
    renderers.register("unknown-domain-tool", DomainRenderer);
    let frame = render::screen(&session, 80, 24, &renderers);
    assert!(frame.tool_targets.is_empty());
    assert!(!frame.lines.iter().any(|line| line.contains("F2 expand")));
    assert!(
        frame
            .lines
            .iter()
            .any(|line| line.contains("Found 3 domain records"))
    );
    session.toggle_tool();
    let expanded = render::screen(&session, 80, 24, &renderers);
    assert!(
        !expanded
            .lines
            .iter()
            .any(|line| line.contains("F2 collapse"))
    );
    assert!(
        expanded
            .lines
            .iter()
            .any(|line| line.contains("Found 3 domain records"))
    );
    assert!(
        !expanded
            .lines
            .iter()
            .any(|line| line.contains("args:") || line.contains("query"))
    );
}

#[test]
fn disclosure_depends_on_hidden_details_and_current_preview_width() {
    struct SameView;
    impl ToolRenderer for SameView {
        fn render(&self, _: &ToolCall) -> Vec<String> {
            vec!["A long readable result ".repeat(7)]
        }
    }
    let mut session = session();
    let mut event = tool("run-1", ToolState::Succeeded);
    if let Event::Item {
        item: Item::Tool { call, .. },
        ..
    } = &mut event
    {
        call.text.clear();
        call.data = None;
    }
    session.apply(event);
    let mut renderers = Renderers::default();
    // Generic arguments alone are already visible and need no disclosure.
    assert!(
        render::screen(&session, 100, 30, &renderers)
            .tool_targets
            .is_empty()
    );
    renderers.register("unknown-domain-tool", SameView);
    assert!(
        render::screen(&session, 100, 30, &renderers)
            .tool_targets
            .is_empty()
    );
    let narrow = render::screen(&session, 32, 30, &renderers);
    assert_eq!(narrow.tool_targets.len(), 1);
    assert!(narrow.lines.iter().any(|line| line.contains("F2 expand")));
    session.toggle_tool();
    assert!(
        render::screen(&session, 32, 30, &renderers)
            .lines
            .iter()
            .any(|line| line.contains("F2 collapse"))
    );
    // Growing the viewport removes an obsolete disclosure even if previously expanded.
    let wide = render::screen(&session, 100, 30, &renderers);
    assert!(wide.tool_targets.is_empty());
    assert!(!wide.lines.iter().any(|line| line.contains("F2 collapse")));
}

#[test]
fn tool_details_are_independent_and_scoped_to_actor_run_and_call() {
    let mut session = session();
    session.apply(tool("first", ToolState::Succeeded));
    session.apply(tool("second", ToolState::Failed));
    let actor = ActorId::from("agent");
    let call = CallId::from("call-1");
    session.toggle_tool();
    assert!(session.tool_expanded(&actor, &"second".into(), &call));
    assert!(!session.tool_expanded(&actor, &"first".into(), &call));
    session.focus_tool(true);
    session.toggle_tool();
    assert!(session.tool_expanded(&actor, &"first".into(), &call));
    session.toggle_tool();
    assert!(!session.tool_expanded(&actor, &"first".into(), &call));
    assert!(session.tool_expanded(&actor, &"second".into(), &call));
    let frame = render::screen(&session, 100, 30, &Renderers::default());
    assert!(frame.lines.iter().any(|line| line.contains("failed")));
    assert!(
        frame
            .spans
            .iter()
            .flatten()
            .any(|span| span.tone == render::Tone::Error)
    );
    assert!(
        frame
            .spans
            .iter()
            .flatten()
            .any(|span| span.tone == render::Tone::Success)
    );
    session.expanded = true;
    session.toggle_tool();
    assert!(!session.tool_expanded(&actor, &"first".into(), &call));
    assert!(session.tool_expanded(&actor, &"second".into(), &call));
}

#[test]
fn mouse_targets_toggle_only_the_visible_call_even_after_scrolling() {
    let mut session = session();
    for index in 0..12 {
        session.apply(tool(&format!("run-{index}"), ToolState::Succeeded));
    }
    session.scroll = 12;
    let frame = render::screen(&session, 80, 24, &Renderers::default());
    assert!(!frame.tool_targets.is_empty());
    for target in &frame.tool_targets {
        assert!(frame.lines[target.row].contains("F2 expand"));
        assert!(target.end <= 80);
    }
    let target = &frame.tool_targets[0];
    session.toggle_tool_details(&target.actor, &target.run, &target.call);
    assert!(session.tool_expanded(&target.actor, &target.run, &target.call));
    assert!(!session.tool_expanded(&target.actor, &"run-11".into(), &target.call));
    session.apply(Event::Snapshot(snapshot()));
    session.toggle_tool_details(&target.actor, &target.run, &target.call);
    assert!(!session.tool_expanded(&target.actor, &target.run, &target.call));
    let native = crate::UiOptions {
        selection: crate::SelectionMode::Native,
        ..Default::default()
    };
    session.apply(tool("native", ToolState::Succeeded));
    assert!(
        render::screen_with_options(
            &session,
            80,
            24,
            &Renderers::default(),
            &native,
            std::time::Duration::ZERO
        )
        .tool_targets
        .is_empty()
    );
}

#[test]
fn unchanged_tool_bodies_are_reused_without_caching_activity_or_click_scope() {
    use crate::UiOptions;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;

    struct Counted(Arc<AtomicUsize>);
    impl ToolRenderer for Counted {
        fn render(&self, call: &ToolCall) -> Vec<String> {
            self.0.fetch_add(1, Ordering::Relaxed);
            vec![format!("{} {:?}", call.text, call.state)]
        }
        fn render_expanded(&self, call: &ToolCall) -> Vec<String> {
            let mut rows = self.render(call);
            rows.push("Additional detail".into());
            rows
        }
        fn has_expanded_details(&self, _: &ToolCall) -> bool {
            true
        }
    }
    let count = Arc::new(AtomicUsize::new(0));
    let mut renderers = Renderers::default();
    renderers.register("unknown-domain-tool", Counted(count.clone()));
    let mut session = session();
    session.apply_at(
        Event::RunStarted {
            conversation: "room-a".into(),
            run: "run-1".into(),
        },
        Duration::ZERO,
    );
    session.apply(tool("run-1", ToolState::Running));
    let first = render::screen_with_options(
        &session,
        80,
        24,
        &renderers,
        &UiOptions::default(),
        Duration::ZERO,
    );
    for tick in 1..=10 {
        session.insert("x");
        let frame = render::screen_with_options(
            &session,
            80,
            24,
            &renderers,
            &UiOptions::default(),
            Duration::from_millis(tick * 100),
        );
        assert_ne!(frame.lines, first.lines);
        assert!(frame.lines.join("\n").contains("Working"));
    }
    assert_eq!(count.load(Ordering::Relaxed), 1);

    session.apply(tool("run-1", ToolState::Failed));
    let frame = render::screen(&session, 80, 24, &renderers);
    assert!(frame.lines.join("\n").contains("Three records Failed"));
    assert_eq!(count.load(Ordering::Relaxed), 2);
    session.toggle_tool_details(&"agent".into(), &"run-1".into(), &"call-1".into());
    let expanded = render::screen(&session, 80, 24, &renderers);
    assert!(expanded.lines.join("\n").contains("Additional detail"));
    let after_expansion = count.load(Ordering::Relaxed);
    assert_eq!(expanded, render::screen(&session, 80, 24, &renderers));
    assert_eq!(count.load(Ordering::Relaxed), after_expansion);

    let mut next = session.snapshot().cloned().unwrap();
    next.conversation = "room-b".into();
    next.active_run = None;
    session.apply(Event::Snapshot(next));
    let frame = render::screen(&session, 80, 24, &renderers);
    assert!(
        frame
            .tool_targets
            .iter()
            .all(|target| target.conversation.as_str() == "room-b")
    );
    assert_eq!(count.load(Ordering::Relaxed), after_expansion);

    // Re-registering permits the application to change renderer configuration.
    renderers.register("unknown-domain-tool", Counted(count.clone()));
    render::screen(&session, 80, 24, &renderers);
    assert!(count.load(Ordering::Relaxed) > after_expansion);
}

#[test]
fn cached_tool_layout_matches_fresh_rendering_after_data_and_size_changes() {
    use crate::UiOptions;
    let mut session = session();
    session.apply(tool("run-1", ToolState::Succeeded));
    let renderers = Renderers::default();
    for (width, preview, expanded, text) in [
        (80, 4, false, "first result"),
        (30, 1, false, "first result"),
        (80, 4, true, "first result"),
        (80, 4, true, "updated result\nwith more content"),
        (80, 4, false, "updated result"),
    ] {
        let snapshot = session.snapshot.as_mut().unwrap();
        let call = match &mut snapshot.items[0] {
            Item::Tool { call, .. } => Some(call),
            _ => None,
        }
        .unwrap();
        call.text = text.into();
        call.data = Some(json!({"text": text}));
        call.arguments = json!({"view": preview});
        if session.tool_expanded(&"agent".into(), &"run-1".into(), &"call-1".into()) != expanded {
            session.toggle_tool_details(&"agent".into(), &"run-1".into(), &"call-1".into());
        }
        let options = UiOptions {
            tool_preview_lines: preview,
            ..UiOptions::default()
        };
        let cached = render::screen_with_options(
            &session,
            width,
            30,
            &renderers,
            &options,
            std::time::Duration::ZERO,
        );
        let fresh = render::screen_with_options(
            &session,
            width,
            30,
            &Renderers::default(),
            &options,
            std::time::Duration::ZERO,
        );
        assert_eq!(cached, fresh);
    }
}
