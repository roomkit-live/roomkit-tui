//! Confirming a choice stays visible until its host request settles.
use super::*;
use roomkit_tui_protocol::Choice;

fn show(session: &mut Session, title: &str) {
    session.apply(Event::Choices {
        conversation: "room".into(),
        command: "action".into(),
        title: title.into(),
        choices: vec![Choice::new("entry", "Example repository")],
    });
}

#[test]
fn a_slow_picker_request_is_visible_sent_once_and_retryable() {
    let (mut session, client, host) = fixture(false);
    show(&mut session, "Repositories");
    press(&mut session, &client, key(KeyCode::Enter), Duration::ZERO);
    let first = host.try_recv().unwrap();
    assert!(
        crate::render::screen(&session, 80, 24, &crate::render::Renderers::default())
            .lines
            .iter()
            .any(|line| line.contains("Waiting for response"))
    );
    press(&mut session, &client, key(KeyCode::Enter), Duration::ZERO);
    assert!(host.try_recv().is_err());
    session.apply(Event::Reply {
        request: first.id,
        error: Some("Repository unavailable".into()),
    });
    assert!(
        crate::render::screen(&session, 80, 24, &crate::render::Renderers::default())
            .lines
            .iter()
            .any(|line| line.contains("Repository unavailable"))
    );
    press(&mut session, &client, key(KeyCode::Enter), Duration::ZERO);
    let retry = host.try_recv().unwrap();
    // A new catalogue may arrive before the acknowledgement for the previous one.
    show(&mut session, "Files");
    session.apply(Event::Reply {
        request: retry.id,
        error: None,
    });
    assert_eq!(session.picker().unwrap().title(), "Files");
}
