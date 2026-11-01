//! What a terminal accepts as a whole snapshot, checked by hosts before they
//! send one.

use roomkit_tui_protocol::SnapshotError::{
    Bounds, Commands, InputActions, Modes, Permissions, UnknownSelectedAgent,
};
use roomkit_tui_protocol::limits::{
    MAX_CHOICES, MAX_COMMAND_DESCRIPTION_BYTES, MAX_INPUT_ACTION_DESCRIPTION_BYTES,
    MAX_INPUT_ACTIONS, MAX_LABEL_BYTES, MAX_PENDING_PERMISSIONS, MAX_PERMISSION_DETAIL_BYTES,
    MAX_PERMISSION_SUMMARY_BYTES, MAX_PERMISSION_TITLE_BYTES, MAX_SNAPSHOT_ENTRIES,
};
use roomkit_tui_protocol::*;

fn permission(id: &str, run: &str) -> Permission {
    Permission::new(id, "agent", run, "Run a command", "cargo test")
}

/// A running conversation using every part the rules check.
fn snapshot() -> Snapshot {
    let mut snapshot = Snapshot::new("room", "Room", Actor::new("agent", "Agent"));
    snapshot.commands = vec![
        CommandSpec::new("mode", "Change the mode"),
        CommandSpec::new("shell", "Run a shell command"),
    ];
    snapshot.mode = Some(ModeState::new(
        "mode",
        "ask",
        vec![Choice::new("ask", "Ask")],
    ));
    snapshot.input_actions = vec![InputAction::new('!', "shell", "Shell")];
    snapshot.active_run = Some(RunId::new("run-1"));
    snapshot.permissions = vec![permission("p1", "run-1")];
    snapshot
}

/// One edit to the valid snapshot.
type Change = fn(&mut Snapshot);

fn refused(change: impl FnOnce(&mut Snapshot)) -> Result<(), SnapshotError> {
    let mut snapshot = snapshot();
    change(&mut snapshot);
    snapshot.validate()
}

fn assert_refused(cases: &[(Change, SnapshotError)]) {
    for (n, (change, expected)) in cases.iter().enumerate() {
        assert_eq!(refused(*change), Err(*expected), "case {n}");
    }
}

fn long(bytes: usize) -> String {
    "x".repeat(bytes)
}

#[test]
fn a_snapshot_using_every_checked_part_is_accepted() {
    assert_eq!(snapshot().validate(), Ok(()));
    assert_eq!(
        Commands.to_string(),
        "Host snapshot has invalid or duplicate commands"
    );
}

#[test]
fn a_snapshot_beyond_the_view_bounds_or_without_its_agent_is_refused() {
    assert_refused(&[
        (
            |s| s.conversation = long(MAX_LABEL_BYTES + 1).into(),
            Bounds,
        ),
        (
            |s| {
                s.actors = (0..=MAX_SNAPSHOT_ENTRIES)
                    .map(|n| Actor::new(format!("a{n}"), "Actor"))
                    .collect();
            },
            Bounds,
        ),
        (
            |s| s.selected_agent = ActorId::new("ghost"),
            UnknownSelectedAgent,
        ),
    ]);
}

#[test]
fn an_invalid_command_or_mode_is_refused() {
    let invalid: [fn(&mut CommandSpec); 6] = [
        |c| c.name = "bad name".into(),
        |c| c.name = String::new(),
        |c| c.description = long(MAX_COMMAND_DESCRIPTION_BYTES + 1),
        |c| c.arguments = long(MAX_LABEL_BYTES + 1),
        |c| c.choices = vec![String::new(); MAX_CHOICES + 1],
        |c| c.choices = vec![long(MAX_LABEL_BYTES + 1)],
    ];
    for (n, change) in invalid.into_iter().enumerate() {
        let result = refused(|s| change(&mut s.commands[1]));
        assert_eq!(result, Err(Commands), "command case {n}");
    }
    assert_refused(&[
        (
            |s| s.commands.push(CommandSpec::new("shell", "Again")),
            Commands,
        ),
        (
            |s| {
                let more = (0..MAX_SNAPSHOT_ENTRIES).map(|n| CommandSpec::new(format!("c{n}"), ""));
                s.commands.extend(more);
            },
            Commands,
        ),
        (|s| s.commands.retain(|c| c.name != "mode"), Modes),
    ]);
}

