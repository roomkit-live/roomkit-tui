//! RMK-1325: a model's reasoning shows as it comes, and folds to one line once
//! the model goes on. Nothing stores it.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]

use std::time::Duration;

use super::*;
use crate::render::{self, Renderers};
use crate::{Steering, UiOptions};

const RUN: &str = "agent:m1";

fn turn() -> Session {
    let mut snapshot = snapshot();
    snapshot.can_steer = true;
    snapshot.local_actor = Some("person:alice".into());
    let mut session = Session::default();
    session.apply(Event::Snapshot(snapshot));
    started(&mut session, RUN);
    session
}

fn secs(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

fn thinks_at(session: &mut Session, at: u64, text: &str) {
    let event = Event::ThinkingDelta {
        conversation: "room-a".into(),
        actor: "agent".into(),
        run: RUN.into(),
        text: text.into(),
    };
    session.apply_at(event, secs(at));
}

fn writes_at(session: &mut Session, at: u64, text: &str) {
    let event = Event::TextDelta {
        conversation: "room-a".into(),
        message: "d0".into(),
        actor: "agent".into(),
        run: RUN.into(),
        text: text.into(),
    };
    session.apply_at(event, secs(at));
}

fn calls(session: &mut Session, at: u64, id: &str, state: ToolState) {
    let event = Event::Item {
        conversation: "room-a".into(),
        item: Item::Tool {
            actor: "agent".into(),
            run: RUN.into(),
            call: ToolCall::new(id, "lookup", json!({}), state),
        },
    };
    session.apply_at(event, secs(at));
}

fn draw(session: &Session, at: u64) -> String {
    render::screen_with_options(
        session,
        60,
        30,
        &Renderers::default(),
        &UiOptions::default(),
        secs(at),
    )
    .lines
    .join("\n")
}

fn order(session: &Session) -> Vec<&'static str> {
    session
        .snapshot()
        .unwrap()
        .items
        .iter()
        .map(|item| match item {
            Item::Thinking(_) => "thinking",
            Item::Tool { .. } => "tool",
            Item::Message(message) if message.actor.as_str() == "agent" => "answer",
            Item::Message(_) => "person",
            _ => "other",
        })
        .collect()
}

#[test]
fn reasoning_shows_its_last_lines_and_its_time_while_it_comes() {
    let mut session = turn();
    thinks_at(&mut session, 10, "one\ntwo\nthree\n");
    thinks_at(&mut session, 11, "four");
    let screen = draw(&session, 22);
    assert!(screen.contains("∴ Thinking · 12s"), "{screen}");
    assert!(
        screen.contains("two") && screen.contains("four"),
        "{screen}"
    );
    assert!(!screen.contains("one"), "{screen}");
    assert_eq!(order(&session), ["thinking"], "One block");
}

#[test]
fn the_answer_folds_the_reasoning_to_one_line() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing the options");
    writes_at(&mut session, 22, "The answer");
    let screen = draw(&session, 99);
    assert!(screen.contains("∴ Thought for 12.0s"), "{screen}");
    assert!(!screen.contains("Weighing"), "{screen}");
    assert!(screen.contains("The answer"), "{screen}");
    assert_eq!(order(&session), ["thinking", "answer"]);
}

#[test]
fn blanks_between_the_reasoning_and_the_answer_do_not_end_it() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing");
    writes_at(&mut session, 12, "\n\n");
    assert!(draw(&session, 13).contains("∴ Thinking"));
    writes_at(&mut session, 14, "Words");
    assert!(draw(&session, 15).contains("∴ Thought for 4.0s"));
}

#[test]
fn a_call_and_the_end_of_the_run_end_the_reasoning_too() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing");
    calls(&mut session, 11, "c1", ToolState::Running);
    assert!(draw(&session, 12).contains("∴ Thought for 1.0s"));

    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing");
    session.apply_at(
        Event::RunFinished {
            conversation: "room-a".into(),
            run: RUN.into(),
            outcome: RunOutcome::Interrupted,
        },
        secs(13),
    );
    assert!(draw(&session, 14).contains("∴ Thought for 3.0s"));
}

