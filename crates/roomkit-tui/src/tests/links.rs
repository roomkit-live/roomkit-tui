//! A web address in any text an agent or a host sends carries its whole
//! destination on every row it is shown on.

#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]

use roomkit_tui_protocol::*;

use super::*;
use crate::content::plain;
use crate::render::{self, Frame, Line, Renderers, Span};

fn links(span: &Span, url: &str) -> bool {
    span.hyperlink
        .as_ref()
        .is_some_and(|link| link.url() == url)
}

/// The text of the fragments of `rows` that link `url`, in order.
fn linked(rows: &[Line], url: &str) -> String {
    rows.iter()
        .flatten()
        .filter(|span| links(span, url))
        .map(|span| span.text.as_str())
        .collect()
}

/// Asserts that `frame` shows `url` wrapped over several rows, every
/// fragment of it linking the whole address.
fn assert_linked_on_every_row(frame: &Frame, url: &str) {
    let rows = frame
        .spans
        .iter()
        .filter(|row| row.iter().any(|span| links(span, url)))
        .count();
    let screen = frame.lines.join("\n");
    assert_eq!(linked(&frame.spans, url), url, "{screen}");
    assert!(rows > 1, "{url} wraps: {screen}");
}

fn asks(session: &mut Session, text: String, description: String) {
    let option = QuestionOption::new("Postgres", description);
    let item = QuestionItem::new("Database", text, vec![option]);
    session.apply(Event::QuestionAsked {
        conversation: "room-a".into(),
        question: Question::new("q1", "agent", "run-1", vec![item]),
    });
}

fn requests(session: &mut Session, title: String, detail: String) {
    session.apply(Event::PermissionRequested {
        conversation: "room-a".into(),
        permission: Permission::new("p1", "agent", "run-1", title, detail),
    });
}

#[test]
fn an_address_in_unicode_quotes_or_before_unicode_punctuation_is_linked() {
    let url = "https://example.com/guide";
    for text in [
        format!("See “{url}” first."),
        format!("See {url}。"),
        format!("«{url}», then go on"),
        format!("「{url}」、"),
        format!("（{url}）"),
        format!("Read {url}…"),
    ] {
        assert_eq!(linked(&plain(&text), url), url, "{text}");
    }
}

#[test]
fn a_question_links_the_addresses_of_its_text_and_option_descriptions() {
    let text_url = "https://example.com/questions/sessions?store=database&scope=all";
    let option_url = "https://example.com/databases/postgres/replication?mode=streaming";
    let mut session = session();
    started(&mut session, "run-1");
    asks(
        &mut session,
        format!("Which database holds the sessions? See {text_url}"),
        format!("Shared between machines, as {option_url} says"),
    );
    let frame = render::screen(&session, 60, 30, &Renderers::default());
    assert_linked_on_every_row(&frame, text_url);
    assert_linked_on_every_row(&frame, option_url);
}

#[test]
fn an_approval_links_the_addresses_of_its_title_and_detail() {
    let title_url = "https://example.com/records/42/export?format=csv&columns=all";
    let detail_url = "https://example.com/storage/buckets/reports/objects?prefix=2026";
    let mut session = session();
    started(&mut session, "run-1");
    requests(
        &mut session,
        format!("Fetch {title_url}"),
        format!("Then upload it to {detail_url}\nand summarize it."),
    );
    let frame = render::screen(&session, 60, 30, &Renderers::default());
    assert_linked_on_every_row(&frame, title_url);
    assert_linked_on_every_row(&frame, detail_url);

    // The decision's receipt in the history names the request by its title.
    let permission = session.permissions()[0].clone();
    session.apply(Event::PermissionResolved {
        conversation: "room-a".into(),
        permission: "p1".into(),
    });
    session.apply(Event::Item {
        conversation: "room-a".into(),
        item: Item::Approval(PermissionDecision::new(
            permission,
            Decision::Allow,
            "person:alice",
        )),
    });
    let frame = render::screen(&session, 60, 30, &Renderers::default());
    assert_linked_on_every_row(&frame, title_url);
}

/// The detail of an approval is protocol-bounded to 64 KiB, beyond what one
/// line of the linked layout holds: a long line is laid out whole all the same.
#[test]
fn a_long_approval_detail_stays_whole_through_the_linked_layout() {
    let mut session = session();
    started(&mut session, "run-1");
    let detail = format!("{}https://example.com/end END", "word ".repeat(4_000));
    requests(&mut session, "Run a long command".into(), detail);
    session.permission_scroll = usize::MAX;
    let frame = render::screen(&session, 80, 30, &Renderers::default());
    assert!(
        frame.lines.iter().any(|l| l.contains("END")),
        "END is shown"
    );
    assert_eq!(
        linked(&frame.spans, "https://example.com/end"),
        "https://example.com/end"
    );
}

const NOTICE_URL: &str = "https://example.com/settings/privacy/regions?organization=example#global";

fn notice() -> String {
    format!("The host refused it: select Global at {NOTICE_URL} first, then try again.")
}

#[test]
fn a_notice_under_a_card_a_form_or_a_panel_links_its_address_on_every_row() {
    type Opens = fn(&mut Session);
    let opens: [(&str, Opens); 4] = [
        ("question", |s| asks(s, "Which one?".into(), String::new())),
        ("approval", |s| {
            requests(s, "Run tests".into(), "cargo test".into())
        }),
        ("form", |s| {
            let field = FormField::new("code", "Code", FieldKind::Text);
            s.apply(Event::Form {
                conversation: "room-a".into(),
                form: FormSpec::new("f1", "summarize", "Connect", vec![field]),
            });
        }),
        ("panel", |s| {
            s.apply(Event::Panel {
                conversation: "room-a".into(),
                panel: PanelSpec::new("p1", "summarize", "Jobs", "first\nsecond"),
            });
        }),
    ];
    for (screen, open) in opens {
        let mut session = session();
        started(&mut session, "run-1");
        open(&mut session);
        session.notice = notice();
        let frame = render::screen(&session, 60, 30, &Renderers::default());
        assert!(
            frame.lines.iter().any(|l| l.contains("first, then")),
            "{screen}"
        );
        assert_linked_on_every_row(&frame, NOTICE_URL);
    }
}

#[test]
fn a_notice_on_the_welcome_screen_links_its_address_on_every_row() {
    let mut session = session();
    session.notice = notice();
    let options = crate::UiOptions {
        welcome: Some(crate::WelcomeScreen {
            logo: Vec::new(),
            logo_tiles: Vec::new(),
            wordmark: vec!["MARK".into()],
            width: 60,
        }),
        ..crate::UiOptions::default()
    };
    let draw = |session: &Session| {
        let zero = std::time::Duration::ZERO;
        render::screen_with_options(session, 80, 30, &Renderers::default(), &options, zero)
    };
    let frame = draw(&session);
    assert!(
        frame.lines.iter().any(|l| l.contains("MARK")),
        "the welcome screen"
    );
    assert_linked_on_every_row(&frame, NOTICE_URL);
}
