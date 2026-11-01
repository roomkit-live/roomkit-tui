//! What a terminal accepts as a whole snapshot, checked by hosts before they
//! send one.

use roomkit_tui_protocol::limits::{MAX_LABEL_BYTES, MAX_PERMISSION_DETAIL_BYTES};
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

#[test]
fn a_snapshot_is_refused_for_the_first_rule_it_breaks() {
    assert_eq!(snapshot().validate(), Ok(()));
    let cases: [(Change, SnapshotError); 9] = [
        (
            |s| s.conversation = "r".repeat(MAX_LABEL_BYTES + 1).into(),
            SnapshotError::Bounds,
        ),
        (
            |s| s.selected_agent = ActorId::new("ghost"),
            SnapshotError::UnknownSelectedAgent,
        ),
        (
            |s| s.commands.push(CommandSpec::new("bad name", "Space")),
            SnapshotError::Commands,
        ),
        (
            |s| s.commands.push(CommandSpec::new("shell", "Again")),
            SnapshotError::Commands,
        ),
        (
            |s| s.commands.retain(|c| c.name != "mode"),
            SnapshotError::Modes,
        ),
        (
            |s| s.input_actions[0].prefix = 'a',
            SnapshotError::InputActions,
        ),
        (
            |s| s.input_actions[0].command = "missing".into(),
            SnapshotError::InputActions,
        ),
        (
            |s| s.permissions.push(permission("p2", "run-2")),
            SnapshotError::Permissions,
        ),
        (
            |s| s.permissions.push(permission("p1", "run-1")),
            SnapshotError::Permissions,
        ),
    ];
    for (n, (change, expected)) in cases.into_iter().enumerate() {
        assert_eq!(refused(change), Err(expected), "case {n}");
    }
    assert_eq!(
        SnapshotError::Commands.to_string(),
        "Host snapshot has invalid or duplicate commands"
    );
}

#[test]
fn a_permission_fits_only_when_it_can_be_shown_in_full() {
    assert!(permission("p1", "run-1").fits());
    let mut long = permission("p1", "run-1");
    long.detail = "x".repeat(MAX_PERMISSION_DETAIL_BYTES + 1);
    assert!(!long.fits(), "A person must read everything they approve");
    assert_eq!(
        refused(|s| s.permissions[0] = long),
        Err(SnapshotError::Permissions)
    );
}