#[test]
fn each_answer_of_the_model_has_its_own_block_in_the_turn_s_order() {
    let mut session = turn();
    thinks_at(&mut session, 10, "First");
    calls(&mut session, 11, "c1", ToolState::Running);
    calls(&mut session, 12, "c1", ToolState::Succeeded);
    thinks_at(&mut session, 13, "Second");
    assert_eq!(order(&session), ["thinking", "tool", "thinking"]);
    let screen = draw(&session, 14);
    assert!(screen.contains("∴ Thought for 1.0s"), "{screen}");
    assert!(screen.contains("∴ Thinking · 1s"), "{screen}");
    assert!(
        screen.contains("Second") && !screen.contains("First"),
        "{screen}"
    );
}

#[test]
fn a_folded_block_opens_and_closes_as_a_call_s_details_do() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing the options");
    writes_at(&mut session, 12, "Words");
    assert!(draw(&session, 13).contains("· F2 expand"));
    session.toggle_tool();
    let opened = draw(&session, 13);
    assert!(opened.contains("Weighing the options"), "{opened}");
    assert!(opened.contains("· F2 collapse"), "{opened}");
    session.toggle_tool();
    assert!(!draw(&session, 13).contains("Weighing"));
}

#[test]
fn an_opened_block_shows_a_bounded_number_of_lines() {
    let mut session = turn();
    let long: Vec<String> = (0..150).map(|index| format!("line-{index}")).collect();
    thinks_at(&mut session, 10, &long.join("\n"));
    writes_at(&mut session, 11, "Words");
    session.toggle_tool();
    let lines = render::screen_with_options(
        &session,
        100,
        200,
        &Renderers::default(),
        &UiOptions::default(),
        secs(12),
    )
    .lines;
    assert!(lines.iter().any(|line| line.contains("line-149")));
    assert!(lines.iter().any(|line| line.contains("line-50")));
    assert!(!lines.iter().any(|line| line.contains("line-49")));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("view limited to the last 100 lines"))
    );
}

/// A model's words are no terminal sequences.
#[test]
fn reasoning_cannot_drive_the_terminal() {
    let mut session = turn();
    thinks_at(&mut session, 10, "plain\u{1b}[2Jtext\u{7}");
    let screen = draw(&session, 11);
    assert!(
        !screen.contains('\u{1b}') && !screen.contains('\u{7}'),
        "{screen:?}"
    );
}

#[test]
fn blank_reasoning_takes_no_room() {
    let mut session = turn();
    thinks_at(&mut session, 10, "\n \n");
    assert!(!draw(&session, 11).contains('∴'));
}

#[test]
fn reasoning_of_another_conversation_or_of_a_finished_run_is_ignored() {
    let mut session = turn();
    session.apply(Event::ThinkingDelta {
        conversation: "room-b".into(),
        actor: "agent".into(),
        run: RUN.into(),
        text: "Elsewhere".into(),
    });
    session.apply(Event::ThinkingDelta {
        conversation: "room-a".into(),
        actor: "agent".into(),
        run: "agent:old".into(),
        text: "Before".into(),
    });
    assert!(order(&session).is_empty());
}

/// A view that came back reads what the host holds, and it holds no reasoning.
#[test]
fn a_snapshot_drops_the_reasoning() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing");
    session.apply(Event::Snapshot(snapshot()));
    assert!(order(&session).is_empty());
    assert!(!draw(&session, 11).contains('∴'));
}

/// reasoning after the last call must not take F2 from it.
#[test]
fn f2_still_reaches_the_newest_call_after_reasoning() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Before the call");
    calls(&mut session, 11, "c1", ToolState::Running);
    calls(&mut session, 12, "c1", ToolState::Succeeded);
    thinks_at(&mut session, 13, "After the call");
    writes_at(&mut session, 14, "Words");
    let (actor, run, call) = ("agent".into(), RUN.into(), "c1".into());
    assert!(session.tool_focused(&actor, &run, &call));
    session.toggle_tool();
    assert!(session.tool_expanded(&actor, &run, &call));
    assert!(!draw(&session, 15).contains("After the call"));
}

/// With no call to open, F2 opens the reasoning.
#[test]
fn f2_opens_the_reasoning_when_the_conversation_has_no_call() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing");
    writes_at(&mut session, 11, "Words");
    session.toggle_tool();
    assert!(draw(&session, 12).contains("Weighing"));
}

