#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]

use roomkit_tui_protocol::*;
use unicode_width::UnicodeWidthStr;

use super::*;
use crate::render::{self, Renderers};

#[test]
fn unicode_editing_keeps_graphemes_intact_and_paste_does_not_submit() {
    let mut editor = crate::editor::Editor::default();
    editor.insert("a👩‍💻e\u{301}");
    editor.backspace();
    assert_eq!(editor.text(), "a👩‍💻");
    editor.left();
    editor.delete();
    assert_eq!(editor.text(), "a");
    editor.insert("\r\n世界\u{1b}");
    assert_eq!(editor.text(), "a\n世界");
    editor.submitted();
    editor.insert("draft");
    editor.history_previous();
    assert_eq!(editor.text(), "a\n世界");
    editor.history_next();
    assert_eq!(editor.text(), "draft");

    // Deleting a newline can join a combining mark to the preceding letter.
    editor.set("a\n\u{301}".into());
    editor.left();
    editor.left();
    editor.delete();
    assert_eq!(editor.text(), "a\u{301}");
    assert_eq!(editor.cursor(), editor.text().len());
    editor.backspace();
    assert!(editor.text().is_empty());
}

/// A block of a long paste edits as one character: the cursor moves over it,
/// Backspace and Delete remove it whole, a vertical move never lands in it,
/// and history recalls it as a block that still sends the paste.
#[test]
fn a_pasted_block_edits_as_one_character_and_history_keeps_it() {
    let paste = "line 1\nline 2\nline 3\nline 4";
    let block = "[Pasted text #1 · 4 lines]";
    let mut editor = crate::editor::Editor::default();
    editor.insert("a ");
    editor.paste(paste);
    editor.insert(" b");
    assert_eq!(editor.text(), format!("a {block} b"));
    assert_eq!(editor.expanded(), format!("a {paste} b"));
    assert_eq!(editor.expanded_len(), editor.expanded().len());

    editor.left();
    editor.left();
    assert_eq!(editor.cursor(), 2 + block.len(), "after the block");
    editor.left();
    assert_eq!(editor.cursor(), 2, "over the block");
    editor.right();
    assert_eq!(editor.cursor(), 2 + block.len());
    editor.backspace();
    assert_eq!(editor.text(), "a  b");
    assert_eq!(editor.expanded(), "a  b");

    editor.set(String::new());
    editor.paste(paste);
    editor.home();
    editor.delete();
    assert!(editor.text().is_empty(), "{}", editor.text());

    // A vertical move toward the block's columns lands after it.
    editor.insert("first line\n");
    editor.paste(paste);
    editor.home();
    editor.vertical(true);
    editor.end();
    editor.vertical(false);
    assert_eq!(editor.cursor(), editor.text().len());

    editor.submitted();
    editor.insert("unsent");
    editor.history_previous();
    // Numbers are never reused in a draft: two blocks never share a label.
    assert!(editor.text().ends_with("[Pasted text #2 · 4 lines]"));
    assert_eq!(editor.expanded(), format!("first line\n{paste}"));
    editor.history_next();
    assert_eq!(editor.text(), "unsent");
    assert_eq!(editor.expanded(), "unsent");

    // A short paste stays as it is.
    editor.set(String::new());
    editor.paste("one\ntwo");
    assert_eq!(editor.text(), "one\ntwo");
}

#[test]
fn filled_composer_centers_the_draft_and_keeps_unicode_caret_inside_the_surface() {
    use crate::UiOptions;
    let mut session = session();
    let renderers = Renderers::default();
    let empty = render::screen(&session, 80, 24, &renderers);
    let surface: Vec<_> = empty
        .spans
        .iter()
        .filter(|line| line.iter().any(|span| span.tone == render::Tone::Input))
        .collect();
    assert_eq!(surface.len(), 3); // One editable row with padding above and below.
    let (column, row) = empty.cursor.unwrap();
    assert_eq!(column, 4);
    assert_eq!(empty.lines[usize::from(row)].trim(), "›");
    assert_eq!(empty.lines[usize::from(row) - 1].trim(), "");
    assert_eq!(empty.lines[usize::from(row) + 1].trim(), "");
    for (draft, height) in [("first\nsecond".to_string(), 4), ("x".repeat(53), 5)] {
        session.set_draft(draft);
        let frame = render::screen(&session, 32, 24, &renderers);
        let count = frame
            .spans
            .iter()
            .filter(|line| line.iter().any(|span| span.tone == render::Tone::Input))
            .count();
        assert_eq!(count, height); // Both explicit newlines and soft wrapping grow the surface.
    }
    for (minimum, padding) in [(1, 0), (2, 1), (5, 2), (u16::MAX, u16::MAX)] {
        let options = UiOptions {
            input_min_lines: minimum,
            input_padding: padding,
            ..Default::default()
        };
        for draft in ["", "世界👩‍💻", "first\nsecond\nthird\nfourth\nfifth\nsixth"] {
            session.set_draft(draft);
            for (width, height) in [(12, 8), (32, 14), (80, 24)] {
                let frame = render::screen_with_options(
                    &session,
                    width,
                    height,
                    &renderers,
                    &options,
                    Default::default(),
                );
                let (column, row) = frame.cursor.unwrap();
                assert!(column >= 2 && column < width - 1 && row < height);
                assert!(frame.spans[usize::from(row)].iter().all(|span| matches!(
                    span.tone,
                    render::Tone::Input | render::Tone::InputPrompt | render::Tone::Placeholder
                )));
                assert_eq!(frame.lines[usize::from(row)].width(), usize::from(width));
                assert!(frame.lines.len() <= usize::from(height));
            }
        }
    }
}

