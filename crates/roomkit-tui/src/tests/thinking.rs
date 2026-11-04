//! RMK-1325: a model's reasoning shows as it comes, and folds to one line once
//! the model goes on. Nothing stores it.

#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]

use std::time::Duration;

use super::*;
use crate::render::{self, Renderers};
use crate::{Steering, UiOptions};

const RUN: &str = "agent:m1";

fn turn() -> Session {
    let mut snapshot = snapshot();
    snapshot.can_steer = true;
    snapshot.local_actor = Some("person:alice".into());
    let mut session = Session::default();
    session.apply(Event::Snapshot(snapshot));
    started(&mut session, RUN);
    session
}

fn secs(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

fn thinks_at(session: &mut Session, at: u64, text: &str) {
    let event = Event::ThinkingDelta {
        conversation: "room-a".into(),
        actor: "agent".into(),
        run: RUN.into(),
        text: text.into(),
    };
    session.apply_at(event, secs(at));
}

fn writes_at(session: &mut Session, at: u64, text: &str) {
    let event = Event::TextDelta {
        conversation: "room-a".into(),
        message: "d0".into(),
        actor: "agent".into(),
        run: RUN.into(),
        text: text.into(),
    };
    session.apply_at(event, secs(at));
}

fn calls(session: &mut Session, at: u64, id: &str, state: ToolState) {
    let event = Event::Item {
        conversation: "room-a".into(),
        item: Item::Tool {
            actor: "agent".into(),
            run: RUN.into(),
            call: ToolCall::new(id, "lookup", json!({}), state),
        },
    };
    session.apply_at(event, secs(at));
}

fn draw(session: &Session, at: u64) -> String {
    render::screen_with_options(
        session,
        60,
        30,
        &Renderers::default(),
        &UiOptions::default(),
        secs(at),
    )
    .lines
    .join("\n")
}

fn order(session: &Session) -> Vec<&'static str> {
    session
        .snapshot()
        .unwrap()
        .items
        .iter()
        .map(|item| match item {
            Item::Thinking(_) => "thinking",
            Item::Tool { .. } => "tool",
            Item::Message(message) if message.actor.as_str() == "agent" => "answer",
            Item::Message(_) => "person",
            _ => "other",
        })
        .collect()
}

#[test]
fn reasoning_shows_its_last_lines_and_its_time_while_it_comes() {
    let mut session = turn();
    thinks_at(&mut session, 10, "one\ntwo\nthree\n");
    thinks_at(&mut session, 11, "four");
    let screen = draw(&session, 22);
    assert!(screen.contains("∴ Thinking · 12s"), "{screen}");
    assert!(
        screen.contains("two") && screen.contains("four"),
        "{screen}"
    );
    assert!(!screen.contains("one"), "{screen}");
    assert_eq!(order(&session), ["thinking"], "One block");
}

#[test]
fn the_answer_folds_the_reasoning_to_one_line() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing the options");
    writes_at(&mut session, 22, "The answer");
    let screen = draw(&session, 99);
    assert!(screen.contains("∴ Thought for 12.0s"), "{screen}");
    assert!(!screen.contains("Weighing"), "{screen}");
    assert!(screen.contains("The answer"), "{screen}");
    assert_eq!(order(&session), ["thinking", "answer"]);
}

#[test]
fn blanks_between_the_reasoning_and_the_answer_do_not_end_it() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing");
    writes_at(&mut session, 12, "\n\n");
    assert!(draw(&session, 13).contains("∴ Thinking"));
    writes_at(&mut session, 14, "Words");
    assert!(draw(&session, 15).contains("∴ Thought for 4.0s"));
}

#[test]
fn a_call_and_the_end_of_the_run_end_the_reasoning_too() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing");
    calls(&mut session, 11, "c1", ToolState::Running);
    assert!(draw(&session, 12).contains("∴ Thought for 1.0s"));

    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing");
    session.apply_at(
        Event::RunFinished {
            conversation: "room-a".into(),
            run: RUN.into(),
            outcome: RunOutcome::Interrupted,
        },
        secs(13),
    );
    assert!(draw(&session, 14).contains("∴ Thought for 3.0s"));
}

#[test]
fn each_answer_of_the_model_has_its_own_block_in_the_turn_s_order() {
    let mut session = turn();
    thinks_at(&mut session, 10, "First");
    calls(&mut session, 11, "c1", ToolState::Running);
    calls(&mut session, 12, "c1", ToolState::Succeeded);
    thinks_at(&mut session, 13, "Second");
    assert_eq!(order(&session), ["thinking", "tool", "thinking"]);
    let screen = draw(&session, 14);
    assert!(screen.contains("∴ Thought for 1.0s"), "{screen}");
    assert!(screen.contains("∴ Thinking · 1s"), "{screen}");
    assert!(
        screen.contains("Second") && !screen.contains("First"),
        "{screen}"
    );
}