/// Alt+Up reaches a block; it is in the focus order.
#[test]
fn focus_moves_to_a_block_and_f2_opens_it() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing");
    calls(&mut session, 11, "c1", ToolState::Succeeded);
    session.focus_tool(true);
    session.toggle_tool();
    assert!(draw(&session, 12).contains("Weighing"));
}

/// The focus moves from the call F2 reaches, not from the block after it.
#[test]
fn focus_moves_from_the_call_f2_reaches() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Before the call");
    calls(&mut session, 11, "c1", ToolState::Succeeded);
    thinks_at(&mut session, 12, "After the call");
    writes_at(&mut session, 13, "Words");
    session.focus_tool(true);
    session.toggle_tool();
    assert!(draw(&session, 14).contains("Before the call"));
}

/// The activity view, which roomkit-code shows by default, opens blocks with the
/// technical timeline: by F2, and by a click on a block.
#[test]
fn the_activity_view_opens_blocks_with_the_timeline() {
    let options = UiOptions {
        tool_activity: true,
        ..UiOptions::default()
    };
    let frame = |session: &Session| {
        render::screen_with_options(session, 60, 30, &Renderers::default(), &options, secs(14))
    };
    let shown = |session: &Session| frame(session).lines.join("\n").contains("Weighing");
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing");
    calls(&mut session, 11, "c1", ToolState::Succeeded);
    writes_at(&mut session, 12, "Words");
    assert!(!shown(&session));

    session.toggle_activity();
    assert!(shown(&session), "F2 opens it");
    session.toggle_activity();
    assert!(!shown(&session), "F2 closes it");

    let header = frame(&session);
    let row = header
        .lines
        .iter()
        .position(|line| line.contains("∴ Thought"))
        .unwrap();
    let target = header
        .tool_targets
        .iter()
        .find(|target| target.row == row)
        .unwrap();
    session.toggle_activity_target(target);
    assert!(shown(&session), "A click opens it");
}

/// blanks open no block, so no invisible item takes the
/// focus or parts two calls, and a blank says no stop passed.
#[test]
fn blank_reasoning_opens_no_block() {
    let mut session = turn();
    calls(&mut session, 10, "c1", ToolState::Succeeded);
    thinks_at(&mut session, 11, "\n\n");
    calls(&mut session, 12, "c2", ToolState::Running);
    assert_eq!(order(&session), ["tool", "tool"]);
    assert!(session.tool_focused(&"agent".into(), &RUN.into(), &"c2".into()));
}

#[test]
fn a_blank_after_the_calls_says_no_stop_passed() {
    let mut session = turn();
    calls(&mut session, 10, "c1", ToolState::Running);
    hand(&mut session, "sent");
    calls(&mut session, 11, "c1", ToolState::Succeeded);
    thinks_at(&mut session, 12, "\n");
    assert_eq!(session.steering(&"sent".into()), Some(Steering::Handed));
}

#[test]
fn blanks_of_the_next_answer_say_no_stop_passed_twice() {
    let mut session = turn();
    calls(&mut session, 10, "c1", ToolState::Succeeded);
    hand(&mut session, "sent");
    thinks_at(&mut session, 11, "\n");
    thinks_at(&mut session, 12, "\n");
    thinks_at(&mut session, 13, "Going on without it");
    assert_eq!(order(&session), ["tool", "thinking", "person"]);
    assert_eq!(session.steering(&"sent".into()), Some(Steering::Handed));
}

/// the view promises the last lines, whatever came before.
#[test]
fn the_newest_lines_stay_in_view_past_the_bound() {
    let mut session = turn();
    let limit = crate::Limits::default().text_bytes.get();
    let piece = format!("{}\n", "x".repeat(1023));
    for at in 0..(limit / 1024 + 8) {
        thinks_at(&mut session, at as u64, &piece);
    }
    thinks_at(&mut session, 99, "NEWEST-LINE");
    assert!(draw(&session, 100).contains("NEWEST-LINE"));
    writes_at(&mut session, 101, "Words");
    session.toggle_tool();
    assert!(
        draw(&session, 102).contains("NEWEST-LINE"),
        "Opened once ended"
    );
    let snapshot = session.snapshot().unwrap();
    let Item::Thinking(block) = &snapshot.items[0] else {
        panic!("Expected reasoning");
    };
    assert!(block.text.len() <= limit, "{}", block.text.len());
}