#[test]
fn host_placeholder_and_prompt_are_optional_sanitized_and_never_submitted() {
    use crate::UiOptions;
    let mut session = session();
    let renderers = Renderers::default();
    let options = UiOptions {
        input_placeholder: "Ask the observatory…\nNot another input row".into(),
        input_prompt: "→".into(),
        input_min_lines: 3,
        ..Default::default()
    };
    let frame =
        render::screen_with_options(&session, 80, 24, &renderers, &options, Default::default());
    let (_, row) = frame.cursor.unwrap();
    let row = usize::from(row);
    assert!(frame.lines[row].contains("→ Ask the observatory…"));
    assert!(
        frame.lines[row - 2..row]
            .iter()
            .all(|line| line.trim().is_empty())
    );
    assert!(
        frame.lines[row + 1..=row + 2]
            .iter()
            .all(|line| line.trim().is_empty())
    );
    assert!(
        !frame
            .lines
            .iter()
            .any(|line| line.contains("Not another input row"))
    );
    assert!(session.draft().is_empty());
    assert!(session.input_command().is_err());
    session.insert("a");
    let frame =
        render::screen_with_options(&session, 80, 24, &renderers, &options, Default::default());
    assert!(
        !frame
            .lines
            .iter()
            .any(|line| line.contains("Ask the observatory"))
    );
    assert_eq!(session.draft(), "a");
    session.set_draft(String::new());
    let options = UiOptions {
        input_placeholder: "\x1b]52;c;untrusted\x07".into(),
        input_prompt: "世界🌍very long".into(),
        ..Default::default()
    };
    let frame =
        render::screen_with_options(&session, 12, 8, &renderers, &options, Default::default());
    assert!(
        frame
            .lines
            .iter()
            .all(|line| line.width() <= 12 && !line.chars().any(char::is_control))
    );
    let options = UiOptions {
        input_prompt: String::new(),
        ..Default::default()
    };
    let frame =
        render::screen_with_options(&session, 80, 24, &renderers, &options, Default::default());
    let (column, row) = frame.cursor.unwrap();
    assert_eq!(column, 2);
    assert!(frame.lines[usize::from(row)].trim().is_empty());
}

#[test]
fn multiline_cursor_navigation_does_not_replace_the_draft_with_history() {
    let mut editor = crate::editor::Editor::default();
    editor.insert("old input");
    editor.submitted();
    editor.insert("界e\u{301}\nabc\n👩‍💻z");
    editor.vertical(true);
    assert_eq!(&editor.text()[..editor.cursor()], "界e\u{301}\nabc");
    editor.vertical(true);
    assert_eq!(&editor.text()[..editor.cursor()], "界e\u{301}");
    editor.vertical(false);
    editor.vertical(false);
    assert_eq!(editor.cursor(), editor.text().len());
    assert_eq!(editor.text(), "界e\u{301}\nabc\n👩‍💻z");
}

#[test]
fn attachment_draft_waits_for_acceptance_and_errors_preserve_it() {
    let mut s = session();
    assert!(s.add_image(image_fixture()).is_err());
    let mut view = snapshot();
    view.can_attach_images = true;
    s.apply(Event::Snapshot(view));
    s.add_image(image_fixture()).unwrap();
    let command = s.input_command().unwrap();
    assert!(
        matches!(&command, Command::SubmitImages { message, .. } if message.text.is_empty() && message.images.len() == 1)
    );
    let request = s.request(command).unwrap();
    s.sent(&request);
    assert!(s.images_pending());
    s.apply(Event::Reply {
        request: request.id,
        error: Some("Attachment rejected".into()),
    });
    assert_eq!(s.images.len(), 1);
    s.editor.insert("Describe this");
    let request = s.request(s.input_command().unwrap()).unwrap();
    s.sent(&request);
    s.apply(Event::Reply {
        request: request.id,
        error: None,
    });
    assert!(s.editor.text().is_empty());
    assert!(s.images.is_empty());
}

#[test]
fn images_cannot_be_silently_submitted_as_commands_or_to_a_text_only_agent() {
    let mut s = session();
    let mut view = snapshot();
    view.can_attach_images = true;
    s.apply(Event::Snapshot(view));
    for _ in 0..MAX_IMAGES {
        s.add_image(image_fixture()).unwrap();
    }
    assert!(s.add_image(image_fixture()).is_err());
    s.editor.insert("/summarize");
    assert!(
        s.input_command()
            .unwrap_err()
            .to_string()
            .contains("Remove attached images")
    );
    s.editor.set(String::new());
    s.apply(Event::Snapshot(snapshot()));
    assert_eq!(s.images.len(), MAX_IMAGES);
    assert!(
        s.input_command()
            .unwrap_err()
            .to_string()
            .contains("cannot receive images")
    );
    let mut next = snapshot();
    next.conversation = "room-b".into();
    s.apply(Event::Snapshot(next));
    assert!(s.images.is_empty());
}

#[test]
fn composer_renders_attachment_labels_without_payloads() {
    let mut s = session();
    let mut view = snapshot();
    view.can_attach_images = true;
    s.apply(Event::Snapshot(view));
    s.add_image(image_fixture()).unwrap();
    let screen = render::screen(&s, 100, 24, &Renderers::default());
    let text = screen.lines.join("\n");
    assert!(text.contains("Image 1"));
    assert!(!text.contains("AQIDBA=="));
    assert!(screen.cursor.is_some());
    for width in [20, 40, 80] {
        for height in [6, 10, 20] {
            let frame = render::screen(&s, width, height, &Renderers::default());
            assert!(frame.lines.len() <= usize::from(height));
            assert!(
                frame
                    .lines
                    .iter()
                    .all(|line| line.width() <= usize::from(width))
            );
            if let Some((x, y)) = frame.cursor {
                assert!(x < width && y < height);
            }
        }
    }
}
