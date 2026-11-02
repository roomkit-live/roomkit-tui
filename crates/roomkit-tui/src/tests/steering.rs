//! RMK-1161: a message written while the agent's turn runs goes to that turn
//! when the host steers, and shows where it went.

#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]

use std::time::Duration;

use super::*;
use crate::render::{self, Renderers};
use crate::{SessionError, Steering, UiOptions};

fn steering_session() -> Session {
    let mut snapshot = snapshot();
    snapshot.can_steer = true;
    let mut session = Session::default();
    session.apply(Event::Snapshot(snapshot));
    session
}

#[test]
fn a_message_during_a_turn_is_sent_only_when_the_host_steers() {
    let mut refusing = session();
    started(&mut refusing, "agent:m1");
    refusing.set_draft("Use the second ledger");
    assert_eq!(refusing.input_command(), Err(SessionError::Busy));

    let mut steering = steering_session();
    started(&mut steering, "agent:m1");
    steering.set_draft("Use the second ledger");
    assert!(matches!(
        steering.input_command(),
        Ok(Command::Submit { text, .. }) if text == "Use the second ledger"
    ));
    // Images are no message the host hands to a turn.
    steering.images.push(image_fixture());
    assert_eq!(steering.input_command(), Err(SessionError::Busy));
}

#[test]
fn a_steered_message_shows_where_it_went() {
    let mut session = steering_session();
    started(&mut session, "agent:m1");
    let sent = Message::new("m2", "person:alice", "Use the second ledger");
    session.apply(Event::Item {
        conversation: "room-a".into(),
        item: Item::Message(sent),
    });
    let message = MessageId::new("m2");
    assert_eq!(session.steering(&message), None);
    session.apply(Event::MessageSteered {
        conversation: "room-a".into(),
        message: message.clone(),
        run: "agent:m1".into(),
    });
    assert_eq!(session.steering(&message), Some(Steering::Handed));
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
    assert!(draw(&session).contains("↳ sent to the running turn"));

    // The turn no longer read it: a turn of its own answers it.
    session.apply(Event::RunStarted {
        conversation: "room-a".into(),
        run: "agent:m2".into(),
        answers: Some(message.clone()),
    });
    assert_eq!(session.steering(&message), Some(Steering::OwnTurn));
    assert!(draw(&session).contains("↳ answered in its own turn"));
    // A replayed hand-over never undoes it.
    session.apply(Event::MessageSteered {
        conversation: "room-a".into(),
        message: message.clone(),
        run: "agent:m1".into(),
    });
    assert_eq!(session.steering(&message), Some(Steering::OwnTurn));
}

#[test]
fn steering_of_another_conversation_is_ignored() {
    let mut session = steering_session();
    session.apply(Event::MessageSteered {
        conversation: "room-b".into(),
        message: "m2".into(),
        run: "agent:m1".into(),
    });
    assert_eq!(session.steering(&"m2".into()), None);
}

/// The review of 5374eb1: when a message's own turn started before the host
/// said where it went, the late hand-over showed it sent to the running turn.
#[test]
fn a_hand_over_after_its_own_turn_started_shows_its_own_turn() {
    let mut session = steering_session();
    let message = MessageId::new("m2");
    session.apply(Event::RunStarted {
        conversation: "room-a".into(),
        run: "agent:m2".into(),
        answers: Some(message.clone()),
    });
    assert_eq!(
        session.steering(&message),
        None,
        "A plain turn shows nothing"
    );
    session.apply(Event::MessageSteered {
        conversation: "room-a".into(),
        message: message.clone(),
        run: "agent:m1".into(),
    });
    assert_eq!(session.steering(&message), Some(Steering::OwnTurn));
}

#[test]
fn another_conversation_forgets_where_messages_went() {
    let mut session = steering_session();
    session.apply(Event::MessageSteered {
        conversation: "room-a".into(),
        message: "m2".into(),
        run: "agent:m1".into(),
    });
    let mut other = snapshot();
    other.conversation = "room-b".into();
    session.apply(Event::Snapshot(other));
    assert_eq!(session.steering(&"m2".into()), None);
}

#[test]
fn identities_beyond_the_bound_are_ignored() {
    let mut session = steering_session();
    let long = MessageId::new("m".repeat(300));
    session.apply(Event::MessageSteered {
        conversation: "room-a".into(),
        message: long.clone(),
        run: "agent:m1".into(),
    });
    assert_eq!(session.steering(&long), None);
}

/// The review of 5374eb1: a message sent while the turn was being stopped
/// was neither read nor answered.
#[test]
fn a_message_while_the_turn_stops_waits() {
    let mut session = steering_session();
    started(&mut session, "agent:m1");
    let stop = session
        .request(Command::Interrupt {
            run: "agent:m1".into(),
        })
        .unwrap();
    session.sent(&stop);
    session.set_draft("Too late");
    assert_eq!(session.input_command(), Err(SessionError::Busy));
}