#[test]
fn one_huge_piece_of_reasoning_keeps_its_end_within_the_bound() {
    let mut session = turn();
    let limit = crate::Limits::default().text_bytes.get();
    thinks_at(&mut session, 10, &format!("{}END", "é".repeat(limit)));
    assert!(draw(&session, 11).contains("END"));
    let snapshot = session.snapshot().unwrap();
    let Item::Thinking(block) = &snapshot.items[0] else {
        panic!("Expected reasoning");
    };
    assert!(block.text.len() <= limit, "{}", block.text.len());
}

/// opened while it comes, a block follows its newest lines.
#[test]
fn a_block_opened_while_it_comes_shows_its_newest_lines() {
    let mut session = turn();
    let long: Vec<String> = (0..150).map(|index| format!("line-{index}")).collect();
    thinks_at(&mut session, 10, &long.join("\n"));
    session.toggle_tool();
    let lines = render::screen_with_options(
        &session,
        100,
        200,
        &Renderers::default(),
        &UiOptions::default(),
        secs(12),
    )
    .lines;
    assert!(lines.iter().any(|line| line.contains("line-149")));
    assert!(lines.iter().any(|line| line.contains("line-50")));
    assert!(!lines.iter().any(|line| line.contains("line-49")));

    writes_at(&mut session, 13, "Words");
    let ended = render::screen_with_options(
        &session,
        100,
        200,
        &Renderers::default(),
        &UiOptions::default(),
        secs(14),
    )
    .lines;
    assert!(
        ended.iter().any(|line| line.contains("line-149")),
        "The view does not jump when the block ends"
    );
}

fn thinks_as(session: &mut Session, actor: &str, run: &str, at: u64, text: &str) {
    let event = Event::ThinkingDelta {
        conversation: "room-a".into(),
        actor: actor.into(),
        run: run.into(),
        text: text.into(),
    };
    session.apply_at(event, secs(at));
}

fn thinking_texts(session: &Session) -> Vec<&str> {
    session
        .snapshot()
        .unwrap()
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Thinking(block) => Some(block.text.as_str()),
            _ => None,
        })
        .collect()
}

/// each run keeps its block, whatever another run writes.
#[test]
fn another_run_neither_ends_a_block_nor_writes_in_it() {
    let mut session = turn();
    started(&mut session, "agent-b:m2");
    thinks_as(&mut session, "agent", RUN, 10, "A one");
    thinks_as(&mut session, "agent-b", "agent-b:m2", 11, "B one");
    session.apply_at(
        Event::TextDelta {
            conversation: "room-a".into(),
            message: "b0".into(),
            actor: "agent-b".into(),
            run: "agent-b:m2".into(),
            text: "B words".into(),
        },
        secs(12),
    );
    session.apply_at(
        Event::MessageFinalized {
            conversation: "room-a".into(),
            draft: "b0".into(),
            message: Message::new("b1", "agent-b", "B words"),
        },
        secs(13),
    );
    thinks_as(&mut session, "agent", RUN, 14, " A two");
    assert_eq!(thinking_texts(&session), ["A one A two", "B one"]);
    assert!(draw(&session, 15).contains("∴ Thinking"));
}

/// An answer delivered whole ends its agent's reasoning.
#[test]
fn a_finished_answer_ends_its_agent_s_reasoning() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing");
    session.apply_at(
        Event::MessageFinalized {
            conversation: "room-a".into(),
            draft: "d0".into(),
            message: Message::new("a0", "agent", "Words"),
        },
        secs(12),
    );
    assert!(draw(&session, 13).contains("∴ Thought for 2.0s"));
}

/// reasoning told after its run's end opens nothing.
#[test]
fn reasoning_after_the_end_of_its_run_is_ignored() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing");
    session.apply_at(
        Event::RunFinished {
            conversation: "room-a".into(),
            run: RUN.into(),
            outcome: RunOutcome::Completed,
        },
        secs(11),
    );
    thinks_at(&mut session, 12, " late");
    assert_eq!(thinking_texts(&session), ["Weighing"]);
    assert!(!draw(&session, 13).contains("∴ Thinking"));
}

