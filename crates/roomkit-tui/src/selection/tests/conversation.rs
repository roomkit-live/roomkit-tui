//! A selection anchored in the conversation: scrolling, off-screen text and
//! new messages, exercised through the real session and renderer.

use std::time::Duration;

use roomkit_tui_protocol::{Event, Item, Message, ToolCall, ToolState};
use serde_json::json;

use super::*;
use crate::UiOptions;
use crate::render::{self, Renderers};

const WIDTH: u16 = 60;
const HEIGHT: u16 = 20;

/// A conversation with one reply of `paragraphs` numbered paragraphs, far
/// longer than the screen.
fn conversation(paragraphs: usize) -> Session {
    replies(1, paragraphs)
}

/// `count` replies of `paragraphs` paragraphs each, numbered across replies.
fn replies(count: usize, paragraphs: usize) -> Session {
    let mut snapshot = crate::tests::snapshot();
    for reply in 0..count {
        let text = (1..=paragraphs)
            .map(|n| format!("Paragraph {:02} of the reply.", reply * paragraphs + n))
            .collect::<Vec<_>>()
            .join("\n\n");
        let id = format!("reply-{reply}");
        snapshot
            .items
            .push(Item::Message(Message::new(id, "agent", text)));
    }
    let mut session = Session::default();
    session.apply(Event::Snapshot(snapshot));
    session
}

/// Lay out the screen and keep the offsets it shows, like the terminal.
fn draw(session: &mut Session) -> Frame {
    draw_with(session, &UiOptions::default())
}

fn draw_with(session: &mut Session, options: &UiOptions) -> Frame {
    let renderers = Renderers::default();
    let frame =
        render::screen_with_options(session, WIDTH, HEIGHT, &renderers, options, Duration::ZERO);
    session.sync(&frame, Duration::ZERO);
    frame
}

fn row_of(frame: &Frame, text: &str) -> u16 {
    frame
        .lines
        .iter()
        .position(|line| line.contains(text))
        .unwrap() as u16
}

fn copy(selection: &TextSelection, session: &Session) -> Result<Option<String>, &'static str> {
    copy_with(selection, session, &UiOptions::default())
}

fn copy_with(
    selection: &TextSelection,
    session: &Session,
    options: &UiOptions,
) -> Result<Option<String>, &'static str> {
    let renderers = Renderers::default();
    selection.text(session, |items, width| {
        render::timeline_text(session, &renderers, options, Duration::ZERO, width, items)
    })
}

/// Highlighted text of the conversation area; controls have their own tones.
fn selected_text(frame: &Frame) -> String {
    let area = frame.timeline.as_ref().unwrap();
    frame.spans[area.top..area.top + area.height]
        .iter()
        .flatten()
        .filter(|span| span.tone == Tone::Selected)
        .map(|span| span.text.as_str())
        .collect()
}

#[test]
fn scrolling_while_held_extends_the_selection_beyond_the_screen() {
    let mut session = conversation(30);
    session.scroll_back(1_000);
    let top = draw(&mut session);
    let start = row_of(&top, "Paragraph 01");
    let mut selection = TextSelection::new(top, 0, start, &session);
    assert!(selection.scrolls());

    // The wheel scrolls under the held pointer, which now points later on.
    session.scroll_to_bottom();
    selection.content_scrolled();
    let bottom = draw(&mut session);
    selection.follow(&bottom, &session);
    assert!(
        selection.dragged(),
        "the end follows the pointer after a scroll"
    );
    let end = row_of(&bottom, "Paragraph 30");
    selection.hold(WIDTH - 1, end, &bottom, &session);
    selection.release();

    assert!(!bottom.lines.join("\n").contains("Paragraph 01"));
    let text = copy(&selection, &session).unwrap().unwrap();
    // Copied as shown: the message's indentation, the screen's line breaks.
    assert!(
        text.starts_with("   Paragraph 01 of the reply.\n\n"),
        "{text}"
    );
    assert!(text.ends_with("Paragraph 30 of the reply."), "{text}");
    assert_eq!(text.matches("Paragraph").count(), 30);
}

