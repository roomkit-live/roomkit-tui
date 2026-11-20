//! A question card is shown whole: its options' labels, which are what is
//! sent, and the person's own words with their caret.

#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]

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
