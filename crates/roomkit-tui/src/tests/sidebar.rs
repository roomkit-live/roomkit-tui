#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]

use roomkit_tui_protocol::*;
use unicode_width::UnicodeWidthStr;

use super::*;
use crate::render::{self, Renderers};

#[test]
fn sidebar_is_scoped_bounded_and_rejects_stale_navigation() {
    let mut s = session();
    let change = |conversation: &str, sidebar| Event::SidebarChanged {
        conversation: conversation.into(),
        sidebar,
    };
    let spec = sidebar_spec();
    let encoded = serde_json::to_vec(&change("room-a", Some(spec.clone()))).unwrap();
    s.apply(serde_json::from_slice(&encoded).unwrap());
    let frame = render::screen(&s, 140, 30, &Renderers::default());
    let target = frame
        .action_targets
        .iter()
        .find(|t| t.sidebar)
        .unwrap()
        .clone();
    assert!(matches!(
        s.action_command(&target),
        Some(Command::Invoke { .. })
    ));
    s.apply(change("other-room", None));
    assert!(s.sidebar.is_some());
    let mut bad = spec.clone();
    bad.sections[0].action = Some(SidebarAction::new("unadvertised", String::new()));
    s.apply(change("room-a", Some(bad)));
    assert_eq!(s.sidebar.as_ref().unwrap().spec, spec);
    let mut bad = spec.clone();
    bad.title = "x".repeat(257);
    s.apply(change("room-a", Some(bad)));
    assert_eq!(s.sidebar.as_ref().unwrap().spec, spec);
    s.apply(change("room-a", None));
    assert!(s.action_command(&target).is_none());
    s.apply(change("room-a", Some(spec)));
    s.sidebar_visible = Some(false);
    s.apply(Event::Snapshot(snapshot()));
    assert!(s.sidebar.is_none());
    assert_eq!(s.sidebar_visible, Some(false));
    assert!(s.action_command(&target).is_none());
}

#[test]
fn sidebar_reflows_live_usage_and_keeps_keyboard_selection_visible() {
    let mut s = session();
    let mut spec = sidebar_spec();
    spec.title = "Session\x1b[2J".into();
    s.apply(Event::SidebarChanged {
        conversation: "room-a".into(),
        sidebar: Some(spec),
    });
    s.snapshot.as_mut().unwrap().context =
        Some(ContextUsage::new(1250).with_capacity(5000).estimated());
    s.snapshot.as_mut().unwrap().mode = Some(ModeState::new(
        "summarize",
        "ask",
        vec![Choice::new("ask", "Ask")],
    ));
    s.editor.insert("Unsent draft");
    let renderers = Renderers::default();
    let frame = render::screen(&s, 140, 30, &renderers);
    assert!(frame.sidebar.is_some());
    let text = frame.lines.join("\n");
    assert!(text.contains("~1250 / 5000 tokens"));
    assert!(
        !text.contains("scripted"),
        "Provider protocol belongs in configuration, not the sidebar"
    );
    assert!(text.contains("25.0% used · 75.0% left"));
    assert!(!text.contains('\x1b'));
    assert!(frame.lines.iter().all(|l| l.width() <= 140));
    assert!(usize::from(frame.cursor.unwrap().0) < frame.sidebar.as_ref().unwrap().start);
    let footer = frame.lines.last().unwrap().split('│').next().unwrap();
    assert!(footer.trim().starts_with("[Ask] · Enter send"));
    assert!(!text.contains("⇧Tab"));
    assert!(
        frame.lines[frame.lines.len() - 2]
            .split('│')
            .next()
            .unwrap()
            .trim()
            .is_empty()
    );
    assert_eq!(text.matches("model-a").count(), 1);
    let narrow = render::screen(&s, 100, 30, &renderers);
    assert!(narrow.sidebar.is_none());
    assert!(narrow.lines.join("\n").contains("context ~1250/5000"));
    assert!(
        narrow
            .lines
            .join("\n")
            .contains("local/model-a · effort n/a")
    );
    s.sidebar_visible = Some(false);
    let hidden = render::screen(&s, 140, 30, &renderers);
    assert!(
        hidden
            .lines
            .join("\n")
            .contains("local/model-a · effort n/a")
    );
    s.sidebar_visible = Some(true);
    // A host that supplies only a context section still needs the model footer.
    let agent_section = s.sidebar.as_mut().unwrap().spec.sections.remove(0);
    let custom = render::screen(&s, 140, 30, &renderers);
    assert!(
        custom
            .lines
            .join("\n")
            .contains("local/model-a · effort n/a")
    );
    s.sidebar
        .as_mut()
        .unwrap()
        .spec
        .sections
        .insert(0, agent_section);
    assert_eq!(s.editor.text(), "Unsent draft");
    let side = s.sidebar.as_mut().unwrap();
    side.selected = 29;
    side.focused = true;
    let frame = render::screen(&s, 140, 18, &renderers);
    assert!(frame.sidebar.as_ref().unwrap().scroll > 0);
    assert!(frame.lines.join("\n").contains("Record 29"));
    assert!(usize::from(frame.cursor.unwrap().0) > frame.sidebar.unwrap().start);
    for width in [0, 11, 80, 111, 112, 140, 220] {
        for height in [0, 7, 17, 18, 30] {
            let frame = render::screen(&s, width, height, &renderers);
            assert!(frame.lines.len() <= usize::from(height));
            assert!(frame.lines.iter().all(|l| l.width() <= usize::from(width)));
        }
    }
}