/// a long session keeps every block's time.
#[test]
fn a_long_turn_keeps_each_block_s_time() {
    let mut session = turn();
    for round in 0..70u64 {
        thinks_at(&mut session, round * 10, "Weighing");
        calls(
            &mut session,
            round * 10 + 5,
            &format!("c{round}"),
            ToolState::Succeeded,
        );
    }
    let times = render::screen_with_options(
        &session,
        100,
        400,
        &Renderers::default(),
        &UiOptions::default(),
        secs(999),
    )
    .lines
    .iter()
    .filter(|line| line.contains("∴ Thought for 5.0s"))
    .count();
    assert_eq!(times, 70);
}

#[test]
fn another_conversation_ends_no_block() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing");
    session.apply_at(
        Event::TextDelta {
            conversation: "room-b".into(),
            message: "d0".into(),
            actor: "agent".into(),
            run: RUN.into(),
            text: "Elsewhere".into(),
        },
        secs(11),
    );
    assert!(draw(&session, 12).contains("∴ Thinking"));
}

#[test]
fn a_view_without_a_clock_shows_no_time() {
    let mut session = turn();
    session.apply(Event::ThinkingDelta {
        conversation: "room-a".into(),
        actor: "agent".into(),
        run: RUN.into(),
        text: "Weighing".into(),
    });
    let live = draw(&session, 20);
    assert!(
        live.contains("∴ Thinking") && !live.contains("∴ Thinking ·"),
        "{live}"
    );
    session.apply(Event::RunFinished {
        conversation: "room-a".into(),
        run: RUN.into(),
        outcome: RunOutcome::Completed,
    });
    let ended = draw(&session, 21);
    assert!(
        ended.contains("∴ Thought") && !ended.contains("∴ Thought for"),
        "{ended}"
    );
}

/// A block a host sends in its snapshot has no timing here: it reads as ended,
/// and a replacement snapshot forgets the timing of the blocks it drops.
#[test]
fn a_block_a_snapshot_holds_reads_as_ended() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing");
    let id = match &session.snapshot().unwrap().items[0] {
        Item::Thinking(block) => block.id.clone(),
        _ => panic!("Expected reasoning"),
    };
    let mut again = snapshot();
    again.active_run = Some(RUN.into());
    again.items = vec![Item::Thinking(Thinking::new(id, "agent", RUN, "Weighing"))];
    session.apply(Event::Snapshot(again));
    let screen = draw(&session, 11);
    assert!(
        screen.contains("∴ Thought") && !screen.contains("∴ Thinking"),
        "{screen}"
    );
}

/// A block a host sends as an item is the turn's output as any other.
#[test]
fn a_block_a_host_sends_goes_above_what_the_turn_has_not_read_and_stays_bounded() {
    let mut session = turn();
    hand(&mut session, "sent");
    let limit = crate::Limits::default().text_bytes.get();
    session.apply(Event::Item {
        conversation: "room-a".into(),
        item: Item::Thinking(Thinking::new(
            "t1",
            "agent",
            RUN,
            format!("{}END", "x".repeat(limit * 2)),
        )),
    });
    assert_eq!(order(&session), ["thinking", "person"]);
    let kept = thinking_texts(&session)[0];
    assert!(kept.len() <= limit, "{}", kept.len());
    assert!(kept.ends_with("END"));
}

/// Another agent's block says nothing of whether this one wrote since its calls.
#[test]
fn another_agent_s_reasoning_hides_no_stop() {
    let mut session = turn();
    hand(&mut session, "sent");
    calls(&mut session, 10, "c1", ToolState::Succeeded);
    started(&mut session, "agent-b:m2");
    thinks_as(&mut session, "agent-b", "agent-b:m2", 11, "B one");
    thinks_at(&mut session, 12, "With the message");
    assert_eq!(session.steering(&"sent".into()), Some(Steering::Read));
}

#[test]
fn a_long_line_of_reasoning_wraps_to_the_view() {
    let mut session = turn();
    let words = vec!["word"; 60].join(" ");
    thinks_at(&mut session, 10, &words);
    writes_at(&mut session, 11, "Words");
    session.toggle_tool();
    let lines = draw(&session, 12);
    let rows = lines
        .lines()
        .filter(|line| line.contains("word word"))
        .count();
    assert!(rows >= 5, "{lines}");
    assert!(
        lines.lines().all(|line| line.chars().count() <= 60),
        "{lines}"
    );
}

