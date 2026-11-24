//! Placement through the same host events used by the terminal.
use super::*;
use roomkit_tui_protocol::RowPosition;

fn stored(id: &str, index: u64, follows: Option<u64>) -> Message {
    let mut message = Message::new(id, "agent", id);
    message.position = Some(RowPosition::stored(index, follows));
    message
}

fn publish(session: &mut Session, message: Message) {
    session.apply(Event::Item {
        conversation: "room-a".into(),
        item: Item::Message(message),
    });
}

fn first_word(session: &mut Session, follows_index: u64) {
    session.apply(Event::DraftPosition {
        conversation: "room-a".into(),
        message: "draft".into(),
        actor: "agent".into(),
        run: "run".into(),
        follows_index,
    });
    session.apply(Event::TextDelta {
        conversation: "room-a".into(),
        message: "draft".into(),
        actor: "agent".into(),
        run: "run".into(),
        text: "Waiting".into(),
    });
}

fn texts(session: &Session) -> Vec<&str> {
    session
        .snapshot()
        .into_iter()
        .flat_map(|s| &s.items)
        .filter_map(|item| match item {
            Item::Message(m) => Some(m.text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_first_word_precedes_an_intervening_message_and_its_replay_is_unique() {
    let mut session = session();
    started(&mut session, "run");
    publish(&mut session, stored("Peer replied", 4, None));
    first_word(&mut session, 3);
    assert_eq!(texts(&session), ["Waiting", "Peer replied"]);
    session.apply(Event::MessageFinalized {
        conversation: "room-a".into(),
        draft: "draft".into(),
        message: stored("Waiting", 5, Some(3)),
    });
    publish(&mut session, stored("Summary", 6, Some(5)));
    publish(&mut session, stored("Peer replied", 4, None));
    assert_eq!(texts(&session), ["Waiting", "Peer replied", "Summary"]);
}

#[test]
fn the_committed_text_corrects_an_anchor_estimated_before_a_hidden_tool() {
    let mut session = session();
    started(&mut session, "run");
    publish(&mut session, stored("Peer replied", 4, None));
    first_word(&mut session, 3);
    session.apply(Event::MessageFinalized {
        conversation: "room-a".into(),
        draft: "draft".into(),
        message: stored("Waiting", 7, Some(6)),
    });
    assert_eq!(texts(&session), ["Peer replied", "Waiting"]);
}
