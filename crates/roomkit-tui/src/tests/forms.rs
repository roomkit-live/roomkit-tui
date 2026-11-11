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

#[test]
fn form_links_remain_clickable_across_wrapping_and_resizing() {
    let mut s = session();
    let url = format!("https://example.com/authorize?state={}", "x".repeat(180));
    s.apply(Event::Form {
        conversation: "room-a".into(),
        form: form().with_description(format!(
            "Open this link:\n\n{url}\n\nPaste the return URL below."
        )),
    });
    for (width, height) in [(80, 24), (40, 20), (120, 40)] {
        let frame = render::screen(&s, width, height, &Renderers::default());
        let links: Vec<_> = frame
            .spans
            .iter()
            .flatten()
            .filter(|span| span.hyperlink.is_some())
            .collect();
        assert!(links.len() > 1);
        assert!(
            links
                .iter()
                .all(|span| span.hyperlink.as_ref().unwrap().url() == url)
        );
        assert_eq!(
            links
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>(),
            url
        );
    }
}

#[test]
fn form_markdown_links_match_conversation_links() {
    let url = format!(
        "https://example.com/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fcallback&scope=openid+profile&state={}",
        "x".repeat(180)
    );
    let source = format!("[Open sign-in page](<{url}>)");
    let mut s = session();
    s.apply(Event::Item {
        conversation: "room-a".into(),
        item: Item::Message(Message::new("link", "agent", &source)),
    });
    let renderers = Renderers::default();
    for width in [40, 80, 120] {
        let conversation = render::screen(&s, width, 50, &renderers);
        let links = |frame: &render::Frame| -> Vec<render::Span> {
            frame
                .spans
                .iter()
                .flatten()
                .filter(|span| span.hyperlink.is_some())
                .cloned()
                .collect()
        };
        let expected = links(&conversation);
        assert!(!expected.is_empty());
        s.apply(Event::Form {
            conversation: "room-a".into(),
            form: form().with_markdown_description(&source),
        });
        let actual = links(&render::screen(&s, width, 50, &renderers));
        assert!(!actual.is_empty(), "Form link label lost its destination");
        let text = |spans: &[render::Span]| {
            spans
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>()
        };
        assert_eq!(text(&actual), text(&expected));
        assert!(
            actual
                .iter()
                .all(|span| span.hyperlink == expected[0].hyperlink
                    && span.emphasis == expected[0].emphasis)
        );
        assert!(text(&actual).starts_with("Open sign-in page"));
        s.form = None;
    }
}

#[test]
fn plain_form_instructions_and_inline_field_links_keep_their_meaning() {
    let mut s = session();
    let mut spec = form().with_description("Keep **literal** text and [brackets].");
    spec.fields[0].help = "Copy a key from https://example.com/console; keep it private".into();
    s.apply(Event::Form {
        conversation: "room-a".into(),
        form: spec,
    });
    let frame = render::screen(&s, 100, 30, &Renderers::default());
    assert!(
        frame
            .lines
            .join("\n")
            .contains("Keep **literal** text and [brackets].")
    );
    assert!(frame.spans.iter().flatten().any(|span| {
        span.text == "https://example.com/console"
            && span
                .hyperlink
                .as_ref()
                .is_some_and(|link| link.url() == span.text)
    }));
}
