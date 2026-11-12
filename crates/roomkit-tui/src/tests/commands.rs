#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]

use roomkit_tui_protocol::*;
use unicode_width::UnicodeWidthStr;

use super::*;
use crate::Session;
use crate::render::{self, Renderers};

#[test]
fn host_modes_cycle_without_optimistic_policy_changes_or_losing_the_draft() {
    let mut state = snapshot();
    state
        .commands
        .push(CommandSpec::new("policy", "Host policy"));
    let mut mode = ModeState::new(
        "policy",
        "observe",
        vec![
            Choice::new("observe", "Observe"),
            Choice::new("assist", "Assist"),
            Choice::new("act", "Act"),
        ],
    );
    state.mode = Some(mode.clone());
    let mut session = Session::default();
    session.apply(Event::Snapshot(state));
    session.set_draft("Unsent draft");
    started(&mut session, "run-1");
    let command = session.cycle_mode().unwrap();
    assert_eq!(
        command,
        Command::Invoke {
            name: "policy".into(),
            arguments: "assist".into()
        }
    );
    let request = session.request(command).unwrap();
    session.sent(&request);
    assert!(
        session.cycle_mode().is_none(),
        "Repeated keys wait for confirmation"
    );
    assert_eq!(
        session
            .snapshot
            .as_ref()
            .unwrap()
            .mode
            .as_ref()
            .unwrap()
            .selected,
        "observe"
    );
    mode.selected = "assist".into();
    session.apply(Event::ModeChanged {
        conversation: "room-b".into(),
        mode: mode.clone(),
    });
    assert_eq!(
        session
            .snapshot
            .as_ref()
            .unwrap()
            .mode
            .as_ref()
            .unwrap()
            .selected,
        "observe"
    );
    session.apply(Event::Reply {
        request: request.id,
        error: None,
    });
    session.apply(Event::ModeChanged {
        conversation: "room-a".into(),
        mode,
    });
    assert_eq!(
        session.cycle_mode().unwrap(),
        Command::Invoke {
            name: "policy".into(),
            arguments: "act".into()
        }
    );
    assert_eq!(session.draft(), "Unsent draft");
    assert_eq!(session.active_run(), Some(&RunId::from("run-1")));
    assert!(
        render::screen(&session, 100, 30, &Renderers::default())
            .lines
            .iter()
            .any(|line| line.contains("[Assist]") && !line.contains("⇧Tab"))
    );
}

#[test]
fn host_selectors_are_scoped_filterable_and_preserve_the_conversation_draft() {
    let mut session = session();
    session.insert("Draft to keep");
    let choices = vec![Choice::new("id-1", "Earlier record").with_description("Yesterday")];
    session.apply(Event::Choices {
        conversation: "other-room".into(),
        command: "summarize".into(),
        title: "Records".into(),
        choices: choices.clone(),
    });
    assert!(session.picker().is_none());
    session.apply(Event::Choices {
        conversation: "room-a".into(),
        command: "unknown".into(),
        title: "Records".into(),
        choices: choices.clone(),
    });
    assert!(session.picker().is_none());
    session.apply(Event::Choices {
        conversation: "room-a".into(),
        command: "summarize".into(),
        title: "Records".into(),
        choices,
    });
    assert!(session.picker().is_some());
    for width in [12, 31, 32, 60, 100] {
        for height in [8, 14, 16, 20, 24] {
            let frame = render::screen(&session, width, height, &Renderers::default());
            assert!(frame.lines.len() <= usize::from(height));
            assert!(
                frame
                    .lines
                    .iter()
                    .all(|line| line.width() <= usize::from(width))
            );
            if let Some((x, y)) = frame.cursor {
                assert!(x < width && y < height);
                assert!(frame.lines[usize::from(y)].contains("Filter:"));
                assert!(
                    frame
                        .lines
                        .iter()
                        .any(|line| line.contains("Earlier record"))
                );
            }
        }
    }
    let request = session
        .request(Command::Invoke {
            name: "summarize".into(),
            arguments: "id-1".into(),
        })
        .unwrap();
    session.sent(&request);
    session.apply(Event::Reply {
        request: request.id,
        error: Some("No longer available".into()),
    });
    assert!(session.picker().is_some());
    let request = session.request(request.command).unwrap();
    session.sent(&request);
    session.apply(Event::Reply {
        request: request.id,
        error: None,
    });
    assert!(session.picker().is_none());
    assert_eq!(session.draft(), "Draft to keep");
}

#[test]
fn host_commands_and_argument_completion_need_no_domain_specific_ui_logic() {
    let mut session = session();
    session.insert("/sum");
    session.complete();
    assert_eq!(session.draft(), "/summarize ");
    session.insert("rep");
    session.complete();
    assert_eq!(session.draft(), "/summarize report");
    assert_eq!(
        session.input_command().unwrap(),
        Command::Invoke {
            name: "summarize".into(),
            arguments: "report".into()
        }
    );
    session.set_draft("/review");
    assert!(session.input_command().is_err());
}

