#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]

use std::time::Duration;

use roomkit_tui_protocol::*;

use super::*;
use crate::render::{self, Renderers};
use crate::{APPROVAL_ARMING_DELAY, Focus};

fn asked(conversation: &str, id: &str, run: &str) -> Event {
    let items = vec![
        QuestionItem::new(
            "Database",
            "Which database holds the sessions?",
            vec![
                QuestionOption::new("SQLite", "One file, nothing to install"),
                QuestionOption::new("Postgres", "Shared between machines"),
            ],
        ),
        QuestionItem::new(
            "Checks",
            "Which checks run?",
            vec![QuestionOption::new("Tests", "")],
        )
        .with_multi_select(true),
    ];
    Event::QuestionAsked {
        conversation: conversation.into(),
        question: Question::new(id, "agent", run, items),
    }
}

fn resolved(id: &str) -> Event {
    Event::QuestionResolved {
        conversation: "room-a".into(),
        question: id.into(),
    }
}

#[test]
fn questions_of_a_followed_run_are_shown_in_order_until_resolved() {
    let mut session = session();
    started(&mut session, "run-1");
    session.apply(asked("room-b", "elsewhere", "run-1"));
    session.apply(asked("room-a", "not-live", "run-9"));
    assert_eq!(session.focus(), Focus::Composer, "neither is this view's");
    session.apply(asked("room-a", "q1", "run-1"));
    session.apply(asked("room-a", "q2", "run-1"));
    session.apply(asked("room-a", "q1", "run-1"));
    assert_eq!(session.focus(), Focus::Question);
    assert_eq!(session.question().unwrap().id().as_str(), "q1");
    let observed = session.observation().unwrap();
    assert_eq!(
        observed.questions,
        [QuestionId::new("q1"), QuestionId::new("q2")]
    );
    session.apply(resolved("q1"));
    assert_eq!(session.question().unwrap().id().as_str(), "q2");
    session.apply(resolved("q2"));
    assert_eq!(session.focus(), Focus::Composer);
}

#[test]
fn a_decision_comes_before_questions_and_a_snapshot_drops_them() {
    let mut session = session();
    started(&mut session, "run-1");
    session.apply(asked("room-a", "q1", "run-1"));
    session.apply(Event::PermissionRequested {
        conversation: "room-a".into(),
        permission: Permission::new("p1", "agent", "run-1", "Run tests", "cargo test"),
    });
    assert_eq!(session.focus(), Focus::Permission);
    session.apply(Event::PermissionResolved {
        conversation: "room-a".into(),
        permission: "p1".into(),
    });
    assert_eq!(session.focus(), Focus::Question);
    // The host tells the questions still asked again after a snapshot.
    session.apply(Event::Snapshot(snapshot()));
    assert!(session.question().is_none());
}

#[test]
fn a_refused_answer_lets_the_card_answer_again() {
    let mut session = session();
    started(&mut session, "run-1");
    session.apply(asked("room-a", "q1", "run-1"));
    let command = Command::AnswerQuestion {
        question: "q1".into(),
        answers: None,
    };
    let request = session.request(command).unwrap();
    session.sent(&request);
    session.question_mut().unwrap().sent = true;
    session.apply(Event::Reply {
        request: request.id,
        error: Some("Temporary failure".into()),
    });
    assert!(!session.question().unwrap().sent);
    assert!(!session.notice.is_empty());
}

#[test]
fn the_card_is_drawn_whole_and_takes_answers_once_it_could_be_read() {
    let mut session = session();
    started(&mut session, "run-1");
    session.apply(asked("room-a", "q1", "run-1"));
    let renderers = Renderers::default();
    for (width, height) in [(80, 24), (40, 20), (120, 40)] {
        let frame = render::screen(&session, width, height, &renderers);
        let text = frame.lines.join("\n");
        assert!(frame.lines.len() <= usize::from(height));
        assert!(
            text.contains("Which database holds the sessions?"),
            "{text}"
        );
        assert!(
            text.contains("1. SQLite") && text.contains("2. Postgres"),
            "{text}"
        );
        assert!(text.contains("3. Other…"), "{text}");
        assert!(
            text.contains("Read the question before answering"),
            "{text}"
        );
        assert_eq!(frame.question.as_ref().map(QuestionId::as_str), Some("q1"));
    }
    let small = render::screen(&session, 40, 14, &renderers);
    assert!(small.lines.join(" ").contains("Enlarge terminal"));
    assert!(small.question.is_none(), "a card not shown is never armed");

    let frame = render::screen(&session, 80, 24, &renderers);
    let shown = Duration::from_secs(10);
    session.sync(&frame, shown);
    assert!(!session.question_armed(shown));
    assert!(session.question_armed(shown + APPROVAL_ARMING_DELAY));
    let armed = render::screen_with_options(
        &session,
        120,
        40,
        &renderers,
        &crate::UiOptions::default(),
        shown + APPROVAL_ARMING_DELAY,
    );
    let text = armed.lines.join("\n");
    assert!(
        text.contains("Enter confirm · Tab next · Esc don't answer"),
        "{text}"
    );
}

#[test]
fn a_card_back_on_screen_is_read_again_before_it_takes_keys() {
    let mut session = session();
    started(&mut session, "run-1");
    session.apply(asked("room-a", "q1", "run-1"));
    let renderers = Renderers::default();
    let shown = render::screen(&session, 80, 24, &renderers);
    session.sync(&shown, Duration::ZERO);
    assert!(session.question_armed(APPROVAL_ARMING_DELAY));
    // A decision covers the card for a while.
    session.apply(Event::PermissionRequested {
        conversation: "room-a".into(),
        permission: Permission::new("p1", "agent", "run-1", "Run tests", "cargo test"),
    });
    let covered = render::screen(&session, 80, 24, &renderers);
    assert!(covered.question.is_none());
    session.sync(&covered, Duration::from_secs(5));
    session.apply(Event::PermissionResolved {
        conversation: "room-a".into(),
        permission: "p1".into(),
    });
    let back = render::screen(&session, 80, 24, &renderers);
    session.sync(&back, Duration::from_secs(10));
    assert!(!session.question_armed(Duration::from_secs(10)));
    assert!(session.question_armed(Duration::from_secs(10) + APPROVAL_ARMING_DELAY));
}

#[test]
fn a_view_keeps_a_bounded_number_of_question_cards() {
    let mut session = session();
    started(&mut session, "run-1");
    for index in 0..=limits::MAX_PENDING_QUESTIONS {
        session.apply(asked("room-a", &format!("q{index}"), "run-1"));
    }
    assert_eq!(
        session.observation().unwrap().questions.len(),
        limits::MAX_PENDING_QUESTIONS
    );
    assert!(!session.notice.is_empty());
}

#[test]
fn own_words_put_the_cursor_on_their_row() {
    let mut session = session();
    started(&mut session, "run-1");
    session.apply(asked("room-a", "q1", "run-1"));
    let card = session.question_mut().unwrap();
    card.typing = true;
    card.insert("DuckDB");
    let frame = render::screen(&session, 80, 24, &Renderers::default());
    let (column, row) = frame.cursor.unwrap();
    let line = &frame.lines[usize::from(row)];
    assert!(line.contains("3. DuckDB"), "{line}");
    assert_eq!(usize::from(column), line.trim_end().len());
}