#[test]
fn a_folded_block_opens_and_closes_as_a_call_s_details_do() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing the options");
    writes_at(&mut session, 12, "Words");
    assert!(draw(&session, 13).contains("· F2 expand"));
    session.toggle_tool();
    let opened = draw(&session, 13);
    assert!(opened.contains("Weighing the options"), "{opened}");
    assert!(opened.contains("· F2 collapse"), "{opened}");
    session.toggle_tool();
    assert!(!draw(&session, 13).contains("Weighing"));
}

#[test]
fn an_opened_block_shows_a_bounded_number_of_lines() {
    let mut session = turn();
    let long: Vec<String> = (0..150).map(|index| format!("line-{index}")).collect();
    thinks_at(&mut session, 10, &long.join("\n"));
    writes_at(&mut session, 11, "Words");
    session.toggle_tool();
    let lines = render::screen_with_options(
        &session,
        100,
        200,
        &Renderers::default(),
        &UiOptions::default(),
        secs(12),
    )
    .lines;
    assert!(lines.iter().any(|line| line.contains("line-99")));
    assert!(!lines.iter().any(|line| line.contains("line-100")));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("view limited to 100 lines"))
    );
}

/// A model's words are no terminal sequences.
#[test]
fn reasoning_cannot_drive_the_terminal() {
    let mut session = turn();
    thinks_at(&mut session, 10, "plain\u{1b}[2Jtext\u{7}");
    let screen = draw(&session, 11);
    assert!(
        !screen.contains('\u{1b}') && !screen.contains('\u{7}'),
        "{screen:?}"
    );
}

#[test]
fn blank_reasoning_takes_no_room() {
    let mut session = turn();
    thinks_at(&mut session, 10, "\n \n");
    assert!(!draw(&session, 11).contains('∴'));
}

#[test]
fn reasoning_of_another_conversation_or_of_a_finished_run_is_ignored() {
    let mut session = turn();
    session.apply(Event::ThinkingDelta {
        conversation: "room-b".into(),
        actor: "agent".into(),
        run: RUN.into(),
        text: "Elsewhere".into(),
    });
    session.apply(Event::ThinkingDelta {
        conversation: "room-a".into(),
        actor: "agent".into(),
        run: "agent:old".into(),
        text: "Before".into(),
    });
    assert!(order(&session).is_empty());
}

/// A view that came back reads what the host holds, and it holds no reasoning.
#[test]
fn a_snapshot_drops_the_reasoning() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing");
    session.apply(Event::Snapshot(snapshot()));
    assert!(order(&session).is_empty());
    assert!(!draw(&session, 11).contains('∴'));
}

#[test]
fn the_view_bounds_a_reasoning_without_end() {
    let mut session = turn();
    let limit = crate::Limits::default().text_bytes.get();
    for at in 0..4 {
        thinks_at(&mut session, at, &"x".repeat(limit));
    }
    let snapshot = session.snapshot().unwrap();
    let Item::Thinking(block) = &snapshot.items[0] else {
        panic!("Expected reasoning");
    };
    assert!(block.text.len() <= limit + 64, "{}", block.text.len());
}

fn hand(session: &mut Session, id: &str) {
    session.apply(Event::Item {
        conversation: "room-a".into(),
        item: Item::Message(Message::new(id, "person:alice", "A message")),
    });
    session.apply(Event::MessageSteered {
        conversation: "room-a".into(),
        message: id.into(),
        run: RUN.into(),
    });
}

/// RMK-1317: reasoning is what the turn writes, so it says when it read.
#[test]
fn reasoning_after_the_calls_goes_below_the_message_it_read() {
    let mut session = turn();
    hand(&mut session, "sent");
    calls(&mut session, 10, "c1", ToolState::Running);
    calls(&mut session, 11, "c1", ToolState::Succeeded);
    assert_eq!(session.steering(&"sent".into()), Some(Steering::Handed));
    thinks_at(&mut session, 12, "Now with the message");
    assert_eq!(order(&session), ["tool", "person", "thinking"]);
    assert_eq!(session.steering(&"sent".into()), Some(Steering::Read));
}

#[test]
fn reasoning_written_without_the_message_stays_above_it() {
    let mut session = turn();
    hand(&mut session, "sent");
    thinks_at(&mut session, 10, "Weighing");
    writes_at(&mut session, 11, "Words");
    assert_eq!(order(&session), ["thinking", "answer", "person"]);
    assert_eq!(session.steering(&"sent".into()), Some(Steering::Handed));
}

/// Handed once the calls ended, its reasoning is the next answer's, written
/// without the message, and the words after it are that same answer.
#[test]
fn words_after_reasoning_are_one_answer_for_a_message_handed_after_the_calls() {
    let mut session = turn();
    calls(&mut session, 10, "c1", ToolState::Running);
    calls(&mut session, 11, "c1", ToolState::Succeeded);
    hand(&mut session, "sent");
    thinks_at(&mut session, 12, "Going on without it");
    writes_at(&mut session, 13, "Words");
    writes_at(&mut session, 14, " go on");
    assert_eq!(order(&session), ["tool", "thinking", "answer", "person"]);
    assert_eq!(session.steering(&"sent".into()), Some(Steering::Handed));
}
