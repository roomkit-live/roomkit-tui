//! Optional OSC 7501 output, owned by the same thread and writer as the terminal.

use std::io::{self, Write};

use roomkit_tui_protocol::RunOutcome;

use crate::Session;

/// Opt into OSC 7501 program status for this terminal's root record.
///
/// The application supplies its stable identity. No environment detection,
/// terminal access or worker is started by construction. Assign to
/// [`crate::UiOptions::terminal_status`] to enable reporting in interactive mode.
/// Reports contain only this identity, a state and the permission kind.
///
/// ```
/// use roomkit_tui::{TerminalStatus, UiOptions};
///
/// let mut options = UiOptions::default();
/// options.terminal_status = Some(TerminalStatus::new("research-desk")?);
/// // Supply options to run_with_options or run_with_observer.
/// # Ok::<(), &'static str>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalStatus {
    app: String,
}

impl TerminalStatus {
    /// Validate an OSC 7501 application identity: 1–32 ASCII letters, digits,
    /// underscores, dots, plus signs or hyphens. Invalid values return an error
    /// rather than being truncated or interpreted as terminal controls.
    pub fn new(app: impl Into<String>) -> Result<Self, &'static str> {
        let app = app.into();
        if !(1..=32).contains(&app.len())
            || !app
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
        {
            return Err(
                "Terminal status app must contain 1–32 ASCII letters, digits, _, ., + or -",
            );
        }
        Ok(Self { app })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Working,
    Permission,
    Done,
    Error,
}

impl State {
    fn from_session(session: &Session) -> Option<Self> {
        session.snapshot.as_ref()?;
        Some(if !session.permissions().is_empty() {
            Self::Permission
        } else if session.is_executing() {
            Self::Working
        } else {
            match session.execution_outcome() {
                Some(RunOutcome::Completed) => Self::Done,
                Some(RunOutcome::Failed | RunOutcome::Unknown) => Self::Error,
                _ => Self::Idle,
            }
        })
    }

    fn pairs(self) -> &'static str {
        match self {
            Self::Idle => "state=idle",
            Self::Working => "state=working",
            Self::Permission => "state=blocked:kind=permission",
            Self::Done => "state=done",
            Self::Error => "state=error",
        }
    }

    fn is_result(self) -> bool {
        matches!(self, Self::Done | Self::Error)
    }
}

/// Keeps presentation acknowledgment separate from the host's confirmed outcome.
pub(crate) struct Reporter {
    config: TerminalStatus,
    source: Option<State>,
    sent: Option<State>,
    acknowledged: bool,
    visible: bool,
    reported: bool,
}

impl Reporter {
    pub fn new(config: TerminalStatus) -> Self {
        Self {
            config,
            source: None,
            sent: None,
            acknowledged: false,
            visible: false,
            reported: false,
        }
    }

    /// Called after each projected host event, independently of frame deduplication.
    pub fn observe(&mut self, writer: &mut impl Write, session: &Session) -> io::Result<()> {
        let state = State::from_session(session);
        if self.source != state {
            self.source = state;
            self.acknowledged = false;
            self.visible = false;
        }
        if let Some(state) = state {
            self.send(
                writer,
                if self.acknowledged {
                    State::Idle
                } else {
                    state
                },
            )?;
        }
        Ok(())
    }

    /// Only a successfully rendered, unfrozen result can be acknowledged.
    pub fn drawn(&mut self) {
        self.visible = self.sent.is_some_and(State::is_result);
    }

    /// User input dismisses a presented result, never a permission or active work.
    pub fn acknowledge(&mut self, writer: &mut impl Write) -> io::Result<()> {
        if self.visible && self.source.is_some_and(State::is_result) {
            self.acknowledged = true;
            self.visible = false;
            self.send(writer, State::Idle)?;
        }
        Ok(())
    }

    fn send(&mut self, writer: &mut impl Write, state: State) -> io::Result<()> {
        if self.sent != Some(state) {
            // Even a partially written first report needs a cleanup attempt.
            self.reported = true;
            self.write(writer, state.pairs())?;
            self.sent = Some(state);
        }
        Ok(())
    }

    fn write(&self, writer: &mut impl Write, pairs: &str) -> io::Result<()> {
        writer.write_all(format!("\x1b]7501;{pairs}:app={}\x1b\\", self.config.app).as_bytes())?;
        writer.flush()
    }

    /// Best-effort cleanup is invoked by the terminal's existing drop guard.
    pub fn clear(&mut self, writer: &mut impl Write) -> io::Result<()> {
        if self.reported {
            self.write(writer, "state=clear")?;
            self.reported = false;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
