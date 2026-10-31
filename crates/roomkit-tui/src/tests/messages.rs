#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]
use super::*;
use crate::UiOptions;
use crate::render::Tone;
use crate::render::{self, Renderers};
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

#[test]
fn own_message_surface_uses_view_identity_and_keeps_other_authors_named() {
    let mut snapshot = snapshot();
    snapshot.local_actor = Some("viewer".into());
    for (id, name, text) in [
        (
            "viewer",
            "Local author",
            "Review **文档** please and include examples\n\nSecond paragraph",
        ),
        ("colleague", "Colleague", "Another human's comment"),
        ("agent", "Researcher", "Assistant reply"),
    ] {
        snapshot.actors.push(Actor::new(id, name));
        snapshot
            .items
            .push(Item::Message(Message::new(id, id, text)));
    }
    let mut session = Session::default();
    session.apply(Event::Snapshot(snapshot.clone()));
    let mut options = UiOptions {
        highlight_own_messages: true,
        ..Default::default()
    };
    let draw = |session: &Session, options: &UiOptions| {
        render::screen_with_options(
            session,
            34,
            40,
            &Renderers::default(),
            options,
            Duration::ZERO,
        )
    };
    // Changing the selected agent must not mistake other participants for us.
    for selected in ["agent", "another-agent"] {
        session.snapshot.as_mut().unwrap().selected_agent = selected.into();
        let frame = draw(&session, &options);
        let text = frame.lines.join("\n");
        assert!(!text.contains("Local author"));
        assert!(text.contains("● Colleague") && text.contains("● Researcher"));
        let first = frame
            .lines
            .iter()
            .position(|l| l.contains("Review"))
            .unwrap();
        let last = frame
            .lines
            .iter()
            .position(|l| l.contains("Second paragraph"))
            .unwrap();
        assert!(frame.lines[first].starts_with("  › Review"));
        assert!(frame.lines[first - 1].trim().is_empty());
        assert!(frame.lines[last + 1].trim().is_empty());
        assert!(
            frame.lines[first + 1..=last]
                .iter()
                .all(|line| !line.contains('›'))
        );
        for row in first - 1..=last + 1 {
            assert_eq!(frame.lines[row].width(), 34);
            assert!(
                frame.spans[row]
                    .iter()
                    .all(|s| matches!(s.tone, Tone::Input | Tone::InputPrompt))
            );
        }
        assert!(
            frame.spans[first..=last]
                .iter()
                .flatten()
                .any(|s| s.text.contains("文档") && s.emphasis.bold)
        );
        assert_eq!(session.snapshot().unwrap().items, snapshot.items);
    }
    options.highlight_own_messages = false;
    assert!(
        draw(&session, &options)
            .lines
            .join("\n")
            .contains("● Local author")
    );
    options.highlight_own_messages = true;
    session.snapshot.as_mut().unwrap().local_actor = None;
    assert!(
        draw(&session, &options)
            .lines
            .join("\n")
            .contains("● Local author")
    );
}

#[test]
fn older_snapshots_do_not_guess_the_local_author() {
    let mut encoded = serde_json::to_value(snapshot()).unwrap();
    assert!(encoded.get("local_actor").is_none());
    assert_eq!(
        serde_json::from_value::<Snapshot>(encoded.clone())
            .unwrap()
            .local_actor,
        None
    );
    encoded["local_actor"] = json!("viewer");
    let snapshot: Snapshot = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(snapshot.local_actor, Some("viewer".into()));
    assert_eq!(serde_json::to_value(snapshot).unwrap(), encoded);
}
