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