#[test]
fn a_selection_dragged_upwards_copies_in_document_order() {
    let mut session = conversation(30);
    let bottom = draw(&mut session);
    let mut selection = TextSelection::new(
        bottom,
        13,
        row_of(&draw(&mut session), "Paragraph 29"),
        &session,
    );
    session.scroll_back(1_000);
    let top = draw(&mut session);
    selection.hold(10, row_of(&top, "Paragraph 02"), &top, &session);
    // Columns 10 and 13 cut inside "Paragraph": the ends keep their columns.
    assert_eq!(
        copy(&selection, &session).unwrap().unwrap().lines().next(),
        Some("ph 02 of the reply.")
    );
    assert!(
        copy(&selection, &session)
            .unwrap()
            .unwrap()
            .ends_with("\n   Paragraph 2")
    );
}

#[test]
fn the_highlight_follows_its_text_when_scrolled_and_a_message_arrives() {
    let mut session = conversation(30);
    let frame = draw(&mut session);
    let row = row_of(&frame, "Paragraph 29");
    let mut selection = TextSelection::new(frame.clone(), 0, row, &session);
    selection.hold(WIDTH - 1, row, &frame, &session);
    selection.release();
    let mut shown = frame.clone();
    selection.paint(&mut shown, &session);
    assert_eq!(selected_text(&shown).trim(), "Paragraph 29 of the reply.");

    session.apply(Event::Item {
        conversation: "room-a".into(),
        item: Item::Message(Message::new("next", "agent", "A newer reply")),
    });
    session.scroll_back(2);
    selection.content_scrolled();
    let mut shown = draw(&mut session);
    // A released selection no longer moves with the pointer.
    selection.follow(&shown, &session);
    selection.paint(&mut shown, &session);
    assert_eq!(selected_text(&shown).trim(), "Paragraph 29 of the reply.");
    assert_eq!(
        copy(&selection, &session).unwrap().unwrap(),
        "   Paragraph 29 of the reply."
    );
}

#[test]
fn a_title_change_keeps_the_selection_and_its_highlight() {
    let mut session = conversation(30);
    let frame = draw(&mut session);
    let row = row_of(&frame, "Paragraph 29");
    let mut selection = TextSelection::new(frame.clone(), 0, row, &session);
    selection.hold(WIDTH - 1, row, &frame, &session);

    let renamed = Event::TitleChanged {
        conversation: "room-a".into(),
        title: "Renamed".into(),
    };
    selection.observe(&renamed);
    session.apply(renamed);
    let mut shown = draw(&mut session);
    let header = shown.timeline.as_ref().unwrap().top;
    assert!(
        shown.lines[..header]
            .iter()
            .any(|line| line.contains("Renamed"))
    );
    selection.follow(&shown, &session);
    selection.release();
    selection.paint(&mut shown, &session);
    assert_eq!(selected_text(&shown).trim(), "Paragraph 29 of the reply.");
    assert_eq!(
        copy(&selection, &session).unwrap().unwrap(),
        "   Paragraph 29 of the reply."
    );
}

#[test]
fn a_selection_spans_grouped_tool_activity_as_shown() {
    let options = UiOptions {
        tool_activity: true,
        animations: false,
        ..Default::default()
    };
    let mut session = replies(2, 6);
    let tool = |call: &str| Item::Tool {
        actor: "agent".into(),
        run: "run-1".into(),
        call: ToolCall::new(call, "search_records", json!({}), ToolState::Succeeded),
    };
    let items = &mut session.snapshot.as_mut().unwrap().items;
    items.insert(1, tool("c-2"));
    items.insert(1, tool("c-1"));
    session.scroll_back(1_000);
    let top = draw_with(&mut session, &options);
    let start = row_of(&top, "Paragraph 01");
    let mut selection = TextSelection::new(top, 0, start, &session);
    session.scroll_to_bottom();
    let bottom = draw_with(&mut session, &options);
    let end = row_of(&bottom, "Paragraph 12");
    selection.hold(WIDTH - 1, end, &bottom, &session);
    assert!(!bottom.lines.join("\n").contains("Paragraph 01"));
    let text = copy_with(&selection, &session, &options).unwrap().unwrap();
    let group = text.lines().filter(|line| line.contains("search_records"));
    assert_eq!(group.count(), 1, "{text}");
    assert!(text.starts_with("   Paragraph 01") && text.ends_with("Paragraph 12 of the reply."));
}

