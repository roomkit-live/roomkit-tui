//! RMK-1317: a message handed to the running turn is read at the turn's next
//! stop. What the turn writes until then goes above the message, as it was
//! written without it; what it writes after goes below.

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

fn written(session: &mut Session, id: &str) {
    session.apply(Event::Item {
        conversation: "room-a".into(),
        item: Item::Message(Message::new(id, "person:alice", "A message")),
    });
}

fn hand(session: &mut Session, id: &str) {
    written(session, id);
    handed(session, id);
}

fn handed(session: &mut Session, id: &str) {
    session.apply(Event::MessageSteered {
        conversation: "room-a".into(),
        message: id.into(),
        run: RUN.into(),
    });
}

fn delta(session: &mut Session, draft: &str, text: &str) {
    session.apply(Event::TextDelta {
        conversation: "room-a".into(),
        message: draft.into(),
        actor: "agent".into(),
        run: RUN.into(),
        text: text.into(),
    });
}

fn finalized(session: &mut Session, draft: &str, id: &str) {
    session.apply(Event::MessageFinalized {
        conversation: "room-a".into(),
        draft: draft.into(),
        message: Message::new(id, "agent", "Some words"),
    });
}

fn call_of(session: &mut Session, run: &str, id: &str, state: ToolState) {
    session.apply(Event::Item {
        conversation: "room-a".into(),
        item: Item::Tool {
            actor: "agent".into(),
            run: run.into(),
            call: ToolCall::new(id, "lookup", json!({}), state),
        },
    });
}

fn call(session: &mut Session, id: &str, state: ToolState) {
    call_of(session, RUN, id, state);
}

fn finished(session: &mut Session, outcome: RunOutcome) {
    session.apply(Event::RunFinished {
        conversation: "room-a".into(),
        run: RUN.into(),
        outcome,
    });
}

fn order(session: &Session) -> Vec<String> {
    session
        .snapshot()
        .unwrap()
        .items
        .iter()
        .map(|item| match item {
            Item::Message(message) => message.id.as_str().to_owned(),
            Item::Tool { call, .. } => call.id.as_str().to_owned(),
            _ => "other".to_owned(),
        })
        .collect()
}

fn steering(session: &Session, id: &str) -> Option<Steering> {
    session.steering(&id.into())
}

/// The session of RMK-1317: sent before the model's first word, the message
/// stayed above an answer written without it.
#[test]
fn what_the_turn_writes_before_its_next_stop_goes_above_the_message() {
    let mut session = turn();
    hand(&mut session, "sent");
    delta(&mut session, "d0", "Fetching the basics.");
    finalized(&mut session, "d0", "a0");
    call(&mut session, "c1", ToolState::Running);
    call(&mut session, "c2", ToolState::Running);
    call(&mut session, "c1", ToolState::Succeeded);
    assert_eq!(order(&session), ["a0", "c1", "c2", "sent"]);
    assert_eq!(steering(&session, "sent"), Some(Steering::Handed));
}

#[test]
fn the_text_that_follows_the_round_of_calls_goes_below_and_read_it() {
    let mut session = turn();
    hand(&mut session, "sent");
    call(&mut session, "c1", ToolState::Running);
    call(&mut session, "c1", ToolState::Succeeded);
    assert_eq!(steering(&session, "sent"), Some(Steering::Handed));
    delta(&mut session, "d1", "Here is the answer");
    delta(&mut session, "d1", " in two parts.");
    finalized(&mut session, "d1", "a1");
    assert_eq!(order(&session), ["c1", "sent", "a1"]);
    assert_eq!(steering(&session, "sent"), Some(Steering::Read));
}

#[test]
fn the_calls_that_follow_the_round_of_calls_go_below_and_read_it() {
    let mut session = turn();
    hand(&mut session, "sent");
    call(&mut session, "c1", ToolState::Running);
    call(&mut session, "c1", ToolState::Failed);
    call(&mut session, "c2", ToolState::Running);
    assert_eq!(order(&session), ["c1", "sent", "c2"]);
    assert_eq!(steering(&session, "sent"), Some(Steering::Read));
}

