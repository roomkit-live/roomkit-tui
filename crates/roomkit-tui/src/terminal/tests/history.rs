//! Composer history navigation follows inputs, including multiline and commands.
use super::*;

fn submit_text(session: &mut Session, client: &Client, host: &Host, text: &str) {
    session.insert(text);
    press(session, client, key(KeyCode::Enter), Duration::ZERO);
    let request = host.try_recv().unwrap();
    session.apply(Event::Reply {
        request: request.id,
        error: None,
    });
    assert!(session.draft().is_empty());
}

#[test]
fn arrows_browse_whole_multiline_history_entries_in_both_directions() {
    let (mut session, client, host) = fixture(false);
    submit_text(&mut session, &client, &host, "first entry");
    submit_text(
        &mut session,
        &client,
        &host,
        "second entry\nwith another line",
    );
    for (code, expected) in [
        (KeyCode::Up, "second entry\nwith another line"),
        (KeyCode::Up, "first entry"),
        (KeyCode::Up, "first entry"),
        (KeyCode::Down, "second entry\nwith another line"),
        (KeyCode::Down, ""),
        (KeyCode::Down, ""),
    ] {
        press(&mut session, &client, key(code), Duration::ZERO);
        assert_eq!(session.draft(), expected);
    }
    assert!(host.try_recv().is_err());
}

#[test]
fn down_returns_from_a_multiline_history_entry_to_the_unfinished_draft() {
    let (mut session, client, host) = fixture(false);
    submit_text(&mut session, &client, &host, "previous\nmessage");
    session.insert("unfinished draft");
    press(&mut session, &client, key(KeyCode::Up), Duration::ZERO);
    press(&mut session, &client, key(KeyCode::Down), Duration::ZERO);
    assert_eq!(session.draft(), "unfinished draft");
}

#[test]
fn recalled_commands_do_not_capture_history_arrows_in_the_completion_menu() {
    let (mut session, client, host) = fixture(false);
    submit_text(&mut session, &client, &host, "older input");
    submit_text(&mut session, &client, &host, "/action");
    session.insert("draft");
    press(&mut session, &client, key(KeyCode::Up), Duration::ZERO);
    assert_eq!(session.draft(), "/action");
    assert!(!session.menu.visible);
    press(&mut session, &client, key(KeyCode::Up), Duration::ZERO);
    assert_eq!(session.draft(), "older input");
    press(&mut session, &client, key(KeyCode::Down), Duration::ZERO);
    press(&mut session, &client, key(KeyCode::Down), Duration::ZERO);
    assert_eq!(session.draft(), "draft");
}

#[test]
fn multiline_drafts_keep_line_navigation_and_restore_the_caret_after_history() {
    let (mut session, client, host) = fixture(false);
    submit_text(&mut session, &client, &host, "older input");
    let draft = "界e\u{301}\nabc\n👩‍💻z";
    session.insert(draft);
    for _ in 0..2 {
        press(&mut session, &client, key(KeyCode::Up), Duration::ZERO);
        assert_eq!(session.draft(), draft);
    }
    let cursor = session.editor.cursor();
    press(&mut session, &client, key(KeyCode::Up), Duration::ZERO);
    assert_eq!(session.draft(), "older input");
    press(&mut session, &client, key(KeyCode::Down), Duration::ZERO);
    assert_eq!(session.draft(), draft);
    assert_eq!(session.editor.cursor(), cursor);
    press(&mut session, &client, key(KeyCode::Down), Duration::ZERO);
    assert!(session.editor.cursor() > cursor);
}

#[test]
fn editing_a_recalled_multiline_entry_restores_line_navigation() {
    let (mut session, client, host) = fixture(false);
    submit_text(&mut session, &client, &host, "older input");
    submit_text(&mut session, &client, &host, "first line\nsecond line");
    press(&mut session, &client, key(KeyCode::Up), Duration::ZERO);
    press(
        &mut session,
        &client,
        key(KeyCode::Char('!')),
        Duration::ZERO,
    );
    press(&mut session, &client, key(KeyCode::Up), Duration::ZERO);
    assert_eq!(session.draft(), "first line\nsecond line!");
    assert_eq!(session.editor.cursor(), "first line".len());
    press(&mut session, &client, key(KeyCode::Down), Duration::ZERO);
    assert_eq!(session.draft(), "first line\nsecond line!");
}

#[test]
fn tab_can_complete_a_recalled_command_and_control_keys_still_browse_history() {
    let (mut session, client, host) = fixture(false);
    submit_text(&mut session, &client, &host, "/action");
    session.insert("draft\nwith two lines");
    press(
        &mut session,
        &client,
        KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
        Duration::ZERO,
    );
    assert_eq!(session.draft(), "/action");
    assert!(!session.menu.visible);
    press(
        &mut session,
        &client,
        KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
        Duration::ZERO,
    );
    assert_eq!(session.draft(), "draft\nwith two lines");
    press(
        &mut session,
        &client,
        KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
        Duration::ZERO,
    );
    press(&mut session, &client, key(KeyCode::Tab), Duration::ZERO);
    assert_eq!(session.draft(), "/action ");
    assert!(!session.editor.is_browsing_history());
    assert!(host.try_recv().is_err());
}
