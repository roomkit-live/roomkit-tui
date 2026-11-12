#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]

use roomkit_tui_protocol::*;
use unicode_width::UnicodeWidthStr;

use super::*;
use crate::Session;
use crate::render::{self, Renderers};

#[test]
fn application_branding_is_sanitized_and_motion_is_optional_without_delaying_output() {
    use crate::{Branding, UiOptions};
    use std::time::Duration;
    let mut session = session();
    let mut options = UiOptions {
        branding: Branding {
            name: "Observatory".into(),
            subtitle: "Research conversations".into(),
            logo: vec!["[o]".into(), "\x1b]52;host\x07".into()],
            logo_tiles: Vec::new(),
        },
        animations: false,
        ..Default::default()
    };
    let renderers = Renderers::default();
    let screen = |session: &Session, options: &UiOptions, time| {
        render::screen_with_options(
            session,
            100,
            24,
            &renderers,
            options,
            Duration::from_millis(time),
        )
    };
    let frame = screen(&session, &options, 0);
    assert!(frame.lines[0].contains("Observatory"));
    assert!(!frame.lines.iter().any(|line| line.contains("RoomKit")));
    assert!(
        frame
            .lines
            .iter()
            .all(|line| !line.chars().any(char::is_control))
    );
    assert_eq!(frame, screen(&session, &options, 300));
    started(&mut session, "active");
    session.apply(tool("active", ToolState::Running));
    assert_eq!(
        screen(&session, &options, 0),
        screen(&session, &options, 300)
    );
    options.animations = true;
    assert_ne!(
        screen(&session, &options, 0),
        screen(&session, &options, 300)
    );
    session.apply(Event::TextDelta {
        conversation: "room-a".into(),
        actor: "agent".into(),
        run: "active".into(),
        message: "answer".into(),
        text: "Available immediately".into(),
    });
    assert!(
        screen(&session, &options, 0)
            .lines
            .iter()
            .any(|line| line.contains("Available immediately"))
    );
}

#[test]
fn layout_keeps_composer_and_decisions_inside_small_resized_terminals() {
    let mut session = session();
    started(&mut session, "active");
    session.apply(Event::PermissionRequested {
        conversation: "room-a".into(),
        permission: Permission::new(
            "permission",
            "agent",
            "active",
            "Review access",
            "First scope\nSecond scope\nLast scope",
        ),
    });
    session
        .editor
        .set("A long draft 世界 👩‍💻\nsecond\nthird\nfourth\nfifth\nsixth".into());
    let renderers = Renderers::default();
    for state in 0..3 {
        for width in [0, 1, 12, 31, 32, 39, 60, 80, 120] {
            for height in 0..=30 {
                let frame = render::screen(&session, width, height, &renderers);
                assert!(frame.lines.len() <= usize::from(height));
                for (line, spans) in frame.lines.iter().zip(&frame.spans) {
                    assert!(
                        line.width() <= usize::from(width),
                        "{width}x{height}: {line:?}"
                    );
                    assert_eq!(
                        *line,
                        spans
                            .iter()
                            .map(|span| span.text.as_str())
                            .collect::<String>()
                    );
                    assert!(!line.chars().any(char::is_control));
                }
                if let Some((x, y)) = frame.cursor {
                    assert!(x < width && y < height);
                    assert!(frame.spans[usize::from(y)].iter().all(|span| matches!(
                        span.tone,
                        render::Tone::Input | render::Tone::InputPrompt | render::Tone::Placeholder
                    )));
                    if state == 0 {
                        assert!(frame.lines.iter().any(|line| line.contains("Ctrl+Y")));
                        assert!(frame.lines.iter().any(|line| line.contains("First scope")));
                    }
                }
                for target in &frame.tool_targets {
                    assert!(target.row < usize::from(height));
                    assert!(target.start < target.end && target.end <= usize::from(width));
                    assert!(frame.lines[target.row].contains("F2"));
                }
            }
        }
        session.snapshot.as_mut().unwrap().permissions.clear();
        session.set_draft(if state == 0 { "/" } else { "" });
    }
}

#[test]
fn history_return_control_stays_visible_and_preserves_the_draft() {
    use crate::{SelectionMode, UiOptions};
    let mut session = session();
    let renderers = Renderers::default();
    session.scroll = 8;
    // Scrolling a short transcript must not advertise an unnecessary action.
    assert!(
        render::screen(&session, 80, 24, &renderers)
            .back_to_bottom
            .is_none()
    );
    for index in 0..12 {
        session.apply(tool(&format!("run-{index}"), ToolState::Succeeded));
    }
    session.set_draft("Unsent draft 世界");
    session.notice = "A long notice ".repeat(40);
    for width in [12, 20, 48, 80, 120] {
        let frame = render::screen(&session, width, 24, &renderers);
        let target = frame.back_to_bottom.unwrap();
        assert_eq!(target.conversation.as_str(), "room-a");
        assert!(target.row < 24);
        assert!(target.start < target.end && target.end <= usize::from(width));
        assert!(frame.lines[target.row].to_lowercase().contains("bottom"));
        assert!(frame.lines[target.row].width() <= usize::from(width));
    }
    let native = UiOptions {
        selection: SelectionMode::Native,
        ..Default::default()
    };
    let frame =
        render::screen_with_options(&session, 80, 24, &renderers, &native, Default::default());
    assert!(frame.back_to_bottom.is_none());
    assert!(
        frame
            .lines
            .iter()
            .any(|line| line.contains("Back to bottom · Ctrl+End"))
    );

    // Tool focus can also move the viewport away from the latest message.
    for _ in 0..5 {
        session.focus_tool(true);
    }
    session.toggle_tool();
    assert!(
        render::screen(&session, 80, 24, &renderers)
            .back_to_bottom
            .is_some()
    );
    session.scroll_to_bottom();
    let frame = render::screen(&session, 80, 24, &renderers);
    assert!(frame.back_to_bottom.is_none());
    assert!(
        !frame
            .lines
            .iter()
            .any(|line| line.contains("Back to bottom"))
    );
    assert_eq!(session.draft(), "Unsent draft 世界");
    assert!(session.tool_expanded(&"agent".into(), &"run-6".into(), &"call-1".into()));
}

