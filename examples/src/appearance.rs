//! Appearance flags belong to this example application, not the terminal library.

use roomkit_tui::{ColorMode, SelectionMode, TerminalStatus, Theme, UiOptions};

pub(crate) const HELP: &str = "Appearance: --theme dark|light|terminal, --no-color, --no-animation, --copy-on-select, --native-selection, --brand NAME, --command-prefix /|$\nStatus: --terminal-status APP, --no-terminal-status";

pub(crate) fn parse(
    args: impl Iterator<Item = String>,
) -> Result<(Vec<String>, UiOptions), String> {
    let mut args = args.peekable();
    let mut remaining = Vec::new();
    let mut options = UiOptions::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--theme" => {
                options.theme = match args.next().as_deref() {
                    Some("dark") => Theme::dark(),
                    Some("light") => Theme::light(),
                    Some("terminal") => Theme::terminal(),
                    _ => return Err("--theme requires dark, light or terminal".into()),
                }
            }
            "--no-color" => options.color = ColorMode::Never,
            "--no-animation" => options.animations = false,
            "--terminal-status" => {
                options.terminal_status = Some(TerminalStatus::new(
                    args.next()
                        .ok_or("--terminal-status requires an application identity")?,
                )?);
            }
            "--no-terminal-status" => options.terminal_status = None,
            "--copy-on-select" => options.selection = SelectionMode::CopyOnSelect,
            "--native-selection" => options.selection = SelectionMode::Native,
            "--brand" => {
                options.branding.name = args.next().ok_or("--brand requires a name")?;
                options.branding.logo.clear();
                options.branding.subtitle.clear();
            }
            "--command-prefix" => {
                options.command_prefix = match args.next().as_deref() {
                    Some("/") => '/',
                    Some("$") => '$',
                    _ => return Err("--command-prefix requires / or $".into()),
                };
            }
            // Preserve values belonging to the other application flags.
            "--database" | "--room" => {
                remaining.push(arg.clone());
                remaining.push(
                    args.next()
                        .ok_or_else(|| format!("{arg} requires a value"))?,
                );
            }
            _ => remaining.push(arg),
        }
    }
    Ok((remaining, options))
}