/// The block's end is where the view anchors a focused block.
#[test]
fn a_focused_opened_block_is_in_view() {
    let mut session = turn();
    let long: Vec<String> = (0..50).map(|index| format!("line-{index}")).collect();
    thinks_at(&mut session, 10, &long.join("\n"));
    calls(&mut session, 11, "c1", ToolState::Succeeded);
    session.focus_tool(true);
    session.toggle_tool();
    for filler in 0..40 {
        session.apply(Event::Item {
            conversation: "room-a".into(),
            item: Item::Message(Message::new(format!("m{filler}"), "agent", "Filler")),
        });
    }
    assert!(draw(&session, 12).contains("line-49"));
}

/// While it comes its lines show, so it offers nothing yet; opened, it says how
/// to close it.
#[test]
fn a_block_offers_f2_only_when_it_has_something_to_open_or_close() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing");
    assert!(!draw(&session, 11).contains("F2 expand"));
    session.toggle_tool();
    assert!(draw(&session, 11).contains("· F2 collapse"));
    session.toggle_tool();
    writes_at(&mut session, 12, "Words");
    assert!(draw(&session, 13).contains("· F2 expand"));
}

/// A click opens a block F2 does not reach, as the newest call has it: its header
/// offers no F2. In the activity view F2 closes every block.
#[test]
fn an_opened_block_offers_f2_only_when_f2_reaches_it() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing");
    calls(&mut session, 11, "c1", ToolState::Succeeded);
    writes_at(&mut session, 12, "Words");
    let header = |session: &Session, tool_activity: bool| {
        let options = UiOptions {
            tool_activity,
            ..UiOptions::default()
        };
        let frame =
            render::screen_with_options(session, 60, 30, &Renderers::default(), &options, secs(13));
        let row = frame
            .lines
            .iter()
            .position(|line| line.contains("∴ Thought"))
            .unwrap();
        (frame.lines[row].clone(), frame.tool_targets.clone(), row)
    };
    let (_, targets, row) = header(&session, false);
    let target = targets.iter().find(|target| target.row == row).unwrap();
    session.toggle_tool_details(&target.actor, &target.run, &target.call);
    let (opened, _, _) = header(&session, false);
    assert!(draw(&session, 13).contains("Weighing"));
    assert!(!opened.contains("F2"), "{opened}");

    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing");
    calls(&mut session, 11, "c1", ToolState::Succeeded);
    writes_at(&mut session, 12, "Words");
    session.toggle_activity();
    session.focus_tool(false);
    let (opened, _, _) = header(&session, true);
    assert!(opened.contains("F2 collapse"), "{opened}");
}

/// A click opens a block: its header is a target from the bracket on.
#[test]
fn a_block_s_header_is_a_click_target() {
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing");
    writes_at(&mut session, 11, "Words");
    let screen = render::screen_with_options(
        &session,
        60,
        30,
        &Renderers::default(),
        &UiOptions::default(),
        secs(12),
    );
    let row = screen
        .lines
        .iter()
        .position(|line| line.contains("∴ Thought"))
        .unwrap();
    let target = screen
        .tool_targets
        .iter()
        .find(|target| target.row == row)
        .unwrap();
    assert_eq!(target.start, 3);
    assert!(target.end > target.start);
    assert!(target.call.as_str().starts_with("thinking:"));
}

fn hand(session: &mut Session, id: &str) {
    session.apply(Event::Item {
        conversation: "room-a".into(),
        item: Item::Message(Message::new(id, "person:alice", "A message")),
    });
    session.apply(Event::MessageSteered {
        conversation: "room-a".into(),
        message: id.into(),
        run: RUN.into(),
    });
}

/// RMK-1317: reasoning is what the turn writes, so it says when it read.
#[test]
fn reasoning_after_the_calls_goes_below_the_message_it_read() {
    let mut session = turn();
    hand(&mut session, "sent");
    calls(&mut session, 10, "c1", ToolState::Running);
    calls(&mut session, 11, "c1", ToolState::Succeeded);
    assert_eq!(session.steering(&"sent".into()), Some(Steering::Handed));
    thinks_at(&mut session, 12, "Now with the message");
    assert_eq!(order(&session), ["tool", "person", "thinking"]);
    assert_eq!(session.steering(&"sent".into()), Some(Steering::Read));
}

