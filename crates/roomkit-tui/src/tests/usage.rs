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
    assert!(screen.contains("turn 350 in (100 cached) / 20 out"));
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

/// RMK-1333: a measure of the turn shows as current, and a stop keeps it.
#[test]
fn a_stopped_run_keeps_what_its_generations_told() {
    let mut session = session();
    session.apply(Event::Snapshot(snapshot()));
    started(&mut session, "run-1");
    session.apply(Event::ContextUpdated {
        conversation: "room-a".into(),
        run: "run-1".into(),
        provider: "local".into(),
        model: "model-a".into(),
        usage: ContextUsage::new(200).with_capacity(1000),
    });
    let draw = |session: &Session| {
        render::screen(session, 230, 30, &Renderers::default())
            .lines
            .join("\n")
    };
    assert!(draw(&session).contains("context 200/1000"));
    session.apply(Event::RunFinished {
        conversation: "room-a".into(),
        run: "run-1".into(),
        outcome: RunOutcome::Interrupted,
    });
    let screen = draw(&session);
    assert!(screen.contains("context 200/1000"), "{screen}");
    assert!(!screen.contains("last report"), "{screen}");
}

/// RMK-1461: the turn's cache reads and writes, and the conversation's cost,
/// which a new execution keeps and another conversation never sets.
#[test]
fn the_cache_and_the_conversation_s_cost_show_and_outlive_a_new_execution() {
    let mut session = session();
    session.apply(Event::SidebarChanged {
        conversation: "room-a".into(),
        sidebar: Some(sidebar_spec()),
    });
    started(&mut session, "run-1");
    let mut usage = TokenUsage::complete(15_080, 85, 14_785);
    usage.cache_write_input_tokens = 300;
    session.apply(Event::UsageUpdated {
        conversation: "room-a".into(),
        run: "run-1".into(),
        provider: "local".into(),
        model: "model-a".into(),
        context: Some(ContextUsage::new(15_165).with_capacity(128_000)),
        usage,
    });
    let cost = |conversation: &str, micros, complete| Event::CostUpdated {
        conversation: conversation.into(),
        cost: ConversationCost::new(micros, "USD", complete),
    };
    session.apply(cost("room-a", 420_000, true));
    session.apply(cost("room-b", 1, true));
    // Wide with the sidebar hidden, the footer shows the usage; at 140
    // columns, the sidebar does.
    let draw = |session: &mut Session, width| {
        session.sidebar_visible = Some(width != 230);
        render::screen(session, width, 30, &Renderers::default())
            .lines
            .join("\n")
    };
    assert!(
        draw(&mut session, 230)
            .contains("turn 15080 in (14785 cached, 300 written) / 85 out · ≈ $0.42")
    );
    let sidebar = draw(&mut session, 140);
    assert!(sidebar.contains("Turn: 15080 in / 85 out"));
    assert!(sidebar.contains("Cache: 14785 read, 300 written"));
    assert!(sidebar.contains("Conversation: ≈ $0.42"));
    started(&mut session, "run-2");
    assert!(session.usage().is_none());
    assert_eq!(session.cost().map(|cost| cost.micros), Some(420_000));
    // Below a cent, four decimals; a turn with no price makes it partial.
    session.apply(cost("room-a", 2_870, false));
    assert!(draw(&mut session, 230).contains("· ≈ $0.0029 (partial)"));
    assert!(draw(&mut session, 140).contains("Some turns have no price"));
    session.apply(Event::Snapshot(snapshot()));
    assert!(session.cost().is_none());
    assert!(!draw(&mut session, 230).contains('≈'));
}

fn related(conversation: &str, cost: Option<ConversationCost>) -> Event {
    Event::RelatedCostUpdated {
        conversation: conversation.into(),
        cost,
    }
}