#[test]
fn the_clipboard_limit_counts_lines_off_screen() {
    // Each message is bounded; the selection spans many of them.
    let mut session = replies(40, 60);
    session.scroll_back(usize::MAX / 2);
    let top = draw(&mut session);
    let mut selection = TextSelection::new(
        top,
        0,
        row_of(&draw(&mut session), "Paragraph 01"),
        &session,
    );
    session.scroll_to_bottom();
    let bottom = draw(&mut session);
    selection.hold(
        WIDTH - 1,
        row_of(&bottom, "Paragraph 2400"),
        &bottom,
        &session,
    );
    assert_eq!(
        copy(&selection, &session),
        Err("Selection exceeds the 64 KiB clipboard limit")
    );
}

#[test]
fn a_selection_whose_text_left_the_conversation_copies_and_paints_nothing() {
    let mut session = conversation(30);
    let frame = draw(&mut session);
    let mut selection =
        TextSelection::new(frame.clone(), 0, row_of(&frame, "Paragraph 28"), &session);
    selection.hold(WIDTH - 1, row_of(&frame, "Paragraph 29"), &frame, &session);
    let mut other = crate::tests::snapshot();
    other.conversation = "room-b".into();
    session.apply(Event::Snapshot(other));
    let mut shown = draw(&mut session);
    selection.paint(&mut shown, &session);
    assert!(selected_text(&shown).is_empty());
    // Said rather than copying nothing in silence.
    assert_eq!(
        copy(&selection, &session),
        Err("Selected text is no longer in the conversation")
    );
}

#[test]
fn only_a_press_on_a_history_line_anchors_in_the_conversation() {
    let mut session = conversation(30);
    let frame = draw(&mut session);
    let area = frame.timeline.clone().unwrap();
    assert!(TextSelection::new(frame.clone(), 0, area.top as u16, &session).scrolls());
    assert!(!TextSelection::new(frame.clone(), 0, 0, &session).scrolls()); // Header.
    let composer = (area.top + area.height + 2) as u16;
    assert!(!TextSelection::new(frame, 0, composer, &session).scrolls());
}

#[test]
fn a_held_pointer_past_the_history_points_at_an_edge_until_released() {
    let mut session = conversation(30);
    let frame = draw(&mut session);
    let area = frame.timeline.clone().unwrap();
    let mut selection = TextSelection::new(frame.clone(), 0, area.top as u16, &session);
    assert_eq!(selection.edge(&frame), None);
    selection.hold(0, 0, &frame, &session);
    assert_eq!(selection.edge(&frame), Some(Edge::Above));
    let below = (area.top + area.height) as u16;
    selection.hold(0, below, &frame, &session);
    assert_eq!(selection.edge(&frame), Some(Edge::Below));
    selection.release();
    assert_eq!(selection.edge(&frame), None);
}

#[test]
fn the_highlight_never_reaches_the_sidebar() {
    let mut session = conversation(30);
    session.apply(Event::SidebarChanged {
        conversation: "room-a".into(),
        sidebar: Some(crate::tests::sidebar_spec()),
    });
    let frame = render::screen(&session, 120, HEIGHT, &Renderers::default());
    let sidebar = frame.sidebar.clone().unwrap();
    let first = row_of(&frame, "Paragraph 27");
    let mut selection = TextSelection::new(frame.clone(), 0, first, &session);
    selection.hold(119, row_of(&frame, "Paragraph 29"), &frame, &session);
    let mut shown = frame;
    selection.paint(&mut shown, &session);
    for spans in &shown.spans {
        let mut column = 0;
        for span in spans {
            if span.tone == Tone::Selected {
                assert!(column + span.text.width() <= sidebar.start);
            }
            column += span.text.width();
        }
    }
    assert!(selected_text(&shown).contains("Paragraph 28 of the reply."));
}