#[test]
fn a_message_handed_while_calls_run_is_read_when_they_end() {
    let mut session = turn();
    call(&mut session, "c1", ToolState::Running);
    hand(&mut session, "sent");
    call(&mut session, "c1", ToolState::Succeeded);
    delta(&mut session, "d1", "The answer");
    assert_eq!(order(&session), ["c1", "sent", "d1"]);
    assert_eq!(steering(&session, "sent"), Some(Steering::Read));
}

/// The loop read its queue as the calls ended: a model that takes its time
/// goes on without the message, which it reads at the stop after that one.
#[test]
fn a_message_handed_after_the_calls_ended_waits_for_the_stop_after_the_next_output() {
    let mut session = turn();
    call(&mut session, "c1", ToolState::Running);
    call(&mut session, "c1", ToolState::Succeeded);
    hand(&mut session, "sent");
    delta(&mut session, "d1", "Going on");
    finalized(&mut session, "d1", "a1");
    call(&mut session, "c2", ToolState::Running);
    call(&mut session, "c2", ToolState::Succeeded);
    assert_eq!(order(&session), ["c1", "a1", "c2", "sent"]);
    assert_eq!(steering(&session, "sent"), Some(Steering::Handed));
    delta(&mut session, "d2", "The answer");
    assert_eq!(order(&session), ["c1", "a1", "c2", "sent", "d2"]);
    assert_eq!(steering(&session, "sent"), Some(Steering::Read));
}

/// The answer is a stop too: the loop asks the model again.
#[test]
fn the_text_after_an_answer_goes_below_and_read_it() {
    let mut session = turn();
    hand(&mut session, "sent");
    delta(&mut session, "d0", "First answer");
    finalized(&mut session, "d0", "a0");
    assert_eq!(order(&session), ["a0", "sent"]);
    assert_eq!(steering(&session, "sent"), Some(Steering::Handed));
    delta(&mut session, "d1", "Second answer");
    assert_eq!(order(&session), ["a0", "sent", "d1"]);
    assert_eq!(steering(&session, "sent"), Some(Steering::Read));
}

/// Text and calls of one model answer: its text ends as its calls start.
#[test]
fn the_calls_after_the_text_of_the_same_answer_stay_above() {
    let mut session = turn();
    hand(&mut session, "sent");
    delta(&mut session, "d0", "Looking");
    finalized(&mut session, "d0", "a0");
    call(&mut session, "c1", ToolState::Running);
    assert_eq!(order(&session), ["a0", "c1", "sent"]);
    assert_eq!(steering(&session, "sent"), Some(Steering::Handed));
}

/// Blanks end no segment: the next round's text extends the same draft.
#[test]
fn blanks_before_a_call_read_nothing_and_their_draft_ends_below() {
    let mut session = turn();
    hand(&mut session, "sent");
    delta(&mut session, "d0", "\n\n");
    call(&mut session, "c1", ToolState::Running);
    assert_eq!(steering(&session, "sent"), Some(Steering::Handed));
    call(&mut session, "c1", ToolState::Succeeded);
    delta(&mut session, "d0", "The answer");
    finalized(&mut session, "d0", "a0");
    assert_eq!(order(&session), ["c1", "sent", "a0"]);
    assert_eq!(steering(&session, "sent"), Some(Steering::Read));
}

#[test]
fn blanks_that_become_text_stay_above_while_the_turn_read_nothing() {
    let mut session = turn();
    delta(&mut session, "d0", " ");
    hand(&mut session, "sent");
    delta(&mut session, "d0", "Words");
    assert_eq!(order(&session), ["d0", "sent"]);
    assert_eq!(steering(&session, "sent"), Some(Steering::Handed));
}

#[test]
fn each_message_waits_for_its_own_stop() {
    let mut session = turn();
    hand(&mut session, "first");
    call(&mut session, "c1", ToolState::Running);
    call(&mut session, "c1", ToolState::Succeeded);
    hand(&mut session, "second");
    delta(&mut session, "d1", "Going on");
    assert_eq!(order(&session), ["c1", "first", "d1", "second"]);
    assert_eq!(steering(&session, "first"), Some(Steering::Read));
    assert_eq!(steering(&session, "second"), Some(Steering::Handed));
}

