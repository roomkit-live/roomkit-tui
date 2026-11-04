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

fn delta_of(session: &mut Session, run: &str, draft: &str, text: &str) {
    session.apply(Event::TextDelta {
        conversation: "room-a".into(),
        message: draft.into(),
        actor: "agent".into(),
        run: run.into(),
        text: text.into(),
    });
}

fn delta(session: &mut Session, draft: &str, text: &str) {
    delta_of(session, RUN, draft, text);
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
    delta_of(&mut session, "agent:other", "d9", "Another");
    assert_eq!(order(&session), ["a0", "sent", "d9"]);
}

/// Review of 9c540f0: blanks go on the draft of one answer, and the first of
/// them said the stop the message waited for has passed.
#[test]
fn blanks_after_the_calls_are_one_answer_however_many() {
    let mut session = turn();
    call(&mut session, "c1", ToolState::Running);
    call(&mut session, "c1", ToolState::Succeeded);
    hand(&mut session, "sent");
    delta(&mut session, "d1", "\n");
    delta(&mut session, "d1", "\n");
    delta(&mut session, "d1", "Going on");
    assert_eq!(order(&session), ["c1", "d1", "sent"]);
    assert_eq!(steering(&session, "sent"), Some(Steering::Handed));
}

#[test]
fn a_message_handed_during_the_blanks_of_an_answer_waits_for_its_end() {
    let mut session = turn();
    call(&mut session, "c1", ToolState::Running);
    call(&mut session, "c1", ToolState::Succeeded);
    delta(&mut session, "d1", "\n\n");
    hand(&mut session, "sent");
    delta(&mut session, "d1", "Going on");
    assert_eq!(order(&session), ["c1", "d1", "sent"]);
    assert_eq!(steering(&session, "sent"), Some(Steering::Handed));
}

#[test]
fn text_that_goes_on_after_the_calls_reads_nothing() {
    let mut session = turn();
    call(&mut session, "c1", ToolState::Running);
    call(&mut session, "c1", ToolState::Succeeded);
    delta(&mut session, "d1", "Going");
    hand(&mut session, "sent");
    delta(&mut session, "d1", " on");
    assert_eq!(order(&session), ["c1", "d1", "sent"]);
    assert_eq!(steering(&session, "sent"), Some(Steering::Handed));
}

/// Handed once the answer ended, the loop had read its queue already.
#[test]
fn a_message_handed_after_an_answer_waits_for_the_stop_after_the_next_one() {
    let mut session = turn();
    hand(&mut session, "first");
    delta(&mut session, "d0", "An answer");
    finalized(&mut session, "d0", "a0");
    hand(&mut session, "second");
    delta(&mut session, "d1", "Next");
    assert_eq!(order(&session), ["a0", "first", "d1", "second"]);
    assert_eq!(steering(&session, "first"), Some(Steering::Read));
    assert_eq!(steering(&session, "second"), Some(Steering::Handed));
}

/// The text of one answer ends as its calls start: no answer ended, and a
/// message handed during those calls is read when they end.
#[test]
fn calls_after_text_leave_no_answer_ended() {
    let mut session = turn();
    hand(&mut session, "first");
    delta(&mut session, "d0", "Looking");
    finalized(&mut session, "d0", "a0");
    call(&mut session, "c1", ToolState::Running);
    hand(&mut session, "second");
    call(&mut session, "c1", ToolState::Succeeded);
    delta(&mut session, "d1", "The answer");
    assert_eq!(order(&session), ["a0", "c1", "first", "second", "d1"]);
    assert_eq!(steering(&session, "second"), Some(Steering::Read));
}

/// A stop reads once: the answer that made it no longer counts.
#[test]
fn a_stop_leaves_no_answer_ended() {
    let mut session = turn();
    hand(&mut session, "first");
    delta(&mut session, "d0", "An answer");
    finalized(&mut session, "d0", "a0");
    delta(&mut session, "d1", "Next");
    hand(&mut session, "second");
    finalized(&mut session, "d1", "a1");
    delta(&mut session, "d2", "Last");
    assert_eq!(order(&session), ["a0", "first", "a1", "second", "d2"]);
    assert_eq!(steering(&session, "second"), Some(Steering::Read));
}

/// With nothing held, text that ends with its calls leaves no answer ended.
#[test]
fn an_answer_with_nothing_held_counts_for_nothing_handed_later() {
    let mut session = turn();
    hand(&mut session, "first");
    call(&mut session, "c1", ToolState::Running);
    call(&mut session, "c1", ToolState::Succeeded);
    delta(&mut session, "d1", "Looking again");
    assert_eq!(steering(&session, "first"), Some(Steering::Read));
    finalized(&mut session, "d1", "a1");
    call(&mut session, "c2", ToolState::Running);
    hand(&mut session, "second");
    call(&mut session, "c2", ToolState::Succeeded);
    delta(&mut session, "d2", "The answer");
    assert_eq!(steering(&session, "second"), Some(Steering::Read));
}

