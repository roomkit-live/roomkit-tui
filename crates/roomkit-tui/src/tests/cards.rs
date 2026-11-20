//! A question card is shown whole: its options' labels, which are what is
//! sent, and the person's own words with their caret.

#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use roomkit_tui_protocol::*;
use unicode_width::UnicodeWidthStr;

use super::*;
use crate::render::{self, Renderers};

fn asks(session: &mut Session, label: &str) {
    let options = vec![
        QuestionOption::new(label, "The first way"),
        QuestionOption::new("Postgres", "Shared between machines"),
    ];
    let item = QuestionItem::new("Database", "Which database holds the sessions?", options);
    session.apply(Event::QuestionAsked {
        conversation: "room-a".into(),
        question: Question::new("q1", "agent", "run-1", vec![item]),
    });
}

/// The words of `frame`, row after row.
fn words(frame: &render::Frame) -> Vec<String> {
    frame
        .lines
        .iter()
        .flat_map(|line| line.split_whitespace().map(str::to_owned))
        .collect()
}

#[test]
fn a_long_option_label_wraps_whole_under_its_number() {
    let label: Vec<String> = (0..36).map(|index| format!("part{index}")).collect();
    let label = label.join(" ");
    let mut session = session();
    started(&mut session, "run-1");
    asks(&mut session, &label);
    let frame = render::screen(&session, 60, 40, &Renderers::default());
    let shown = words(&frame);
    for part in label.split(' ') {
        assert!(
            shown.iter().any(|word| word == part),
            "{part} is cut: {shown:?}"
        );
    }
    let first = frame
        .lines
        .iter()
        .position(|l| l.contains("1. part0"))
        .unwrap();
    let label = &frame.lines[first];
    let indent = label[..label.find("part0").unwrap()].width();
    let next = &frame.lines[first + 1];
    let margin = next.len() - next.trim_start().len();
    assert_eq!(margin, indent, "hanging indent: {next:?}");
    assert!(next.trim_start().starts_with("part"), "{next:?}");
}

/// The cursor's column on its row, and the text of that row before it.
fn before_cursor(frame: &render::Frame) -> (usize, String) {
    let (column, row) = frame.cursor.unwrap();
    let line = &frame.lines[usize::from(row)];
    let column = usize::from(column);
    (column, line.chars().take(column).collect())
}

fn writes(session: &mut Session, words: &str) {
    let card = session.question_mut().unwrap();
    card.typing = true;
    card.insert(words);
}

#[test]
fn the_cursor_in_own_words_follows_the_caret() {
    let mut session = session();
    started(&mut session, "run-1");
    asks(&mut session, "SQLite");
    writes(&mut session, "DuckDB");
    for _ in 0..3 {
        let left = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        session.question_mut().unwrap().key(left);
    }
    let frame = render::screen(&session, 80, 30, &Renderers::default());
    let (_, before) = before_cursor(&frame);
    assert!(before.ends_with("3. Duc"), "{before:?}");
}

#[test]
fn long_own_words_wrap_on_a_few_rows_that_follow_the_caret() {
    let words: Vec<String> = (0..80).map(|index| format!("w{index}")).collect();
    let mut session = session();
    started(&mut session, "run-1");
    asks(&mut session, "SQLite");
    writes(&mut session, &words.join(" "));
    let frame = render::screen(&session, 60, 40, &Renderers::default());
    let (column, before) = before_cursor(&frame);
    assert!(before.ends_with("w79"), "{before:?}");
    assert!(column < 59, "the caret is not pinned at the edge");
    // The words take three rows at most, the first beside the number.
    let row = usize::from(frame.cursor.unwrap().1);
    let own = (0..=row)
        .rev()
        .find(|&index| frame.lines[index].contains("3. "))
        .unwrap();
    assert!(row - own <= 2, "{:#?}", &frame.lines[own..=row]);
    assert!(
        !frame.lines[own].contains("w0 "),
        "the window follows the caret"
    );

    // A card that fits with one row of words stays on screen as they grow.
    let mut short = super::session();
    started(&mut short, "run-1");
    asks(&mut short, "SQLite");
    writes(&mut short, "w");
    let fits = (10..40)
        .find(|&height| {
            render::screen(&short, 60, height, &Renderers::default())
                .question
                .is_some()
        })
        .unwrap();
    writes(&mut short, &words.join(" "));
    let frame = render::screen(&short, 60, fits, &Renderers::default());
    assert!(frame.question.is_some(), "the card is still shown");
    assert!(before_cursor(&frame).1.ends_with("w79"));
}
