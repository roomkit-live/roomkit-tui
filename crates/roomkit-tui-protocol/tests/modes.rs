//! The rule terminals apply to a host's interaction modes, shared with hosts.
#![allow(clippy::unwrap_used, reason = "Invalid fixtures must fail these tests")]

use roomkit_tui_protocol::limits::{MAX_MODE_LABEL_BYTES, MAX_MODES};
use roomkit_tui_protocol::*;

fn modes(selected: &str, count: usize) -> ModeState {
    let choices = (0..count)
        .map(|n| Choice::new(format!("m{n}"), format!("Mode {n}")))
        .collect();
    ModeState::new("mode", selected, choices)
}

#[test]
fn a_mode_fits_only_beside_its_advertised_command_and_within_bounds() {
    let commands = [CommandSpec::new("mode", "Change the mode")];
    assert!(modes("m0", MAX_MODES).fits(&commands));
    assert!(!modes("m0", MAX_MODES).fits(&[]), "Unadvertised command");
    assert!(
        !modes("m0", MAX_MODES + 1).fits(&commands),
        "Too many modes"
    );
    assert!(
        !modes("missing", 2).fits(&commands),
        "Selection outside the choices"
    );
    assert!(!modes("", 0).fits(&commands), "No choices");

    let mut duplicated = modes("m0", 2);
    duplicated.choices[1].value = "m0".into();
    assert!(!duplicated.fits(&commands), "Duplicate values");
    let mut long = modes("m0", 1);
    long.choices[0].label = "x".repeat(MAX_MODE_LABEL_BYTES + 1);
    assert!(!long.fits(&commands), "Label too long");
}