#[test]
fn menus_filter_host_choices_and_preserve_settings_until_confirmation() {
    let mut session = session();
    session.command_prefix = '$';
    session.set_draft("$sum");
    assert_eq!(session.menu().entries()[0].label, "$summarize");
    assert!(session.accept_completion());
    assert_eq!(session.draft(), "$summarize ");
    assert!(session.menu().is_visible() && session.menu().lists_arguments());
    session.menu.navigate(true);
    assert!(session.accept_completion());
    assert_eq!(session.draft(), "$summarize notes");
    assert!(!session.menu().is_visible());
    assert_eq!(
        session.input_command().unwrap(),
        Command::Invoke {
            name: "summarize".into(),
            arguments: "notes".into(),
        }
    );
    assert_eq!(
        session
            .snapshot
            .as_ref()
            .unwrap()
            .model
            .as_ref()
            .unwrap()
            .model,
        "model-a"
    );
    session.set_draft("$summarize re");
    assert_eq!(session.menu().entries().len(), 1);
    session.menu.dismiss();
    assert!(!session.menu().is_visible());
    assert_eq!(session.draft(), "$summarize re");
    session.set_draft("$nothing");
    assert!(session.menu().is_visible() && session.menu().entries().is_empty());
    assert!(!session.accept_completion());
    assert!(session.input_command().is_err());
    session.snapshot.as_mut().unwrap().commands.clear();
    session.refresh_menu();
    assert!(!session.menu().is_visible());
    session.set_draft(String::new());
    session.complete();
    assert!(session.draft().is_empty());
}

#[test]
fn composer_actions_are_opt_in_verbatim_and_escapable() {
    let mut session = session();
    session.set_draft("!printf 'hello world'\n");
    assert!(
        matches!(session.input_command().unwrap(), Command::Submit { text, .. } if text == "!printf 'hello world'\n")
    );
    let mut state = snapshot();
    state
        .input_actions
        .push(InputAction::new('!', "summarize", "Summarize directly"));
    let mut encoded = serde_json::to_value(&state).unwrap();
    encoded.as_object_mut().unwrap().remove("input_actions");
    assert!(
        serde_json::from_value::<Snapshot>(encoded)
            .unwrap()
            .input_actions
            .is_empty()
    );
    session.apply(Event::Snapshot(state));
    session.set_draft("!echo 世界");
    let options = crate::UiOptions {
        input_prompt: "→".into(),
        ..Default::default()
    };
    let renderers = Renderers::default();
    let frame =
        render::screen_with_options(&session, 80, 24, &renderers, &options, Default::default());
    let (column, row) = frame.cursor.unwrap();
    assert_eq!(frame.lines[usize::from(row)].trim(), "! echo 世界");
    assert_eq!(usize::from(column), 4 + "echo 世界".width());
    // Home/Delete still edits the real prefix and leaves command mode.
    session.editor.home();
    let frame =
        render::screen_with_options(&session, 80, 24, &renderers, &options, Default::default());
    assert_eq!(frame.cursor.unwrap().0, 2);
    session.editor.delete();
    let frame =
        render::screen_with_options(&session, 80, 24, &renderers, &options, Default::default());
    assert_eq!(
        frame.lines[usize::from(frame.cursor.unwrap().1)].trim(),
        "→ echo 世界"
    );
    assert!(
        matches!(session.input_command().unwrap(), Command::Submit { text, .. } if text == "echo 世界")
    );
    session.set_draft("!printf 'hello world'\n");
    assert_eq!(
        session.input_command().unwrap(),
        Command::Invoke {
            name: "summarize".into(),
            arguments: "printf 'hello world'\n".into(),
        }
    );
    session.set_draft("\\!literal");
    assert!(
        matches!(session.input_command().unwrap(), Command::Submit { text, .. } if text == "!literal")
    );
    // Only a typed `\` escapes: a long paste that starts with one keeps it.
    session.set_draft(String::new());
    session.paste("\\!literal\nsecond\nthird");
    assert!(
        matches!(session.input_command().unwrap(), Command::Submit { text, .. } if text == "\\!literal\nsecond\nthird")
    );
    session.set_draft("!  \n");
    assert!(session.input_command().is_err());
    let mut bad = snapshot();
    bad.input_actions
        .push(InputAction::new('!', "unregistered", String::new()));
    let mut rejected = Session::default();
    rejected.apply(Event::Snapshot(bad));
    assert!(rejected.snapshot.is_none());
}

#[test]
fn forms_are_scoped_keep_rejected_drafts_and_clear_on_success_or_switch() {
    let mut s = session();
    s.editor.insert("preserve conversation draft");
    let form = FormSpec::new(
        "form-revision",
        "summarize",
        "Record settings",
        vec![
            FormField::new("label", "Label", FieldKind::Text)
                .with_value("record")
                .required(),
        ],
    );
    s.apply(Event::Form {
        conversation: "other-room".into(),
        form: form.clone(),
    });
    assert!(s.form.is_none());
    s.apply(Event::Form {
        conversation: "room-a".into(),
        form: {
            let mut form = form.clone();
            form.command = "unadvertised".into();
            form
        },
    });
    assert!(s.form.is_none());
    s.apply(Event::Form {
        conversation: "room-a".into(),
        form: form.clone(),
    });
    let command = Command::SubmitForm {
        name: "summarize".into(),
        form: form.id.clone(),
        values: Box::default(),
    };
    let request = s.request(command.clone()).unwrap();
    s.sent(&request);
    assert!(s.request(command.clone()).is_err());
    s.apply(Event::Reply {
        request: request.id,
        error: Some("Invalid label".into()),
    });
    assert!(s.form.is_some());
    assert_eq!(s.editor.text(), "preserve conversation draft");
    let request = s.request(command).unwrap();
    s.sent(&request);
    s.apply(Event::Reply {
        request: request.id,
        error: None,
    });
    assert!(s.form.is_none());
    s.apply(Event::Form {
        conversation: "room-a".into(),
        form,
    });
    s.apply(Event::Snapshot(snapshot()));
    assert!(s.form.is_none());
}