/// A notice that fits stays beside the status; a longer one, or one on two
/// lines (a provider's refusal, then its words), follows the activity row
/// wrapped, within a few rows, instead of being cut at the screen's edge.
#[test]
fn a_long_notice_follows_the_activity_row_wrapped() {
    let renderers = Renderers::default();
    let mut session = session();
    session.notice = "Saved".into();
    let frame = render::screen(&session, 100, 24, &renderers);
    assert!(frame.lines.iter().any(|line| line.ends_with("· Saved")));

    session.notice = "Provider rejected the request format or parameters. Check model and \
        tool-calling support.\nProvider says: This OpenCode Go model is hosted outside Europe \
        and the United States. Select Global at \
        https://opencode.ai/console/org_01ABC/settings/privacy#regions to use it."
        .into();
    for width in [100, 80] {
        let frame = render::screen(&session, width, 24, &renderers);
        let text = frame.lines.join("\n");
        for shown in [
            "tool-calling support.",
            "Provider says: This OpenCode Go model",
            "https://opencode.ai/console/org_01ABC/settings/privacy#regions",
            "to use it.",
        ] {
            assert!(text.contains(shown), "{shown} at {width}:\n{text}");
        }
        assert!(
            frame
                .lines
                .iter()
                .all(|line| line.width() <= usize::from(width))
        );
    }

    session.notice = "word ".repeat(200);
    let frame = render::screen(&session, 80, 24, &renderers);
    let rows: Vec<_> = frame
        .lines
        .iter()
        .filter(|line| line.contains("word"))
        .collect();
    assert_eq!(rows.len(), 5, "{rows:#?}");
    assert!(rows[4].trim_end().ends_with('…'), "{rows:#?}");

    // On a short terminal the notice gives way to the composer.
    for height in 8..=12 {
        let frame = render::screen(&session, 80, height, &renderers);
        let text = frame.lines.join("\n");
        assert!(frame.lines.len() <= usize::from(height), "{text}");
        let cursor = frame.cursor.map(|(_, row)| row);
        assert!(
            cursor.is_some_and(|row| row < height),
            "{height}: {cursor:?}\n{text}"
        );
        assert!(text.contains("word"), "{text}");
    }
}

#[test]
fn markdown_view_preserves_source_and_can_be_disabled_by_the_host() {
    let mut session = session();
    let source = "\n\n\n**Hello** `world`\n\n\n";
    session.apply(Event::Item {
        conversation: "room-a".into(),
        item: Item::Message(Message::new("message", "agent", source)),
    });
    let renderers = Renderers::default();
    let view = render::screen(&session, 80, 30, &renderers);
    let author = view
        .lines
        .iter()
        .position(|line| line.contains("● Researcher"))
        .unwrap();
    assert_eq!(view.lines[author + 1], "   Hello world");
    assert!(view.spans[author + 1].iter().any(|span| span.emphasis.bold));
    assert!(
        matches!(&session.snapshot().unwrap().items[0], Item::Message(message) if message.text == source)
    );
    let options = crate::UiOptions {
        markdown: false,
        ..Default::default()
    };
    let plain = render::screen_with_options(
        &session,
        80,
        30,
        &renderers,
        &options,
        std::time::Duration::ZERO,
    );
    assert!(
        plain
            .lines
            .iter()
            .any(|line| line.contains("**Hello** `world`"))
    );
}

#[test]
fn wide_footer_combines_model_and_context_and_resize_reclaims_the_extra_row() {
    let mut initial = snapshot();
    initial.context = Some(ContextUsage::new(120).with_capacity(1000));
    let mut view = session();
    view.apply(Event::Snapshot(initial));
    view.insert("preserved draft");
    for height in [8, 16, 30] {
        let wide = render::screen(&view, 200, height, &Renderers::default());
        let narrow = render::screen(&view, 80, height, &Renderers::default());
        let wide_model = wide
            .lines
            .iter()
            .position(|line| line.contains("local/model-a"))
            .unwrap();
        assert!(wide.lines[wide_model].contains("context 120/1000 · 88.0% left · 12.0% used"));
        assert!(wide.lines[wide_model + 1].contains("Enter send"));
        let narrow_model = narrow
            .lines
            .iter()
            .position(|line| line.contains("local/model-a"))
            .unwrap();
        assert!(!narrow.lines[narrow_model].contains("context"));
        assert!(narrow.lines[narrow_model + 1].contains("context 120/1000"));
        assert_eq!(wide.lines.len(), usize::from(height));
        assert_eq!(narrow.lines.len(), usize::from(height));
        assert_eq!(wide.cursor.unwrap().1, narrow.cursor.unwrap().1 + 1);
        assert_eq!(view.draft(), "preserved draft");
    }
    started(&mut view, "working");
    let wide = render::screen(&view, 200, 24, &Renderers::default());
    assert!(
        wide.lines.iter().any(|line| line.contains("local/model-a")
            && line.contains("context (last report) 120/1000"))
    );
    let view = session();
    let wide = render::screen(&view, 120, 24, &Renderers::default());
    assert!(
        wide.lines
            .iter()
            .any(|line| line.contains("local/model-a") && line.contains("context ?"))
    );
}
