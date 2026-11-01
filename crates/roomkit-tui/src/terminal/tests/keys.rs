#![allow(clippy::unwrap_used, reason = "Test fixtures fail on invalid state")]
use super::*;

#[test]
fn activity_details_toggle_with_f2_and_reject_stale_mouse_targets() {
    let (mut session, client, _host) = fixture(false);
    session.insert("Unsent draft");
    let tool = |id: &str, name: &str| Event::Item {
        conversation: "room".into(),
        item: Item::Tool {
            actor: "agent".into(),
            run: "run".into(),
            call: ToolCall::new(id, name, serde_json::json!({}), ToolState::Succeeded)
                .with_intent("Looking up a record")
                .with_text("result"),
        },
    };
    session.apply(tool("call", "lookup"));
    session.apply(Event::Item {
        conversation: "room".into(),
        item: Item::Message(roomkit_tui_protocol::Message::new(
            "middle",
            "agent",
            "A new sequence",
        )),
    });
    for index in 0..20 {
        session.apply(tool(&format!("later-{index}"), "another_action"));
    }
    for expected in [true, false] {
        handle_key(
            &mut session,
            &client,
            key(KeyCode::F(2)),
            Duration::ZERO,
            true,
        );
        assert_eq!(session.details_expanded(), expected);
    }
    session.apply(Event::RunStarted {
        conversation: "room".into(),
        run: "new-run".into(),
    });
    session.apply(Event::TextDelta {
        conversation: "room".into(),
        message: "latest".into(),
        actor: "agent".into(),
        run: "new-run".into(),
        text: format!("{}RESPONSE_END", "A streamed line\n".repeat(40)),
    });
    let options = UiOptions {
        tool_activity: true,
        ..UiOptions::default()
    };
    let frame = render::screen_with_options(
        &session,
        90,
        20,
        &Renderers::default(),
        &options,
        Duration::ZERO,
    );
    assert!(
        frame.lines.join("\n").contains("RESPONSE_END"),
        "Closing details resumes following new messages: {:?}",
        frame.lines
    );
    let mut target = render::ToolTarget {
        conversation: "room".into(),
        row: 0,
        start: 0,
        end: 10,
        actor: "agent".into(),
        run: "run".into(),
        call: "call".into(),
    };
    session.toggle_activity_target(&target);
    assert!(session.details_expanded());
    let frame = render::screen_with_options(
        &session,
        90,
        20,
        &Renderers::default(),
        &options,
        Duration::ZERO,
    );
    assert!(
        frame.lines.join("\n").contains("lookup"),
        "The selected old call stays in view"
    );
    target.run = "another-run".into();
    session.toggle_activity_target(&target);
    assert!(session.details_expanded());
    target.run = "run".into();
    target.conversation = "another-room".into();
    session.toggle_activity_target(&target);
    assert!(session.details_expanded());
    assert_eq!(session.draft(), "Unsent draft");
}

#[test]
fn permission_choices_capture_keys_and_never_submit_the_preserved_draft() {
    let (mut session, client, host) = fixture(true);
    request_permission(&mut session, "p", Duration::ZERO);
    session.insert("Unsent draft");
    let armed = APPROVAL_ARMING_DELAY;
    for code in [
        KeyCode::Char('x'),
        KeyCode::Down,
        KeyCode::Enter,
        KeyCode::Enter,
    ] {
        press(&mut session, &client, key(code), armed);
    }
    assert!(matches!(
        host.try_recv().unwrap().command,
        Command::Decide {
            decision: Decision::Deny,
            ..
        }
    ));
    assert!(
        host.try_recv().is_err(),
        "No duplicate decision or submitted draft"
    );
    assert_eq!(session.draft(), "Unsent draft");
    press(&mut session, &client, escape(), armed);
    assert!(matches!(
        host.try_recv().unwrap().command,
        Command::Interrupt { .. }
    ));
}

use crate::session::APPROVAL_ARMING_DELAY;

