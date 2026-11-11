//! Input recall is restored from saved messages with their original intent.
use super::*;

fn restored(room: &str) -> Snapshot {
    let mut snapshot = Snapshot::new(room, "Resumed", Actor::new("agent", "Agent"));
    snapshot.local_actor = Some("person:local".into());
    snapshot
        .commands
        .push(CommandSpec::new("action", "Example"));
    snapshot.items = vec![
        Item::Message(roomkit_tui_protocol::Message::new(
            "old",
            "person:local",
            "saved first input",
        )),
        Item::Message(roomkit_tui_protocol::Message::new(
            "answer",
            "agent",
            "assistant answer",
        )),
        Item::Message(roomkit_tui_protocol::Message::new(
            "someone",
            "person:other",
            "another person's input",
        )),
        Item::Message(roomkit_tui_protocol::Message::new(
            "recent",
            "person:local",
            "saved second\ninput",
        )),
    ];
    snapshot
}

#[test]
fn resumed_conversation_recalls_saved_local_messages_instead_of_resume_command() {
    let (mut session, client, _host) = fixture(false);
    session.editor.set("/resume".into());
    session.editor.submitted();
    session.apply(Event::Snapshot(restored("resumed-room")));
    session.insert("unfinished draft");
    for (code, expected) in [
        (KeyCode::Up, "saved second\ninput"),
        (KeyCode::Up, "saved first input"),
        (KeyCode::Up, "saved first input"),
        (KeyCode::Down, "saved second\ninput"),
        (KeyCode::Down, "unfinished draft"),
    ] {
        press(&mut session, &client, key(code), Duration::ZERO);
        assert_eq!(session.draft(), expected);
    }
}

#[test]
fn first_snapshot_seeds_input_history_and_same_room_refresh_keeps_new_inputs() {
    let (mut session, client, host) = fixture(false);
    session.apply(Event::Snapshot(restored("room")));
    press(&mut session, &client, key(KeyCode::Up), Duration::ZERO);
    assert_eq!(session.draft(), "saved second\ninput");
    press(&mut session, &client, key(KeyCode::Down), Duration::ZERO);
    session.insert("/action");
    press(&mut session, &client, key(KeyCode::Enter), Duration::ZERO);
    let request = host.try_recv().unwrap();
    session.apply(Event::Reply {
        request: request.id,
        error: None,
    });
    session.apply(Event::Snapshot(restored("room")));
    press(&mut session, &client, key(KeyCode::Up), Duration::ZERO);
    assert_eq!(session.draft(), "/action");
}

#[test]
fn restored_prefixes_are_resubmitted_as_text_and_typed_backslashes_are_unchanged() {
    use roomkit_tui_protocol::{InputAction, Message};
    for text in [
        "/action",
        "  /action value",
        "!echo example",
        "\\!literal",
        "\\\\server\\share",
    ] {
        let (mut session, client, host) = fixture(false);
        let mut snapshot = restored("resumed-room");
        snapshot
            .input_actions
            .push(InputAction::new('!', "action", "Run action"));
        snapshot.items = vec![Item::Message(Message::new("message", "person:local", text))];
        session.apply(Event::Snapshot(snapshot));
        press(&mut session, &client, key(KeyCode::Up), Duration::ZERO);
        press(&mut session, &client, key(KeyCode::Enter), Duration::ZERO);
        let request = host.try_recv().unwrap();
        assert!(
            matches!(&request.command, Command::Submit { text: submitted, .. } if submitted == text)
        );
        session.apply(Event::Reply {
            request: request.id,
            error: None,
        });
        // The intent survives a second recall after submitting the restored input.
        press(&mut session, &client, key(KeyCode::Up), Duration::ZERO);
        assert!(
            matches!(session.input_command().unwrap(), Command::Submit { text: submitted, .. } if submitted == text)
        );
    }
    let (mut session, _, _) = fixture(false);
    session.insert("\\\\server\\share");
    assert!(
        matches!(session.input_command().unwrap(), Command::Submit { text, .. } if text == "\\\\server\\share")
    );
}

#[test]
fn switching_conversations_discards_recalled_text_but_keeps_the_unsent_draft() {
    let (mut session, client, _) = fixture(false);
    session.apply(Event::Snapshot(restored("first-room")));
    session.insert("keep my draft");
    press(&mut session, &client, key(KeyCode::Up), Duration::ZERO);
    let mut next = restored("second-room");
    next.items.clear();
    session.apply(Event::Snapshot(next));
    assert_eq!(session.draft(), "keep my draft");
    press(&mut session, &client, key(KeyCode::Up), Duration::ZERO);
    assert_eq!(session.draft(), "keep my draft");
}