/// The conversation's own cost and its total with the tasks its agents handed
/// over are two figures; there is no total before a task cost anything.
#[test]
fn the_total_adds_what_the_related_work_cost_to_the_conversation() {
    let mut session = session();
    session.apply(Event::SidebarChanged {
        conversation: "room-a".into(),
        sidebar: Some(sidebar_spec()),
    });
    session.apply(Event::CostUpdated {
        conversation: "room-a".into(),
        cost: ConversationCost::new(420_000, "USD", true),
    });
    let draw = |session: &mut Session, width| {
        session.sidebar_visible = Some(width != 230);
        render::screen(session, width, 30, &Renderers::default())
            .lines
            .join("\n")
    };
    assert!(!draw(&mut session, 230).contains("total"));
    assert!(!draw(&mut session, 140).contains("Total:"));

    let usd = |micros, complete| Some(ConversationCost::new(micros, "USD", complete));
    session.apply(related("room-a", usd(880_000, true)));
    assert!(draw(&mut session, 230).contains("≈ $0.42 · total ≈ $1.30"));
    let sidebar = draw(&mut session, 140);
    assert!(sidebar.contains("Conversation: ≈ $0.42"));
    assert!(sidebar.contains("Total: ≈ $1.30"));
    assert!(!sidebar.contains("Some turns have no price"));

    // A task with no price leaves the total partial, not the conversation.
    session.apply(related("room-a", usd(880_000, false)));
    assert!(draw(&mut session, 230).contains("≈ $0.42 · total ≈ $1.30 (partial)"));
    assert!(draw(&mut session, 140).contains("Some turns have no price"));

    // Another currency is not added: what the total lacks reads partial.
    session.apply(related(
        "room-a",
        Some(ConversationCost::new(880_000, "EUR", true)),
    ));
    assert!(draw(&mut session, 230).contains("total ≈ $0.42 (partial)"));

    // Another conversation's figure is not this one's.
    session.apply(related("room-b", usd(1, true)));
    assert_eq!(session.total_cost().unwrap().currency, "USD");
    assert_eq!(session.total_cost().unwrap().micros, 420_000);

    session.apply(related("room-a", None));
    assert!(session.total_cost().is_none());
    session.apply(related("room-a", usd(880_000, true)));
    session.apply(Event::Snapshot(snapshot()));
    assert!(session.total_cost().is_none());
}

/// A conversation whose own model has no price still shows what its tasks
/// cost, as a total that cannot be whole.
#[test]
fn a_conversation_with_no_price_totals_its_related_work_as_partial() {
    let mut session = session();
    session.apply(related(
        "room-a",
        Some(ConversationCost::new(880_000, "USD", true)),
    ));
    let total = session.total_cost().unwrap();
    assert_eq!((total.micros, total.complete), (880_000, false));
}

/// A refused request names itself before the numbers, so clipping a narrow
/// footer cannot hide it, and a model change clears it.
#[test]
fn a_refused_request_leads_the_footer_context_until_the_model_changes() {
    let mut initial = snapshot();
    initial.context = Some(ContextUsage::new(120).with_capacity(1000));
    let mut session = session();
    session.apply(Event::Snapshot(initial));
    started(&mut session, "run-1");
    session.apply(Event::ContextExceeded {
        conversation: "room-a".into(),
        run: "run-1".into(),
    });
    let draw = |session: &Session| {
        render::screen(session, 230, 30, &Renderers::default())
            .lines
            .join("\n")
    };
    assert!(draw(&session).contains("context refused (last report) 120/1000"));
    session.apply(Event::ModelChanged {
        conversation: "room-a".into(),
        model: ModelInfo::new("local", "model-b"),
    });
    let screen = draw(&session);
    assert!(!screen.contains("refused") && screen.contains("context ?"));
}

/// The refused run's earlier step is the footer's last report too.
#[test]
fn a_refusal_after_an_earlier_step_reads_as_a_last_report_in_the_footer() {
    let mut session = session();
    started(&mut session, "run-1");
    session.apply(Event::ContextUpdated {
        conversation: "room-a".into(),
        run: "run-1".into(),
        provider: "local".into(),
        model: "model-a".into(),
        usage: ContextUsage::new(500).with_capacity(1000),
    });
    session.apply(Event::ContextExceeded {
        conversation: "room-a".into(),
        run: "run-1".into(),
    });
    let screen = render::screen(&session, 230, 30, &Renderers::default())
        .lines
        .join("\n");
    assert!(
        screen.contains("context refused (last report) 500/1000"),
        "{screen}"
    );
}