/// A press and a release on the same cell, with `arrive` applied between
/// them and drawn as the terminal does.
fn still_click(
    session: &mut Session,
    (column, row): (u16, u16),
    arrive: impl FnOnce(&mut Session),
) -> TextSelection {
    let pressed = draw(session);
    let mut selection = TextSelection::new(pressed, column, row, session);
    arrive(session);
    let next = draw(session);
    selection.follow(&next, session);
    selection.hold(column, row, &next, session);
    selection.release();
    selection
}

fn started(session: &mut Session) {
    session.apply(Event::RunStarted {
        conversation: "room-a".into(),
        run: "run-1".into(),
    });
}

fn streamed(session: &mut Session, text: &str) {
    session.apply(Event::TextDelta {
        conversation: "room-a".into(),
        message: "draft:run-1:0".into(),
        actor: "agent".into(),
        run: "run-1".into(),
        text: text.into(),
    });
}

#[test]
fn text_streaming_under_a_still_pointer_selects_nothing() {
    let mut session = conversation(30);
    started(&mut session);
    let row = row_of(&draw(&mut session), "Paragraph 28");
    let selection = still_click(&mut session, (5, row), |session| {
        streamed(session, "Streaming line one.\n\nStreaming line two.");
    });
    assert!(!selection.dragged());
    assert_eq!(copy(&selection, &session), Ok(None));
}

#[test]
fn a_tool_click_stays_a_click_while_a_message_arrives() {
    let mut session = conversation(30);
    let mut call = ToolCall::new("c-1", "search_records", json!({}), ToolState::Succeeded);
    call.text = (1..=20)
        .map(|n| format!("record {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let items = &mut session.snapshot.as_mut().unwrap().items;
    items.push(Item::Tool {
        actor: "agent".into(),
        run: "run-1".into(),
        call,
    });
    let target = draw(&mut session).tool_targets[0].clone();
    let selection = still_click(
        &mut session,
        (target.start as u16, target.row as u16),
        |session| {
            session.apply(Event::Item {
                conversation: "room-a".into(),
                item: Item::Message(Message::new("next", "agent", "A newer reply")),
            });
        },
    );
    assert!(selection.scrolls());
    assert_eq!(selection.tool_click().map(|t| t.call.as_str()), Some("c-1"));
    assert_eq!(copy(&selection, &session), Ok(None));
}

#[test]
fn a_selection_in_a_streamed_reply_survives_its_finalization() {
    let mut session = conversation(3);
    started(&mut session);
    let reply = "Streamed first.\n\nStreamed second.\n\nStreamed third.";
    streamed(&mut session, reply);
    let frame = draw(&mut session);
    let mut selection =
        TextSelection::new(frame.clone(), 0, row_of(&frame, "Streamed first"), &session);
    selection.hold(
        WIDTH - 1,
        row_of(&frame, "Streamed third"),
        &frame,
        &session,
    );
    let finalized = Event::MessageFinalized {
        conversation: "room-a".into(),
        draft: "draft:run-1:0".into(),
        message: Message::new("stored-1", "agent", reply),
    };
    // The terminal shows the selection each event before the session applies it.
    selection.observe(&finalized);
    session.apply(finalized);
    let mut shown = draw(&mut session);
    selection.release();
    selection.paint(&mut shown, &session);
    assert!(selected_text(&shown).contains("Streamed second."));
    let text = copy(&selection, &session).unwrap().unwrap();
    assert!(text.starts_with("   Streamed first.") && text.ends_with("Streamed third."));
}

#[test]
fn the_edge_above_the_start_of_the_history_scrolls_no_further() {
    let mut session = conversation(30);
    session.scroll_back(1_000);
    let top = draw(&mut session);
    let area = top.timeline.clone().unwrap();
    let mut selection = TextSelection::new(top.clone(), 0, area.top as u16 + 2, &session);
    selection.hold(0, 0, &top, &session);
    assert_eq!(selection.edge(&top), None);
    let below = (area.top + area.height) as u16;
    selection.hold(0, below, &top, &session);
    assert_eq!(selection.edge(&top), Some(Edge::Below));
}