#[test]
fn restored_literal_message_with_an_image_keeps_its_text_and_clears_on_acceptance() {
    use roomkit_tui_protocol::{ImageUpload, Message};
    let (mut session, client, host) = fixture(false);
    let mut snapshot = restored("room");
    snapshot.can_attach_images = true;
    snapshot.items = vec![Item::Message(Message::new(
        "message",
        "person:local",
        "/action",
    ))];
    session.apply(Event::Snapshot(snapshot));
    press(&mut session, &client, key(KeyCode::Up), Duration::ZERO);
    session
        .add_image(ImageUpload::png("example.png", "AA==", 1, 1))
        .unwrap();
    press(&mut session, &client, key(KeyCode::Enter), Duration::ZERO);
    let request = host.try_recv().unwrap();
    assert!(
        matches!(&request.command, Command::SubmitImages { message, .. } if message.text == "/action")
    );
    session.apply(Event::Reply {
        request: request.id,
        error: None,
    });
    assert!(session.draft().is_empty() && session.images().is_empty());
}

#[test]
fn erasing_a_restored_message_starts_a_regular_new_input() {
    use roomkit_tui_protocol::InputAction;
    for delete in [KeyCode::Backspace, KeyCode::Delete] {
        let (mut session, client, _) = fixture(false);
        let mut snapshot = restored("room");
        snapshot
            .input_actions
            .push(InputAction::new('!', "action", "Run action"));
        session.apply(Event::Snapshot(snapshot));
        press(&mut session, &client, key(KeyCode::Up), Duration::ZERO);
        if delete == KeyCode::Delete {
            while session.editor.cursor() > 0 {
                session.editor.left();
            }
        }
        while !session.draft().is_empty() {
            press(&mut session, &client, key(delete), Duration::ZERO);
        }
        session.insert("!new action");
        assert!(
            matches!(session.input_command().unwrap(), Command::Invoke { arguments, .. } if arguments == "new action")
        );
    }
}

#[test]
fn a_newly_advertised_action_does_not_change_a_restored_messages_intent() {
    use roomkit_tui_protocol::{InputAction, Message};
    let (mut session, client, _) = fixture(false);
    let mut snapshot = restored("room");
    snapshot.items = vec![Item::Message(Message::new(
        "message",
        "person:local",
        "!literal message",
    ))];
    session.apply(Event::Snapshot(snapshot.clone()));
    snapshot
        .input_actions
        .push(InputAction::new('!', "action", "Run action"));
    session.apply(Event::Snapshot(snapshot));
    press(&mut session, &client, key(KeyCode::Up), Duration::ZERO);
    press(&mut session, &client, key(KeyCode::Tab), Duration::ZERO);
    assert!(
        matches!(session.input_command().unwrap(), Command::Submit { text, .. } if text == "!literal message")
    );
}

#[test]
fn send_now_treats_a_restored_command_shaped_message_as_a_message() {
    use roomkit_tui_protocol::Message;
    let (mut session, client, host) = fixture(false);
    let mut snapshot = restored("room");
    snapshot.active_run = Some("active".into());
    snapshot.can_interrupt = true;
    snapshot.items = vec![Item::Message(Message::new(
        "message",
        "person:local",
        "/action",
    ))];
    session.apply(Event::Snapshot(snapshot));
    press(&mut session, &client, key(KeyCode::Up), Duration::ZERO);
    press(
        &mut session,
        &client,
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
        Duration::ZERO,
    );
    let request = host.try_recv().unwrap();
    assert!(matches!(request.command, Command::Interrupt { run } if run.as_str() == "active"));
    assert!(session.sends_after_interruption());
}

fn recalled_message_with_identical_command() -> (Session, Client, Host) {
    use roomkit_tui_protocol::Message;
    let (mut session, client, host) = fixture(false);
    let mut snapshot = restored("room");
    snapshot.items = vec![Item::Message(Message::new(
        "message",
        "person:local",
        "/action",
    ))];
    session.apply(Event::Snapshot(snapshot));
    session.insert("/action");
    press(&mut session, &client, key(KeyCode::Enter), Duration::ZERO);
    let command = host.try_recv().unwrap();
    assert!(matches!(command.command, Command::Invoke { .. }));
    session.apply(Event::Reply {
        request: command.id,
        error: None,
    });
    press(&mut session, &client, key(KeyCode::Up), Duration::ZERO);
    press(&mut session, &client, key(KeyCode::Up), Duration::ZERO);
    session.send_after_interruption().unwrap();
    (session, client, host)
}

#[test]
fn recalling_an_identical_command_cancels_the_waited_messages_send() {
    let (mut session, client, host) = recalled_message_with_identical_command();
    assert!(session.sends_after_interruption());
    press(&mut session, &client, key(KeyCode::Down), Duration::ZERO);
    assert!(!session.sends_after_interruption());
    assert!(!composer::send_waiting(
        &mut session,
        &client,
        Duration::ZERO
    ));
    assert!(host.try_recv().is_err());
}

#[test]
fn accepting_a_waited_message_preserves_an_identical_recalled_command() {
    let (mut session, client, host) = recalled_message_with_identical_command();
    assert!(composer::send_waiting(
        &mut session,
        &client,
        Duration::ZERO
    ));
    let sent = host.try_recv().unwrap();
    assert!(matches!(sent.command, Command::Submit { .. }));
    press(&mut session, &client, key(KeyCode::Down), Duration::ZERO);
    session.apply(Event::Reply {
        request: sent.id,
        error: None,
    });
    assert_eq!(session.draft(), "/action");
    assert!(matches!(
        session.input_command().unwrap(),
        Command::Invoke { .. }
    ));
}
