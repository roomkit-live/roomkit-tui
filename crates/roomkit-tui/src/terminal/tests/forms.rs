//! Form cancellation notifies its host without leaking any draft fields.
use super::*;
use roomkit_tui_protocol::{FieldKind, FormField, FormSpec};

#[test]
fn cancelling_a_form_sends_only_its_scoped_cleanup_action() {
    let (mut session, client, host) = fixture(false);
    session.apply(Event::Form {
        conversation: "room".into(),
        form: FormSpec::new(
            "attempt",
            "action",
            "Connect",
            vec![FormField::new("code", "Code", FieldKind::Secret)],
        )
        .with_dismiss("cancel:attempt"),
    });
    session.form.as_mut().unwrap().insert("SECRET_CALLBACK");
    press(&mut session, &client, escape(), Duration::ZERO);
    assert!(session.form.is_none());
    let request = host.try_recv().unwrap();
    assert_eq!(request.conversation.as_str(), "room");
    assert!(
        matches!(request.command, Command::Invoke { name, arguments }
        if name == "action" && arguments == "cancel:attempt")
    );
}

#[test]
fn ordinary_forms_still_cancel_without_a_host_action() {
    let (mut session, client, host) = fixture(false);
    session.apply(Event::Form {
        conversation: "room".into(),
        form: FormSpec::new(
            "settings",
            "action",
            "Settings",
            vec![FormField::new("label", "Label", FieldKind::Text)],
        ),
    });
    press(&mut session, &client, escape(), Duration::ZERO);
    assert!(session.form.is_none());
    assert!(host.try_recv().is_err());
}

#[test]
fn accepted_async_form_can_cancel_while_the_host_operation_is_still_running() {
    let (mut session, client, host) = fixture(false);
    session.apply(Event::Form {
        conversation: "room".into(),
        form: FormSpec::new(
            "attempt",
            "action",
            "Connect",
            vec![FormField::new("code", "Code", FieldKind::Secret).required()],
        )
        .with_dismiss("cancel:attempt")
        .with_async_completion(),
    });
    session.form.as_mut().unwrap().insert("SECRET_CALLBACK");
    press(
        &mut session,
        &client,
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
        Duration::ZERO,
    );
    let submitted = host.try_recv().unwrap();
    session.apply(Event::Reply {
        request: submitted.id,
        error: None,
    });
    assert!(session.form_pending());
    // The host has acknowledged the code, but its network operation is held.
    press(
        &mut session,
        &client,
        key(KeyCode::Char('x')),
        Duration::ZERO,
    );
    assert_eq!(
        session.form.as_ref().unwrap().editors[0].text(),
        "SECRET_CALLBACK"
    );
    press(&mut session, &client, escape(), Duration::ZERO);
    let cancelled = host.try_recv().unwrap();
    assert!(
        matches!(cancelled.command, Command::Invoke { arguments, .. } if arguments == "cancel:attempt")
    );
    assert!(session.form.is_none());
    session.apply(Event::FormCompleted {
        conversation: "room".into(),
        form: "attempt".into(),
        panel: roomkit_tui_protocol::PanelSpec::new("result", "action", "Connected", "Done"),
    });
    assert!(
        session.panel.is_none(),
        "A late result must not reopen the cancelled form"
    );
}

#[test]
fn an_accepted_async_form_without_cleanup_still_dismisses_locally() {
    let (mut session, client, host) = fixture(false);
    session.apply(Event::Form {
        conversation: "room".into(),
        form: FormSpec::new(
            "attempt",
            "action",
            "Connect",
            vec![FormField::new("code", "Code", FieldKind::Secret)],
        )
        .with_async_completion(),
    });
    press(
        &mut session,
        &client,
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
        Duration::ZERO,
    );
    let submitted = host.try_recv().unwrap();
    session.apply(Event::Reply {
        request: submitted.id,
        error: None,
    });
    press(&mut session, &client, escape(), Duration::ZERO);
    assert!(session.form.is_none());
    assert!(host.try_recv().is_err());
}

fn drawn(session: &Session) -> String {
    let (options, renderers) = (UiOptions::default(), crate::render::Renderers::default());
    crate::render::screen_with_options(session, 120, 30, &renderers, &options, Duration::ZERO)
        .lines
        .join("\n")
}

/// A sign-in form holds one field, the address a person pastes. Enter, the
/// key its line named for "next field", moved to that same field: the paste
/// stayed there and nothing said why. With no next field, Enter saves.
#[test]
fn enter_saves_a_form_of_one_field_and_its_line_says_so() {
    let (mut session, client, host) = fixture(false);
    session.apply(Event::Form {
        conversation: "room".into(),
        form: FormSpec::new(
            "attempt",
            "action",
            "Connect",
            vec![FormField::new("code", "Code", FieldKind::Secret).required()],
        ),
    });
    assert!(drawn(&session).contains("Enter save · Esc cancel"));
    assert!(!drawn(&session).contains("next field"));
    // Required and empty: Enter says so, as Ctrl+S does, and sends nothing.
    press(&mut session, &client, key(KeyCode::Enter), Duration::ZERO);
    assert!(host.try_recv().is_err());
    assert!(session.notice().contains("Code"), "{}", session.notice());
    session.form.as_mut().unwrap().insert("PASTED_ADDRESS");
    press(&mut session, &client, key(KeyCode::Enter), Duration::ZERO);
    let request = host.try_recv().unwrap();
    assert!(
        matches!(request.command, Command::SubmitForm { form, values, .. }
        if form == "attempt" && values.get("code") == Some("PASTED_ADDRESS"))
    );
}

/// Alt+Enter stays the field's line break, and Enter the way to the next
/// field of a form that has one: only Ctrl+S saves it.
#[test]
fn enter_still_moves_between_the_fields_of_a_longer_form() {
    let (mut session, client, host) = fixture(false);
    let fields = |count: usize| {
        (0..count)
            .map(|at| {
                FormField::new(
                    format!("f{at}"),
                    format!("Field {at}"),
                    FieldKind::Multiline,
                )
            })
            .collect::<Vec<_>>()
    };
    session.apply(Event::Form {
        conversation: "room".into(),
        form: FormSpec::new("settings", "action", "Settings", fields(2)),
    });
    assert!(drawn(&session).contains("Ctrl+S save · Esc cancel · Enter next field"));
    press(&mut session, &client, key(KeyCode::Enter), Duration::ZERO);
    assert_eq!(session.form.as_ref().unwrap().selected, 1);
    assert!(host.try_recv().is_err());

    session.apply(Event::Form {
        conversation: "room".into(),
        form: FormSpec::new("note", "action", "Note", fields(1)),
    });
    session.form.as_mut().unwrap().insert("first");
    let alt_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);
    press(&mut session, &client, alt_enter, Duration::ZERO);
    assert!(host.try_recv().is_err());
    assert_eq!(session.form.as_ref().unwrap().editors[0].text(), "first\n");
}
