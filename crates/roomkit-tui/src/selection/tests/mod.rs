#![allow(clippy::unwrap_used, reason = "Invalid fixtures must fail the test")]

mod conversation;

use base64::{Engine, engine::general_purpose::STANDARD};

use super::*;

/// Extend a screen selection, which reads only the frame of its press.
fn extend(selection: &mut TextSelection, column: u16, row: u16) {
    selection.hold(column, row, &Frame::default(), &Session::default());
}

/// The text of a screen selection, which lays nothing out again.
fn text(selection: &TextSelection) -> Result<Option<String>, &'static str> {
    selection.text(&Session::default(), |_, _| std::iter::empty())
}
fn frame() -> Frame {
    let lines = vec!["a界e\u{301}z".to_string(), "second line   ".to_string()];
    let spans = lines
        .iter()
        .map(|text| {
            vec![Span {
                text: text.clone(),
                tone: Tone::Text,
                emphasis: Default::default(),
                hyperlink: None,
            }]
        })
        .collect();
    Frame {
        lines,
        spans,
        cursor: Some((0, 0)),
        ..Frame::default()
    }
}

#[test]
fn multiline_copy_does_not_include_the_other_pane() {
    let mut frame = frame();
    frame.spans = vec![
        vec![Span::new("hello   │ side", Tone::Text)],
        vec![Span::new("world   │ panel", Tone::Text)],
    ];
    frame.lines = vec!["hello   │ side".into(), "world   │ panel".into()];
    frame.sidebar = Some(crate::render::SidebarArea {
        start: 8,
        height: 2,
        scroll: 0,
    });
    let mut left = TextSelection::new(frame.clone(), 0, 0, &Session::default());
    extend(&mut left, 7, 1);
    assert_eq!(text(&left).unwrap().unwrap(), "hello\nworld");
    let mut right = TextSelection::new(frame, 10, 0, &Session::default());
    extend(&mut right, 14, 1);
    assert_eq!(text(&right).unwrap().unwrap(), "side\npanel");
}

#[test]
fn reversed_selection_preserves_clusters_and_trims_padding() {
    let mut selection = TextSelection::new(frame(), 5, 1, &Session::default());
    extend(&mut selection, 2, 0); // Inside the wide glyph, selecting it whole.
    assert_eq!(
        text(&selection).unwrap().as_deref(),
        Some("界e\u{301}z\nsecond")
    );
    assert_eq!(selection.frozen().unwrap().spans[0][1].text, "界e\u{301}z");
    assert!(selection.frozen().unwrap().cursor.is_none());
    let mut selection = TextSelection::new(frame(), 0, 0, &Session::default());
    assert!(text(&selection).unwrap().is_none()); // A click never copies.
    extend(&mut selection, 300, 300); // Outside the visible frame is clamped.
    assert_eq!(
        text(&selection).unwrap().as_deref(),
        Some("a界e\u{301}z\nsecond line")
    );
}

#[test]
fn clipboard_payload_is_encoded_and_has_no_executable_terminal_controls() {
    let sequence = clipboard_sequence("text\x1b]52;c;attack\x07");
    let encoded = sequence
        .strip_prefix("\x1b]52;c;")
        .unwrap()
        .strip_suffix('\x07')
        .unwrap();
    assert!(!encoded.chars().any(char::is_control));
    assert_eq!(
        STANDARD.decode(encoded).unwrap(),
        b"text\x1b]52;c;attack\x07"
    );
}

#[test]
fn a_disclosure_click_is_scoped_and_dragging_back_never_triggers_it() {
    let mut frame = frame();
    frame.tool_targets.push(ToolTarget {
        conversation: "room".into(),
        row: 0,
        start: 1,
        end: 4,
        actor: "agent".into(),
        run: "run".into(),
        call: "call".into(),
    });
    let mut click = TextSelection::new(frame.clone(), 2, 0, &Session::default());
    extend(&mut click, 2, 0);
    assert_eq!(click.tool_click().unwrap().call.as_str(), "call");
    assert!(text(&click).unwrap().is_none());
    extend(&mut click, 3, 0);
    extend(&mut click, 2, 0);
    assert!(click.tool_click().is_none());
    let outside = TextSelection::new(frame, 4, 0, &Session::default());
    assert!(outside.tool_click().is_none());
}

#[test]
fn application_action_requires_a_click_and_never_a_drag() {
    let mut frame = frame();
    frame.action_targets.push(ActionTarget {
        conversation: "room".into(),
        command: "jobs".into(),
        arguments: "stop:opaque".into(),
        panel: Some("panel".into()),
        sidebar: false,
        row: 1,
        start: 2,
        end: 8,
    });
    let mut selection = TextSelection::new(frame.clone(), 3, 1, &Session::default());
    assert_eq!(selection.action_click().unwrap().arguments, "stop:opaque");
    extend(&mut selection, 4, 1);
    extend(&mut selection, 3, 1);
    assert!(selection.action_click().is_none());
    assert!(
        TextSelection::new(frame, 8, 1, &Session::default())
            .action_click()
            .is_none()
    );
}

#[test]
fn return_to_bottom_requires_a_click_not_a_text_selection() {
    let mut frame = frame();
    frame.back_to_bottom = Some(BackToBottomTarget {
        conversation: "room".into(),
        row: 1,
        start: 2,
        end: 8,
    });
    let mut click = TextSelection::new(frame.clone(), 2, 1, &Session::default());
    assert_eq!(
        click.back_to_bottom_click().unwrap().conversation.as_str(),
        "room"
    );
    assert!(text(&click).unwrap().is_none());
    extend(&mut click, 3, 1);
    extend(&mut click, 2, 1);
    assert!(click.back_to_bottom_click().is_none());
    assert!(
        TextSelection::new(frame.clone(), 8, 1, &Session::default())
            .back_to_bottom_click()
            .is_none()
    );
    assert!(
        TextSelection::new(frame, 2, 0, &Session::default())
            .back_to_bottom_click()
            .is_none()
    );
}