#[test]
fn reasoning_written_without_the_message_stays_above_it() {
    let mut session = turn();
    hand(&mut session, "sent");
    thinks_at(&mut session, 10, "Weighing");
    writes_at(&mut session, 11, "Words");
    assert_eq!(order(&session), ["thinking", "answer", "person"]);
    assert_eq!(session.steering(&"sent".into()), Some(Steering::Handed));
}

/// Handed once the calls ended, its reasoning is the next answer's, written
/// without the message, and the words after it are that same answer.
#[test]
fn words_after_reasoning_are_one_answer_for_a_message_handed_after_the_calls() {
    let mut session = turn();
    calls(&mut session, 10, "c1", ToolState::Running);
    calls(&mut session, 11, "c1", ToolState::Succeeded);
    hand(&mut session, "sent");
    thinks_at(&mut session, 12, "Going on without it");
    writes_at(&mut session, 13, "Words");
    writes_at(&mut session, 14, " go on");
    assert_eq!(order(&session), ["tool", "thinking", "answer", "person"]);
    assert_eq!(session.steering(&"sent".into()), Some(Steering::Handed));
}

/// A click on a block opens that block alone, in the activity view too: the
/// calls below it stay as they were.
#[test]
fn a_click_on_a_block_opens_it_alone_in_the_activity_view() {
    let options = UiOptions {
        tool_activity: true,
        ..UiOptions::default()
    };
    let drawn = |session: &Session| {
        render::screen_with_options(session, 80, 30, &Renderers::default(), &options, secs(20))
    };
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing the ledgers");
    calls(&mut session, 11, "c1", ToolState::Running);
    calls(&mut session, 12, "c1", ToolState::Succeeded);
    calls(&mut session, 13, "c2", ToolState::Running);
    calls(&mut session, 14, "c2", ToolState::Succeeded);
    writes_at(&mut session, 15, "Words");
    let closed = drawn(&session);
    let row = closed
        .lines
        .iter()
        .position(|line| line.contains("∴ Thought"))
        .unwrap();
    let target = closed
        .tool_targets
        .iter()
        .find(|target| target.row == row)
        .unwrap()
        .clone();
    let rows_of_calls = |lines: &[String]| lines.iter().filter(|l| l.contains("lookup")).count();

    session.toggle_activity_target(&target);
    let opened = drawn(&session);
    assert!(
        opened
            .lines
            .iter()
            .any(|line| line.contains("Weighing the ledgers"))
    );
    assert_eq!(
        rows_of_calls(&opened.lines),
        rows_of_calls(&closed.lines),
        "The calls moved:\n{}",
        opened.lines.join("\n")
    );
    assert!(
        !opened.lines.iter().any(|line| line.contains("F2 collapse")),
        "F2 would open everything, not close this:\n{}",
        opened.lines.join("\n")
    );
    let call = (&"agent".into(), &RUN.into(), &"c1".into());
    assert!(
        !session.tool_expanded(call.0, call.1, call.2),
        "A call opened with it"
    );

    session.toggle_activity_target(&target);
    assert!(
        !drawn(&session)
            .lines
            .iter()
            .any(|line| line.contains("Weighing"))
    );
}

/// With every detail open, as F2 does in the activity view, a click closes them
/// as a click on a call does.
#[test]
fn a_click_on_a_block_with_every_detail_open_closes_them() {
    let options = UiOptions {
        tool_activity: true,
        ..UiOptions::default()
    };
    let mut session = turn();
    thinks_at(&mut session, 10, "Weighing the ledgers");
    calls(&mut session, 11, "c1", ToolState::Succeeded);
    writes_at(&mut session, 12, "Words");
    session.toggle_activity();
    let screen =
        render::screen_with_options(&session, 80, 30, &Renderers::default(), &options, secs(20));
    let row = screen
        .lines
        .iter()
        .position(|line| line.contains("∴ Thought"))
        .unwrap();
    let target = screen
        .tool_targets
        .iter()
        .find(|target| target.row == row)
        .unwrap()
        .clone();
    session.toggle_activity_target(&target);
    let call = (&"agent".into(), &RUN.into(), &"c1".into());
    assert!(!session.tool_expanded(call.0, call.1, call.2));
}