/// Only the turn's own output tells its stop or goes above what it holds.
#[test]
fn what_another_run_does_neither_reads_nor_goes_above() {
    let mut session = turn();
    hand(&mut session, "sent");
    call_of(&mut session, "other", "c1", ToolState::Succeeded);
    delta(&mut session, "d0", "Words");
    assert_eq!(order(&session), ["d0", "sent", "c1"]);
    assert_eq!(steering(&session, "sent"), Some(Steering::Handed));
}

/// Review of 583e681: a call another run has under way says nothing of this
/// turn's calls.
#[test]
fn a_call_another_run_has_running_hides_no_stop() {
    let mut session = turn();
    hand(&mut session, "sent");
    call(&mut session, "c1", ToolState::Running);
    call(&mut session, "c1", ToolState::Succeeded);
    call_of(&mut session, "other", "x", ToolState::Running);
    delta(&mut session, "d1", "The answer");
    assert_eq!(order(&session), ["c1", "sent", "x", "d1"]);
    assert_eq!(steering(&session, "sent"), Some(Steering::Read));
}

#[test]
fn the_text_of_another_run_goes_below() {
    let mut session = turn();
    hand(&mut session, "sent");
    started(&mut session, "agent:other");
    delta_of(&mut session, "agent:other", "d9", "Words");
    assert_eq!(order(&session), ["sent", "d9"]);
}

#[test]
fn a_decision_of_another_run_goes_below() {
    let mut session = turn();
    hand(&mut session, "sent");
    let permission = Permission::new("p1", "agent", "other", "Read a record", "Record 42");
    let decision = PermissionDecision::new(permission, Decision::Allow, "person:alice");
    session.apply(Event::Item {
        conversation: "room-a".into(),
        item: Item::Approval(decision),
    });
    assert_eq!(order(&session), ["sent", "other"]);
}

/// Review of 583e681: the host says where a message went once the hooks that
/// admit it returned, which the next turn may outlast. That turn keeps what it
/// was handed.
#[test]
fn a_message_handed_to_a_turn_that_ended_takes_nothing_from_the_next_one() {
    let mut session = turn();
    written(&mut session, "late");
    finished(&mut session, RunOutcome::Completed);
    started(&mut session, "agent:next");
    written(&mut session, "sent");
    session.apply(Event::MessageSteered {
        conversation: "room-a".into(),
        message: "sent".into(),
        run: "agent:next".into(),
    });
    handed(&mut session, "late");
    delta_of(&mut session, "agent:next", "d9", "Words");
    assert_eq!(order(&session), ["late", "d9", "sent"]);
}

#[test]
fn what_people_write_after_the_calls_hides_no_stop() {
    let mut session = turn();
    hand(&mut session, "sent");
    call(&mut session, "c1", ToolState::Running);
    call(&mut session, "c1", ToolState::Succeeded);
    written(&mut session, "next");
    session.apply(Event::Item {
        conversation: "room-a".into(),
        item: Item::Message(Message::new("sms", "+15550001", "From elsewhere")),
    });
    delta(&mut session, "d1", "The answer");
    assert_eq!(order(&session), ["c1", "sent", "next", "sms", "d1"]);
    assert_eq!(steering(&session, "sent"), Some(Steering::Read));
}

/// The turn's request for a decision is its output too.
#[test]
fn a_decision_of_the_turn_goes_above() {
    let mut session = turn();
    hand(&mut session, "sent");
    let permission = Permission::new("p1", "agent", RUN, "Read a record", "Record 42");
    let decision = PermissionDecision::new(permission, Decision::Allow, "person:alice");
    session.apply(Event::Item {
        conversation: "room-a".into(),
        item: Item::Approval(decision),
    });
    assert_eq!(order(&session), ["other", "sent"]);
}

/// An answer the turn never streamed takes the place of a draft.
#[test]
fn an_answer_without_a_draft_goes_above() {
    let mut session = turn();
    hand(&mut session, "sent");
    finalized(&mut session, "d0", "a0");
    assert_eq!(order(&session), ["a0", "sent"]);
}

/// The host may say where a message went once its turn ended, as the hooks that
/// admit it can outlast the turn: nothing would then forget the hold.
#[test]
fn a_message_handed_to_a_turn_that_ended_holds_nothing() {
    let mut session = turn();
    written(&mut session, "sent");
    finished(&mut session, RunOutcome::Completed);
    handed(&mut session, "sent");
    started(&mut session, "agent:other");
    delta_of(&mut session, "agent:other", "d9", "Another");
    finalized(&mut session, "d8", "a8");
    assert_eq!(order(&session), ["sent", "d9", "a8"]);
}

/// A host that steers without end must not grow the view: beyond the bound the
/// oldest message no longer waits, and the turn's output goes below it.
#[test]
fn a_turn_holds_a_bounded_number_of_unread_messages() {
    let mut session = turn();
    for index in 0..65 {
        hand(&mut session, &format!("m{index}"));
    }
    delta(&mut session, "d0", "Words");
    assert_eq!(order(&session)[..3], ["m0", "d0", "m1"]);
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