#[test]
fn a_new_approval_card_ignores_decisions_until_it_could_be_read() {
    let (mut session, client, host) = fixture(true);
    session.insert("typing a message");
    let shown = Duration::from_secs(10);
    request_permission(&mut session, "p", shown);
    press(
        &mut session,
        &client,
        key(KeyCode::Enter),
        shown + Duration::from_millis(10),
    );
    let ctrl_y = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL);
    press(
        &mut session,
        &client,
        ctrl_y,
        shown + Duration::from_millis(100),
    );
    assert!(host.try_recv().is_err(), "A decision was accepted unseen");
    assert_eq!(session.draft(), "typing a message");
    press(&mut session, &client, ctrl_y, shown + APPROVAL_ARMING_DELAY);
    assert!(matches!(
        host.try_recv().unwrap().command,
        Command::Decide {
            decision: Decision::Allow,
            ..
        }
    ));
    // The next card in line is disarmed again when it replaces the first.
    request_permission(&mut session, "q", shown + Duration::from_secs(1));
    let next = shown + Duration::from_secs(2);
    session.apply_at(
        Event::PermissionResolved {
            conversation: "room".into(),
            permission: "p".into(),
        },
        next,
    );
    press(
        &mut session,
        &client,
        key(KeyCode::Enter),
        next + Duration::from_millis(50),
    );
    assert!(
        host.try_recv().is_err(),
        "The second card was answered unseen"
    );
}

#[test]
fn shift_tab_changes_the_mode_only_from_the_composer() {
    let (mut session, client, host) = fixture(true);
    session.snapshot.as_mut().unwrap().mode = Some(ModeState::new(
        "action",
        "ask",
        vec![Choice::new("ask", "Ask"), Choice::new("auto", "Auto")],
    ));
    let back_tab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
    request_permission(&mut session, "p", Duration::ZERO);
    press(&mut session, &client, back_tab, Duration::from_secs(5));
    assert!(
        host.try_recv().is_err(),
        "The mode changed during a decision"
    );
    assert_eq!(
        session.permission_choice, 1,
        "Shift+Tab moves between choices"
    );
    session.apply(Event::PermissionResolved {
        conversation: "room".into(),
        permission: "p".into(),
    });
    session
        .snapshot
        .as_mut()
        .unwrap()
        .commands
        .push(CommandSpec::new("another", "Another"));
    session.insert("/");
    assert!(session.menu().is_visible());
    let last = session.menu().entries().len() - 1;
    press(&mut session, &client, back_tab, Duration::from_secs(5));
    assert_eq!(
        session.menu().selected(),
        last,
        "Shift+Tab moves up the menu"
    );
    assert!(host.try_recv().is_err(), "The mode changed from the menu");
    session.dismiss_completions();
    press(&mut session, &client, back_tab, Duration::from_secs(5));
    assert!(matches!(
        host.try_recv().unwrap().command,
        Command::Invoke { name, arguments } if name == "action" && arguments == "auto"
    ));
}

#[test]
fn image_removal_and_submission_keep_the_draft_until_host_acknowledgment() {
    let (mut session, client, host) = fixture(false);
    session.snapshot.as_mut().unwrap().can_attach_images = true;
    let image = roomkit_tui_protocol::ImageUpload::png("clipboard.png", "AQIDBA==", 1, 1);
    session.add_image(image.clone()).unwrap();
    press(
        &mut session,
        &client,
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT),
        Duration::ZERO,
    );
    assert!(session.images().is_empty());
    session.add_image(image.clone()).unwrap();
    session.insert("Describe");
    press(&mut session, &client, key(KeyCode::Enter), Duration::ZERO);
    let sent = host.try_recv().unwrap();
    assert!(matches!(sent.command, Command::SubmitImages { .. }));
    assert_eq!(session.draft(), "Describe");
    assert_eq!(session.images().len(), 1);
    press(
        &mut session,
        &client,
        key(KeyCode::Char('c')),
        Duration::ZERO,
    );
    assert_eq!(session.draft(), "Describe");
    session.apply(Event::Reply {
        request: sent.id,
        error: None,
    });
    assert!(session.images().is_empty());
    session.add_image(image).unwrap();
    press(&mut session, &client, ctrl_c(), Duration::ZERO);
    assert!(session.images().is_empty());
}

