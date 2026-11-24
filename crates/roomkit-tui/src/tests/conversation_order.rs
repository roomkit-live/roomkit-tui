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

fn fragment(session: &mut Session, follows_index: u64, text: &str) {
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
        text: text.into(),
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
    fragment(&mut session, 3, "Waiting");
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
    fragment(&mut session, 3, "Waiting");
    session.apply(Event::MessageFinalized {
        conversation: "room-a".into(),
        draft: "draft".into(),
        message: stored("Waiting", 7, Some(6)),
    });
    assert_eq!(texts(&session), ["Peer replied", "Waiting"]);
}

fn indexed_tool(session: &mut Session, index: u64) {
    let mut event = tool("run", ToolState::Running);
    if let Event::Item {
        item: Item::Tool { call, .. },
        ..
    } = &mut event
    {
        call.position = Some(RowPosition::stored(index, None));
    }
    session.apply(event);
}

#[test]
fn a_blank_draft_takes_its_next_segment_position_after_a_tool() {
    let mut session = session();
    started(&mut session, "run");
    fragment(&mut session, 2, " ");
    indexed_tool(&mut session, 3);
    fragment(&mut session, 3, "Waiting");
    assert!(
        matches!(session.snapshot().map(|s| s.items.as_slice()), Some([Item::Tool { .. }, Item::Message(m)]) if m.text == " Waiting")
    );
}

#[test]
fn an_evicted_draft_keeps_its_position_until_finalized() {
    let mut session = Session::new(crate::Limits {
        items: std::num::NonZeroUsize::MIN,
        ..Default::default()
    });
    session.apply(Event::Snapshot(snapshot()));
    started(&mut session, "run");
    publish(&mut session, stored("Peer replied", 4, None));
    fragment(&mut session, 3, "Waiting");
    assert_eq!(texts(&session), ["Peer replied"]);
    session.apply(Event::MessageFinalized {
        conversation: "room-a".into(),
        draft: "draft".into(),
        message: stored("Waiting", 5, Some(3)),
    });
    assert_eq!(texts(&session), ["Peer replied"]);
}

#[test]
fn a_positioned_call_follows_an_earlier_indexed_steered_message() {
    let mut session = session();
    started(&mut session, "run");
    publish(&mut session, stored("Steered", 4, None));
    // A call can be published while the engine is still handing the message over.
    indexed_tool(&mut session, 5);
    session.apply(Event::MessageSteered {
        conversation: "room-a".into(),
        message: "Steered".into(),
        run: "run".into(),
    });
    publish(&mut session, stored("Steered", 4, None));
    assert!(matches!(
        session.snapshot().map(|s| s.items.as_slice()),
        Some([Item::Message(_), Item::Tool { .. }])
    ));
}

#[test]
fn reasoning_stays_before_its_answer_when_a_peer_arrived_first() {
    let mut session = session();
    started(&mut session, "run");
    publish(&mut session, stored("Peer", 4, None));
    session.apply(Event::ThinkingDelta {
        conversation: "room-a".into(),
        actor: "agent".into(),
        run: "run".into(),
        text: "Considering".into(),
    });
    fragment(&mut session, 3, "Waiting");
    assert!(matches!(session.snapshot().map(|s| s.items.as_slice()),
        Some([Item::Thinking(_), Item::Message(answer), Item::Message(peer)]) if answer.text == "Waiting" && peer.text == "Peer"));
}

#[test]
fn reasoning_after_blank_positioned_text_still_precedes_the_first_word() {
    let mut session = session();
    started(&mut session, "run");
    publish(&mut session, stored("Peer", 4, None));
    fragment(&mut session, 3, " ");
    session.apply(Event::ThinkingDelta {
        conversation: "room-a".into(),
        actor: "agent".into(),
        run: "run".into(),
        text: "Considering".into(),
    });
    session.apply(Event::TextDelta {
        conversation: "room-a".into(),
        message: "draft".into(),
        actor: "agent".into(),
        run: "run".into(),
        text: "Waiting".into(),
    });
    assert!(matches!(session.snapshot().map(|s| s.items.as_slice()),
        Some([Item::Thinking(_), Item::Message(answer), Item::Message(peer)]) if answer.text == " Waiting" && peer.text == "Peer"));
}
