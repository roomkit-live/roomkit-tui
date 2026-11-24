use super::*;
use roomkit_tui_protocol::InputAction;

fn action_session() -> (Session, Client, Host) {
    let (mut session, client, host) = fixture(true);
    let mut snapshot = session.snapshot.clone().unwrap();
    snapshot
        .input_actions
        .push(InputAction::new('@', "action", "Address").with_completion());
    session.apply(Event::Snapshot(snapshot));
    (session, client, host)
}

#[test]
fn typing_filters_once_and_enter_chooses_only_the_exact_address() {
    let (mut session, client, host) = action_session();
    press(
        &mut session,
        &client,
        key(KeyCode::Char('@')),
        Duration::ZERO,
    );
    assert!(session.menu().is_visible());
    assert!(composer::request_choices(&mut session, &client));
    let lookup = host.try_recv().unwrap();
    assert_eq!(
        lookup.command,
        Command::CompleteInput {
            name: "action".into()
        }
    );
    session.insert("RE");
    assert!(!composer::request_choices(&mut session, &client));
    session.apply(Event::InputChoices {
        conversation: "room".into(),
        request: lookup.id,
        choices: vec![
            Choice::new("id:1", "Review"),
            Choice::new("peer:2", "Review"),
            Choice::new("id:3", "Api"),
        ],
    });
    assert_eq!(session.menu().entries().len(), 2);
    press(&mut session, &client, key(KeyCode::Down), Duration::ZERO);
    press(&mut session, &client, key(KeyCode::Enter), Duration::ZERO);
    assert_eq!(
        host.try_recv().unwrap().command,
        Command::Invoke {
            name: "action".into(),
            arguments: "peer:2".into()
        }
    );
    assert!(session.draft().is_empty());
    assert!(host.try_recv().is_err());
}

#[test]
fn escaped_and_mid_sentence_prefixes_are_literal_and_late_choices_preserve_the_draft() {
    let (mut session, client, host) = action_session();
    for text in [r"\@review", "mail@host", "@someone hello"] {
        session.set_draft(text);
        assert!(!composer::request_choices(&mut session, &client));
        assert!(!session.menu().is_visible());
    }
    session.set_draft("@");
    assert!(composer::request_choices(&mut session, &client));
    let first = host.try_recv().unwrap();
    session.set_draft("");
    session.set_draft("@");
    assert!(composer::request_choices(&mut session, &client));
    let second = host.try_recv().unwrap();
    session.apply(Event::InputChoices {
        conversation: "room".into(),
        request: first.id,
        choices: vec![Choice::new("stale", "Stale")],
    });
    assert!(session.menu().entries().is_empty());
    press(&mut session, &client, escape(), Duration::ZERO);
    session.apply(Event::InputChoices {
        conversation: "room".into(),
        request: second.id,
        choices: vec![],
    });
    assert!(!session.menu().is_visible());
    assert_eq!(session.draft(), "@");
}

#[test]
fn complete_action_labels_remain_visible_on_an_eighty_column_terminal() {
    let (mut session, client, host) = action_session();
    session.set_draft("@");
    assert!(composer::request_choices(&mut session, &client));
    let label = "Choose workspace · project · secondary profile";
    session.apply(Event::InputChoices {
        conversation: "room".into(),
        request: host.try_recv().unwrap().id,
        choices: vec![Choice::new("id:2", label).with_description("Secondary")],
    });
    let frame = crate::render::screen(&session, 80, 24, &Default::default());
    assert!(frame.lines.iter().any(|line| line.contains(label)));
}
