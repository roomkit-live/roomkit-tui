#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]

use roomkit_tui_protocol::*;

use super::*;
use crate::Session;
use crate::render::{self, Renderers};

#[test]
fn observed_execution_time_advances_through_waits_and_freezes_only_on_completion() {
    use std::time::Duration;
    let mut session = session();
    let start = Event::RunStarted {
        conversation: "room-a".into(),
        run: "timed".into(),
    };
    session.apply_at(start.clone(), Duration::from_secs(10));
    assert_eq!(
        session.run_elapsed(Duration::from_secs(13)),
        Some(Duration::from_secs(3))
    );
    session.apply_at(
        Event::PermissionRequested {
            conversation: "room-a".into(),
            permission: Permission::new("decision", "agent", "timed", "Read record", "Record 42"),
        },
        Duration::from_secs(14),
    );
    assert_eq!(
        session.run_elapsed(Duration::from_secs(25)),
        Some(Duration::from_secs(15))
    );
    let stop = session
        .request(Command::Interrupt {
            run: "timed".into(),
        })
        .unwrap();
    session.sent(&stop);
    session.apply_at(
        Event::Reply {
            request: stop.id,
            error: None,
        },
        Duration::from_secs(26),
    );
    session.apply_at(start.clone(), Duration::from_secs(27)); // Duplicate start must not reset the clock or stopping state.
    assert!(session.interrupting());
    assert!(session.run_outcome().is_none());
    assert_eq!(
        session.run_elapsed(Duration::from_secs(30)),
        Some(Duration::from_secs(20))
    );
    session.apply_at(
        Event::RunFinished {
            conversation: "room-a".into(),
            run: "other".into(),
            outcome: RunOutcome::Completed,
        },
        Duration::from_secs(31),
    );
    assert!(session.run_outcome().is_none());
    let finish = Event::RunFinished {
        conversation: "room-a".into(),
        run: "timed".into(),
        outcome: RunOutcome::Interrupted,
    };
    session.apply_at(finish.clone(), Duration::from_millis(32_250));
    assert_eq!(
        session.run_elapsed(Duration::from_secs(100)),
        Some(Duration::from_millis(22_250))
    );
    assert_eq!(session.run_outcome(), Some(RunOutcome::Interrupted));
    session.apply_at(finish, Duration::from_secs(101));
    session.apply_at(start, Duration::from_secs(102)); // Replay cannot reopen the finished run.
    assert!(session.active_run().is_none());
    assert_eq!(
        session.run_elapsed(Duration::from_secs(103)),
        Some(Duration::from_millis(22_250))
    );
    session.apply_at(
        Event::RunStarted {
            conversation: "room-a".into(),
            run: "next".into(),
        },
        Duration::from_secs(110),
    );
    assert!(session.run_outcome().is_none());
    assert_eq!(
        session.run_elapsed(Duration::from_secs(111)),
        Some(Duration::from_secs(1))
    );
}

#[test]
fn restored_or_untimed_runs_never_invent_a_total_duration() {
    use std::time::Duration;
    let mut session = session();
    session.apply_at(
        Event::RunStarted {
            conversation: "room-a".into(),
            run: "known".into(),
        },
        Duration::from_secs(5),
    );
    let mut restored = snapshot();
    restored.active_run = Some("known".into());
    session.apply_at(Event::Snapshot(restored.clone()), Duration::from_secs(12));
    assert_eq!(
        session.run_elapsed(Duration::from_secs(15)),
        Some(Duration::from_secs(10))
    );
    restored.conversation = "another-room".into();
    session.apply_at(Event::Snapshot(restored), Duration::from_secs(20));
    assert!(session.run_elapsed(Duration::from_secs(25)).is_none());
    session.apply_at(
        Event::RunStarted {
            conversation: "another-room".into(),
            run: "known".into(),
        },
        Duration::from_secs(26),
    );
    assert!(session.run_elapsed(Duration::from_secs(27)).is_none());
    session.apply_at(
        Event::RunFinished {
            conversation: "another-room".into(),
            run: "known".into(),
            outcome: RunOutcome::Completed,
        },
        Duration::from_secs(30),
    );
    assert_eq!(session.run_outcome(), Some(RunOutcome::Completed));
    assert!(session.run_elapsed(Duration::from_secs(31)).is_none());
    session.apply(Event::Snapshot(snapshot()));
    started(&mut session, "untimed");
    session.apply(Event::RunFinished {
        conversation: "room-a".into(),
        run: "untimed".into(),
        outcome: RunOutcome::Failed,
    });
    assert_eq!(session.run_outcome(), Some(RunOutcome::Failed));
    assert!(session.run_elapsed(Duration::from_secs(99)).is_none());
}

#[test]
fn activity_shows_live_and_final_durations_with_space_before_the_composer() {
    use crate::UiOptions;
    use std::time::Duration;
    let mut session = session();
    let renderers = Renderers::default();
    let options = UiOptions {
        animations: false,
        ..Default::default()
    };
    let draw = |session: &Session, now| {
        render::screen_with_options(
            session,
            100,
            24,
            &renderers,
            &options,
            Duration::from_millis(now),
        )
    };
    let ready = draw(&session, 0);
    let row = ready
        .lines
        .iter()
        .position(|line| line.contains("Ready"))
        .unwrap();
    assert_eq!(ready.lines[row + 1], "");
    assert!(ready.spans[row + 2].iter().all(|span| matches!(
        span.tone,
        render::Tone::Input | render::Tone::InputPrompt | render::Tone::Placeholder
    )));
    session.apply_at(
        Event::RunStarted {
            conversation: "room-a".into(),
            run: "timed".into(),
        },
        Duration::from_secs(10),
    );
    assert!(
        draw(&session, 14_200)
            .lines
            .iter()
            .any(|line| line.contains("Working · 4s"))
    );
    assert!(
        draw(&session, 75_000)
            .lines
            .iter()
            .any(|line| line.contains("Working · 1m 05s"))
    );
    assert!(
        draw(&session, 3_675_000)
            .lines
            .iter()
            .any(|line| line.contains("Working · 1h 01m 05s"))
    );
    session.apply_at(
        Event::RunFinished {
            conversation: "room-a".into(),
            run: "timed".into(),
            outcome: RunOutcome::Completed,
        },
        Duration::from_millis(14_250),
    );
    let finished = draw(&session, 20_000);
    assert!(
        finished
            .lines
            .iter()
            .any(|line| line.contains("Execution Completed · 4.2s"))
    );
    assert!(!finished.lines.iter().any(|line| line.contains("Ready")));
    assert_eq!(finished, draw(&session, 200_000));
    let row = finished
        .lines
        .iter()
        .position(|line| line.contains("Execution Completed"))
        .unwrap();
    assert_eq!(finished.lines[row + 1], "");
    assert!(finished.spans[row + 2].iter().all(|span| matches!(
        span.tone,
        render::Tone::Input | render::Tone::InputPrompt | render::Tone::Placeholder
    )));
    for gap in [0, 1, 3, u16::MAX] {
        let options = UiOptions {
            activity_gap: gap,
            ..Default::default()
        };
        for height in [8, 12, 14, 24] {
            let frame = render::screen_with_options(
                &session,
                80,
                height,
                &renderers,
                &options,
                Duration::from_secs(200),
            );
            assert!(frame.lines.len() <= usize::from(height));
            let (_, row) = frame.cursor.unwrap();
            assert!(row < height);
            assert!(frame.spans[usize::from(row)].iter().all(|span| matches!(
                span.tone,
                render::Tone::Input | render::Tone::InputPrompt | render::Tone::Placeholder
            )));
        }
    }
}