/// Written just before the host said where it went, or as text came in.
#[test]
fn a_handed_message_goes_below_what_came_before_the_host_said_so() {
    let mut session = turn();
    written(&mut session, "sent");
    delta(&mut session, "d0", "Words");
    handed(&mut session, "sent");
    delta(&mut session, "d0", " go on");
    finalized(&mut session, "d0", "a0");
    assert_eq!(order(&session), ["a0", "sent"]);
}

#[test]
fn a_message_not_handed_stays_where_it_was_written() {
    let mut session = turn();
    hand(&mut session, "sent");
    written(&mut session, "next");
    delta(&mut session, "d0", "Words");
    assert_eq!(order(&session), ["d0", "sent", "next"]);
}

#[test]
fn a_call_of_another_run_reads_nothing() {
    let mut session = turn();
    hand(&mut session, "sent");
    call_of(&mut session, "other", "c1", ToolState::Succeeded);
    call_of(&mut session, "other", "c2", ToolState::Running);
    assert_eq!(steering(&session, "sent"), Some(Steering::Handed));
}

#[test]
fn the_next_turn_writes_below_what_the_last_one_did_not_read() {
    let mut session = turn();
    hand(&mut session, "sent");
    delta(&mut session, "d0", "An answer");
    finalized(&mut session, "d0", "a0");
    finished(&mut session, RunOutcome::Completed);
    assert_eq!(steering(&session, "sent"), Some(Steering::Handed));
    started(&mut session, "agent:other");
    session.apply(Event::TextDelta {
        conversation: "room-a".into(),
        message: "d9".into(),
        actor: "agent".into(),
        run: "agent:other".into(),
        text: "Another".into(),
    });
    assert_eq!(order(&session), ["a0", "sent", "d9"]);
}

/// A snapshot states the live runs: a turn it no longer names told no end.
#[test]
fn a_snapshot_without_the_turn_leaves_nothing_waiting_for_it() {
    let mut session = turn();
    hand(&mut session, "sent");
    let mut idle = snapshot();
    idle.can_steer = true;
    idle.local_actor = Some("person:alice".into());
    idle.items = vec![Item::Message(Message::new(
        "sent",
        "person:alice",
        "A message",
    ))];
    session.apply(Event::Snapshot(idle));
    started(&mut session, "agent:other");
    call_of(&mut session, "agent:other", "c1", ToolState::Running);
    assert_eq!(order(&session), ["sent", "c1"]);
}

/// Each time a stop reads, no flag of the last answer survives it.
#[test]
fn an_answer_ended_counts_once() {
    let mut session = turn();
    hand(&mut session, "first");
    delta(&mut session, "d0", "An answer");
    finalized(&mut session, "d0", "a0");
    delta(&mut session, "d1", "Next");
    assert_eq!(steering(&session, "first"), Some(Steering::Read));
    finalized(&mut session, "d1", "a1");
    hand(&mut session, "second");
    delta(&mut session, "d2", "Another");
    assert_eq!(order(&session), ["a0", "first", "a1", "d2", "second"]);
    assert_eq!(steering(&session, "second"), Some(Steering::Handed));
}

#[test]
fn a_message_that_gets_its_own_turn_no_longer_waits() {
    let mut session = turn();
    hand(&mut session, "sent");
    session.apply(Event::RunStarted {
        conversation: "room-a".into(),
        run: "agent:own".into(),
        answers: Some("sent".into()),
    });
    delta(&mut session, "d0", "Words");
    assert_eq!(order(&session), ["sent", "d0"]);
    assert_eq!(steering(&session, "sent"), Some(Steering::OwnTurn));
}

#[test]
fn the_line_under_a_message_says_the_turn_read_it() {
    let draw = |session: &Session| {
        render::screen_with_options(
            session,
            60,
            30,
            &Renderers::default(),
            &UiOptions::default(),
            Duration::ZERO,
        )
        .lines
        .join("\n")
    };
    let mut session = turn();
    hand(&mut session, "sent");
    assert!(draw(&session).contains("↳ sent to the running turn"));
    call(&mut session, "c1", ToolState::Running);
    call(&mut session, "c1", ToolState::Succeeded);
    delta(&mut session, "d1", "The answer");
    let screen = draw(&session);
    assert!(screen.contains("↳ read by the turn"), "{screen}");
    assert!(!screen.contains("sent to the running turn"), "{screen}");
}