#[test]
fn sidebar_keyboard_navigation_never_submits_or_edits_the_composer() {
    use roomkit_tui_protocol::{
        SidebarAction, SidebarContent, SidebarEntry, SidebarSection, SidebarSpec, SidebarTone,
    };
    let (mut session, client, host) = fixture(false);
    session.insert("Keep this draft");
    let entries = (0..2)
        .map(|i| {
            SidebarEntry::new(i.to_string())
                .with_tone(SidebarTone::Accent)
                .with_action(SidebarAction::new("action", i.to_string()))
        })
        .collect();
    session.apply(Event::SidebarChanged {
        conversation: "room".into(),
        sidebar: Some(SidebarSpec::new(
            "Details",
            vec![SidebarSection::new(
                "Records",
                SidebarContent::Entries { entries },
            )],
        )),
    });
    session.sidebar.as_mut().unwrap().focused = true;
    for code in [KeyCode::Char('x'), KeyCode::Down, KeyCode::Enter] {
        press(&mut session, &client, key(code), Duration::ZERO);
    }
    let request = host.try_recv().unwrap();
    assert!(
        matches!(request.command, Command::Invoke { name, arguments } if name == "action" && arguments == "1")
    );
    assert_eq!(session.draft(), "Keep this draft");
    assert!(!session.sidebar().unwrap().is_focused());
    session.sidebar.as_mut().unwrap().focused = true;
    press(&mut session, &client, escape(), Duration::ZERO);
    assert!(!session.sidebar().unwrap().is_focused());
    assert!(host.try_recv().is_err());
}

#[test]
fn double_ctrl_c_clears_multiline_drafts_without_submitting_or_losing_history() {
    let (mut session, client, host) = fixture(false);
    session.insert("previous message");
    session.editor.submitted();
    session.insert("unfinished\nsecond line");
    session.editor.history_previous();
    assert!(!press(&mut session, &client, ctrl_c(), Duration::ZERO));
    assert!(session.draft().is_empty());
    session.editor.history_next();
    assert!(session.draft().is_empty(), "Discarded draft resurfaced");
    session.editor.history_previous();
    assert_eq!(session.draft(), "previous message");
    session.editor.history_next();
    assert!(host.try_recv().is_err());
    assert!(press(
        &mut session,
        &client,
        ctrl_c(),
        Duration::from_secs(1)
    ));
}

#[test]
fn quit_hint_expires_once_and_typing_disarms_exit() {
    let (mut session, client, _host) = fixture(false);
    assert!(!press(&mut session, &client, ctrl_c(), Duration::ZERO));
    assert!(session.expire_quit_hint(QUIT_TIMEOUT));
    assert!(!session.expire_quit_hint(QUIT_TIMEOUT));
    assert!(!press(&mut session, &client, ctrl_c(), QUIT_TIMEOUT));
    assert!(!press(
        &mut session,
        &client,
        key(KeyCode::Char('x')),
        QUIT_TIMEOUT
    ));
    assert!(!press(&mut session, &client, ctrl_c(), QUIT_TIMEOUT));
    let mut repeat = ctrl_c();
    repeat.kind = KeyEventKind::Repeat;
    assert!(!press(&mut session, &client, repeat, QUIT_TIMEOUT));
    assert!(press(&mut session, &client, ctrl_c(), QUIT_TIMEOUT));
}