#[test]
fn a_composer_action_must_be_a_distinct_symbol_for_an_advertised_command() {
    for prefix in ['a', '7', ' ', '\t', '\u{7}', '\\'] {
        let result = refused(|s| s.input_actions[0].prefix = prefix);
        assert_eq!(result, Err(InputActions), "prefix {prefix:?}");
    }
    assert_refused(&[
        (
            |s| {
                s.input_actions
                    .push(InputAction::new('!', "shell", "Again"))
            },
            InputActions,
        ),
        (
            |s| {
                let more = "#$%&*+-=?".chars().take(MAX_INPUT_ACTIONS);
                let more = more.map(|prefix| InputAction::new(prefix, "shell", "More"));
                s.input_actions.extend(more);
            },
            InputActions,
        ),
        (
            |s| s.input_actions[0].description = long(MAX_INPUT_ACTION_DESCRIPTION_BYTES + 1),
            InputActions,
        ),
        (
            |s| s.input_actions[0].command = "missing".into(),
            InputActions,
        ),
    ]);
}

#[test]
fn a_pending_permission_must_belong_to_the_active_run_once() {
    assert_refused(&[
        (
            |s| s.permissions.push(permission("p2", "run-2")),
            Permissions,
        ),
        (|s| s.active_run = None, Permissions),
        (
            |s| s.permissions.push(permission("p1", "run-1")),
            Permissions,
        ),
        (
            |s| {
                s.permissions = (0..=MAX_PENDING_PERMISSIONS)
                    .map(|n| permission(&format!("p{n}"), "run-1"))
                    .collect();
            },
            Permissions,
        ),
        (
            |s| s.permissions[0].detail = long(MAX_PERMISSION_DETAIL_BYTES + 1),
            Permissions,
        ),
    ]);
}

#[test]
fn a_snapshot_breaking_several_rules_reports_the_first_in_order() {
    assert_refused(&[
        (
            |s| {
                s.conversation = long(MAX_LABEL_BYTES + 1).into();
                s.selected_agent = ActorId::new("ghost");
            },
            Bounds,
        ),
        (
            |s| {
                s.selected_agent = ActorId::new("ghost");
                s.commands[1].name = "bad name".into();
            },
            UnknownSelectedAgent,
        ),
        (
            |s| {
                s.commands.push(CommandSpec::new("bad name", "Space"));
                s.mode = Some(ModeState::new("mode", "missing", vec![]));
            },
            Commands,
        ),
        (
            |s| {
                s.commands.retain(|c| c.name != "mode");
                s.input_actions[0].prefix = 'a';
            },
            Modes,
        ),
        (
            |s| {
                s.input_actions[0].prefix = 'a';
                s.active_run = None;
            },
            InputActions,
        ),
    ]);
}

#[test]
fn a_permission_fits_only_when_it_can_be_shown_in_full() {
    assert!(permission("p1", "run-1").fits());
    let oversized: [fn(&mut Permission); 4] = [
        |p| p.id = long(MAX_LABEL_BYTES + 1).into(),
        |p| p.title = long(MAX_PERMISSION_TITLE_BYTES + 1),
        |p| p.detail = long(MAX_PERMISSION_DETAIL_BYTES + 1),
        |p| p.summary = Some(long(MAX_PERMISSION_SUMMARY_BYTES + 1)),
    ];
    for (n, change) in oversized.into_iter().enumerate() {
        let mut permission = permission("p1", "run-1");
        change(&mut permission);
        assert!(!permission.fits(), "case {n}: a person must read it all");
    }
}
