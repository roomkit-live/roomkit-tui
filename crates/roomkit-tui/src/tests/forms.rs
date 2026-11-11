//! Scoped asynchronous forms and readable instructions alongside secret input.
#![allow(clippy::unwrap_used, reason = "Invalid fixtures must fail the test")]
use super::*;
use crate::render::{self, Renderers};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn form() -> FormSpec {
    FormSpec::new(
        "attempt",
        "summarize",
        "Connect",
        vec![FormField::new("code", "Return URL", FieldKind::Secret).required()],
    )
}

#[test]
fn form_completion_replaces_only_the_matching_open_form() {
    let mut s = session();
    s.editor.insert("preserved draft");
    s.apply(Event::Form {
        conversation: "room-a".into(),
        form: form(),
    });
    let panel = PanelSpec::new("result", "summarize", "Complete", "Connected");
    for (room, id, command) in [
        ("room-b", "attempt", "summarize"),
        ("room-a", "old", "summarize"),
        ("room-a", "attempt", "unadvertised"),
    ] {
        let mut wrong = panel.clone();
        wrong.command = command.into();
        s.apply(Event::FormCompleted {
            conversation: room.into(),
            form: id.into(),
            panel: wrong,
        });
        assert!(s.form.is_some() && s.panel.is_none());
    }
    let complete = Event::FormCompleted {
        conversation: "room-a".into(),
        form: "attempt".into(),
        panel,
    };
    s.apply(complete.clone());
    assert!(s.form.is_none());
    assert_eq!(s.panel.as_ref().unwrap().spec.title, "Complete");
    assert_eq!(s.editor.text(), "preserved draft");
    s.panel = None;
    s.apply(complete.clone());
    assert!(
        s.panel.is_none(),
        "A result must not reopen a dismissed view"
    );
    s.apply(Event::Form {
        conversation: "room-a".into(),
        form: form(),
    });
    s.form = None;
    s.apply(complete);
    assert!(
        s.panel.is_none(),
        "A result must not reopen a cancelled form"
    );
}

#[test]
fn form_instructions_wrap_and_scroll_without_hiding_the_private_input() {
    let mut s = session();
    let text = format!(
        "https://example.com/authorize?{}\nLAST INSTRUCTION",
        "x".repeat(600)
    );
    s.apply(Event::Form {
        conversation: "room-a".into(),
        form: form().with_description(text),
    });
    s.form.as_mut().unwrap().insert("SECRET_CALLBACK");
    for (width, height) in [(80, 24), (32, 16), (120, 40), (40, 16)] {
        let frame = render::screen(&s, width, height, &Renderers::default());
        assert!(frame.lines.len() <= height.into());
        assert!(frame.lines.join("\n").contains("Return URL"));
        assert!(!frame.lines.join("\n").contains("SECRET_CALLBACK"));
        let (x, y) = frame.cursor.unwrap();
        assert!(x < width && y < height);
    }
    for _ in 0..10 {
        s.form
            .as_mut()
            .unwrap()
            .key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    }
    let frame = render::screen(&s, 40, 16, &Renderers::default());
    assert!(frame.lines.join("\n").contains("LAST INSTRUCTION"));
    assert!(frame.lines.join("\n").contains("Return URL"));
}

#[test]
fn form_guidance_and_cleanup_arguments_are_bounded() {
    for spec in [
        form().with_description("x".repeat(16_385)),
        form().with_dismiss("x".repeat(1025)),
    ] {
        let mut s = session();
        s.apply(Event::Form {
            conversation: "room-a".into(),
            form: spec,
        });
        assert!(s.form.is_none());
    }
}

#[test]
fn asynchronous_form_completion_is_safe_before_or_after_the_submission_reply() {
    for result_first in [true, false] {
        let mut s = session();
        s.apply(Event::Form {
            conversation: "room-a".into(),
            form: form().with_async_completion(),
        });
        let request = s
            .request(Command::SubmitForm {
                name: "summarize".into(),
                form: "attempt".into(),
                values: Box::default(),
            })
            .unwrap();
        s.sent(&request);
        let reply = Event::Reply {
            request: request.id,
            error: None,
        };
        let complete = Event::FormCompleted {
            conversation: "room-a".into(),
            form: "attempt".into(),
            panel: PanelSpec::new("result", "summarize", "Complete", "Connected"),
        };
        if result_first {
            s.apply(complete);
            s.apply(reply);
        } else {
            s.apply(reply);
            assert!(s.form_pending() && s.form.is_some());
            s.apply(complete);
        }
        assert!(s.form.is_none());
        assert_eq!(s.panel.as_ref().unwrap().spec.title, "Complete");
    }
}