#[test]
fn escape_closes_completion_before_interrupting_and_preserves_the_draft() {
    let (mut session, client, host) = fixture(true);
    session.insert("/act");
    assert!(session.menu().is_visible());
    assert!(!press(&mut session, &client, escape(), Duration::ZERO));
    assert!(!session.menu().is_visible());
    assert!(host.try_recv().is_err());
    assert!(!press(&mut session, &client, escape(), Duration::ZERO));
    assert_eq!(
        host.try_recv().unwrap().command,
        Command::Interrupt { run: "run".into() }
    );
    assert_eq!(session.draft(), "/act");
    assert!(session.interrupting());
    assert!(!press(&mut session, &client, escape(), Duration::ZERO));
    assert!(host.try_recv().is_err(), "Repeated interruption was sent");
    assert!(
        session.active_run().is_some(),
        "Input must not claim engine completion"
    );
    session.apply(Event::RunFinished {
        conversation: "room".into(),
        run: "run".into(),
        outcome: RunOutcome::Interrupted,
    });
    assert!(!session.interrupting());
    assert!(!press(&mut session, &client, escape(), Duration::ZERO));
    assert_eq!(session.draft(), "/act");
}

#[test]
fn ctrl_c_interrupts_once_and_second_press_can_leave_while_stopping() {
    let (mut session, client, host) = fixture(true);
    session.insert("draft");
    assert!(!press(&mut session, &client, ctrl_c(), Duration::ZERO));
    assert!(session.draft().is_empty());
    assert!(matches!(
        host.try_recv().unwrap().command,
        Command::Interrupt { .. }
    ));
    assert!(press(
        &mut session,
        &client,
        ctrl_c(),
        Duration::from_secs(1)
    ));
    assert!(host.try_recv().is_err());
}

#[test]
fn escape_and_ctrl_c_close_a_host_picker_without_cancelling_or_arming_exit() {
    let (mut session, client, host) = fixture(true);
    session.insert("draft");
    for event in [escape(), ctrl_c()] {
        session.picker = Some(crate::ChoicePicker::new(
            "action".into(),
            "Choices".into(),
            vec![],
        ));
        assert!(!press(&mut session, &client, event, Duration::ZERO));
        assert!(session.picker().is_none());
        assert!(session.quit_until.is_none());
        assert_eq!(session.draft(), "draft");
        assert!(host.try_recv().is_err());
    }
}

/// Review: the arming delay counted from the event's arrival, so a card
/// hidden behind a mouse selection or an undersized screen could be answered
/// as soon as it appeared.
#[test]
fn a_card_is_armed_only_once_a_frame_has_shown_it() {
    let (mut session, client, host) = fixture(true);
    session.apply_at(
        Event::PermissionRequested {
            conversation: "room".into(),
            permission: permission("p"),
        },
        Duration::ZERO,
    );
    let later = Duration::from_secs(5);
    press(&mut session, &client, key(KeyCode::Enter), later);
    assert!(host.try_recv().is_err(), "A card never drawn was answered");
    draw(&mut session, later);
    press(
        &mut session,
        &client,
        key(KeyCode::Enter),
        later + Duration::from_millis(200),
    );
    assert!(host.try_recv().is_err(), "Answered before it could be read");
    press(
        &mut session,
        &client,
        key(KeyCode::Enter),
        later + APPROVAL_ARMING_DELAY,
    );
    assert!(matches!(
        host.try_recv().unwrap().command,
        Command::Decide { .. }
    ));
}

/// RMK-1135 review: requests of every live run are shown, but Esc on a card
/// interrupted the earliest run, the view's own work, not the run that asked.
#[test]
fn escape_on_an_approval_card_interrupts_the_run_that_asked() {
    let (mut session, client, host) = fixture(true);
    session.apply(Event::RunStarted {
        conversation: "room".into(),
        run: "sms".into(),
    });
    session.apply(Event::PermissionRequested {
        conversation: "room".into(),
        permission: Permission::new("send", "agent", "sms", "Send SMS?", "To Carol"),
    });
    draw(&mut session, Duration::ZERO);
    assert!(!press(&mut session, &client, escape(), Duration::ZERO));
    assert_eq!(
        host.try_recv().unwrap().command,
        Command::Interrupt { run: "sms".into() }
    );
}
