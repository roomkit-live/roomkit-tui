#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]

use roomkit_tui_protocol::*;

use super::*;
use crate::Session;
use crate::render::{self, Renderers};

#[test]
fn model_changes_clear_occupancy_and_ignore_late_measurements_from_old_models() {
    let mut session = session();
    started(&mut session, "run-1");
    let usage = Event::ContextUpdated {
        conversation: "room-a".into(),
        run: "run-1".into(),
        provider: "local".into(),
        model: "model-a".into(),
        usage: ContextUsage::new(100).with_capacity(200),
    };
    session.apply(usage.clone());
    assert_eq!(session.context().unwrap().percent(), Some(50.0));
    session.apply(Event::ModelChanged {
        conversation: "room-a".into(),
        model: ModelInfo::new("other", "model-a").with_effort("low"),
    });
    assert!(session.context().is_none());
    session.apply(usage);
    assert!(session.context().is_none());
}

#[test]
fn usage_is_scoped_restored_and_cleared_before_a_new_execution() {
    let usage = {
        let mut usage = TokenUsage::default();
        usage.input_tokens = Some(350);
        usage.output_tokens = Some(20);
        usage.cached_input_tokens = 100;
        usage.complete = true;
        usage
    };
    let context = ContextUsage::new(200).with_capacity(1000);
    let mut restored = snapshot();
    restored.context = Some(context.clone());
    restored.usage = Some(usage.clone());
    let mut session = session();
    session.apply(Event::Snapshot(restored));
    assert_eq!(session.context().cloned(), Some(context.clone()));
    assert_eq!(session.usage().cloned(), Some(usage.clone()));
    started(&mut session, "run-1");
    assert!(session.context().is_none() && session.usage().is_none());
    let draw = |session: &Session| {
        render::screen(session, 230, 30, &Renderers::default())
            .lines
            .join("\n")
    };
    assert!(draw(&session).contains("context (last report) 200/1000 · 80.0% left"));
    let update = Event::UsageUpdated {
        conversation: "room-a".into(),
        run: "run-1".into(),
        provider: "local".into(),
        model: "model-a".into(),
        context: Some(context.clone()),
        usage: usage.clone(),
    };
    session.apply(update.clone());
    session.apply(update.clone());
    assert_eq!(
        session.usage().unwrap().input_tokens,
        Some(350),
        "reports replace, never double count"
    );
    assert_eq!(
        session.snapshot().unwrap().context,
        session.context().cloned()
    );
    assert_eq!(session.snapshot().unwrap().usage, session.usage().cloned());
    let screen = render::screen(&session, 230, 30, &Renderers::default())
        .lines
        .join("\n");
    assert!(screen.contains("context 200/1000 · 80.0% left"));
    assert!(screen.contains("turn 350 in / 20 out"));
    assert!(!screen.contains("last report"));
    started(&mut session, "run-2");
    session.apply(update);
    assert!(session.context().is_none() && session.usage().is_none());
    assert!(session.snapshot().unwrap().context.is_none());
    assert!(draw(&session).contains("context (last report) 200/1000 · 80.0% left"));
    session.apply(Event::Snapshot(snapshot()));
    assert!(session.context().is_none() && session.usage().is_none());
    assert!(!draw(&session).contains("200/1000"));
}

#[test]
fn last_context_report_survives_missing_usage_but_never_crosses_models_or_conversations() {
    let mut initial = snapshot();
    initial.context = Some(ContextUsage::new(120).with_capacity(1000));
    let mut session = session();
    session.apply(Event::Snapshot(initial));
    started(&mut session, "run-1");
    session.apply(Event::UsageUpdated {
        conversation: "room-a".into(),
        run: "run-1".into(),
        provider: "local".into(),
        model: "model-a".into(),
        context: None,
        usage: {
            let mut usage = TokenUsage::default();
            usage.input_tokens = Some(20);
            usage.cached_input_tokens = 0;
            usage
        },
    });
    let draw = |session: &Session| {
        render::screen(session, 230, 30, &Renderers::default())
            .lines
            .join("\n")
    };
    assert!(draw(&session).contains("context (last report) 120/1000 · 88.0% left"));
    assert!(session.context().is_none());
    session.apply(Event::ModelChanged {
        conversation: "room-a".into(),
        model: ModelInfo::new("local", "model-b"),
    });
    assert!(!draw(&session).contains("120/1000"));
    assert!(draw(&session).contains("context ?"));
    let mut other = snapshot();
    other.conversation = "room-b".into();
    session.apply(Event::Snapshot(other));
    assert!(!draw(&session).contains("120/1000"));
}

#[test]
fn context_has_a_visible_footer_row_even_with_a_long_model_name() {
    let mut initial = snapshot();
    initial.model.as_mut().unwrap().model = "long-model-name-".repeat(20);
    initial.context = Some(ContextUsage::new(120).with_capacity(1000));
    let mut session = session();
    session.apply(Event::Snapshot(initial));
    for height in [8, 16, 30] {
        let frame = render::screen(&session, 80, height, &Renderers::default());
        let context_row = frame
            .lines
            .iter()
            .position(|line| line.contains("context 120/1000"))
            .unwrap();
        assert!(frame.lines[context_row].contains("88.0% left · 12.0% used"));
        assert!(
            frame.spans[context_row]
                .iter()
                .any(|span| span.tone == render::Tone::Accent)
        );
        assert!(!frame.lines[context_row].contains("long-model-name"));
        assert!(frame.cursor.unwrap().1 < height);
    }
    started(&mut session, "run-1");
    let frame = render::screen(&session, 80, 24, &Renderers::default());
    assert!(
        frame
            .lines
            .iter()
            .any(|line| line.contains("context (last report) 120/1000 · 88.0% left"))
    );
}
