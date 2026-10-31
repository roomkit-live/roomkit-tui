//! The interactive terminal: it owns the screen and the keyboard, applies host
//! events to a [`Session`] and sends the person's requests.

mod composer;
mod keys;
mod mouse;
mod requests;
mod screen;
#[cfg(test)]
mod tests;

use std::io::{self, IsTerminal};
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event as Input, KeyCode, KeyEvent, KeyEventKind};
use roomkit_tui_protocol::Event;

use crate::clipboard::Clipboard;
use crate::render::{self, Frame, Renderers};
use crate::selection::TextSelection;
use crate::session::Focus;
use crate::{Client, Session, SessionObserver, UiOptions};
use keys::Outcome;
use mouse::shown;
use requests::{interrupt, paste, send};
use screen::Terminal;

const POLL: Duration = Duration::from_millis(25);
const MAX_EVENTS_PER_FRAME: usize = 128;
const QUIT_TIMEOUT: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_millis(100);

/// Why the terminal returned control to its owning application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExitReason {
    /// The user closed the view. The application still owns host cleanup.
    User,
    /// The host closed its event stream.
    HostDisconnected,
}

/// Run a terminal view using the application's connection and presentations.
///
/// This function blocks the calling thread. Run the host independently and
/// retain its cleanup handle. `Esc` dismisses an overlay or requests interruption.
/// `Ctrl+C` clears the draft and requests interruption; pressing it again within
/// two seconds exits. `Ctrl+D` detaches at once, or after a second press when
/// the host has sent an exit warning (for example running processes).
/// Detaching never promises execution cancellation. Errors and panics restore raw mode and the
/// alternate screen.
pub fn run(client: Client, renderers: Renderers) -> io::Result<ExitReason> {
    run_with_options(client, renderers, UiOptions::default())
}

/// Run with application-provided presentation and optional terminal status.
/// Like [`run`], this blocks the calling thread and never owns engine cleanup.
pub fn run_with_options(
    client: Client,
    renderers: Renderers,
    options: UiOptions,
) -> io::Result<ExitReason> {
    run_session(client, renderers, options, Session::default())
}

/// Run with a passive observer supplied by the application.
/// No observer worker or external integration is started by this function.
/// The application owns the consumer and stops it when the terminal returns.
pub fn run_with_observer(
    client: Client,
    renderers: Renderers,
    options: UiOptions,
    observer: SessionObserver,
) -> io::Result<ExitReason> {
    run_session(
        client,
        renderers,
        options,
        Session::default().with_observer(observer),
    )
}

fn run_session(
    client: Client,
    renderers: Renderers,
    options: UiOptions,
    session: Session,
) -> io::Result<ExitReason> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other(
            "Interactive mode requires a terminal on stdin and stdout",
        ));
    }
    let prefix = options.command_prefix;
    View {
        terminal: Terminal::enter(options)?,
        session: session.with_command_prefix(prefix),
        client,
        renderers,
        selection: None,
        clipboard: Clipboard::default(),
        redraw: Redraw::default(),
        frame: None,
        started: Instant::now(),
        edge_scrolled: Duration::ZERO,
    }
    .run()
}

/// The terminal loop's state; one per interactive view.
struct View {
    terminal: Terminal,
    session: Session,
    client: Client,
    renderers: Renderers,
    selection: Option<TextSelection>,
    clipboard: Clipboard,
    redraw: Redraw,
    frame: Option<Frame>,
    started: Instant,
    /// When a selection held past the conversation's edge last scrolled it.
    edge_scrolled: Duration,
}

impl View {
    fn run(mut self) -> io::Result<ExitReason> {
        loop {
            if let Some(reason) = self.drain_events()? {
                return Ok(reason);
            }
            let now = self.now();
            self.redraw.dirty |= self.clipboard.poll(&mut self.session, now);
            self.redraw.dirty |= self.session.expire_quit_hint(now);
            self.redraw.dirty |= self.scroll_at_edge(now);
            self.draw_if_due()?;
            if !event::poll(POLL)? || self.frame.is_none() {
                continue;
            }
            if let Some(reason) = self.input(event::read()?)? {
                return Ok(reason);
            }
        }
    }

    fn now(&self) -> Duration {
        self.started.elapsed()
    }

    /// Apply a bounded batch of host events, so input stays responsive.
    fn drain_events(&mut self) -> io::Result<Option<ExitReason>> {
        for _ in 0..MAX_EVENTS_PER_FRAME {
            match self.client.try_recv() {
                Ok(event) => {
                    if let (
                        Some(selection),
                        Event::MessageFinalized {
                            conversation,
                            draft,
                            message,
                        },
                    ) = (&mut self.selection, &event)
                    {
                        selection.finalized(conversation, draft, message);
                    }
                    let now = self.now();
                    self.session.apply_at(event, now);
                    if let Some(status) = &mut self.terminal.status {
                        status.observe(&mut self.terminal.stdout, &self.session)?;
                    }
                    self.redraw.dirty = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(Some(ExitReason::HostDisconnected)),
            }
        }
        Ok(None)
    }

    /// Timed frames only animate activity; content changes draw at once.
    /// A frozen screen selection shows no activity.
    fn ticking(&self) -> bool {
        !self.frozen()
            && (self.session.is_executing()
                || (self.terminal.options.animations && self.session.has_running_tool()))
    }

    fn draw_if_due(&mut self) -> io::Result<()> {
        let now = self.now();
        if !self.redraw.due(now, self.ticking()) {
            return Ok(());
        }
        let next = self.next_frame(now)?;
        let shown = shown(&next, self.selection.as_ref(), &self.session);
        self.terminal.draw(&shown)?;
        let live = !self.frozen();
        if live
            && self.session.form.is_none()
            && shown.cursor.is_some()
            && let Some(status) = &mut self.terminal.status
        {
            status.drawn();
        }
        if live {
            self.session.sync(&next, now);
        }
        self.frame = Some(next);
        self.redraw.drawn(self.now());
        Ok(())
    }

    /// The next frame: a screen selection's frozen frame, or the live screen,
    /// along which a held conversation selection follows its pointer.
    fn next_frame(&mut self, now: Duration) -> io::Result<Frame> {
        if let Some(frozen) = self.selection.as_ref().and_then(TextSelection::frozen) {
            return Ok(frozen);
        }
        let (width, height) = crossterm::terminal::size()?;
        let next = render::screen_with_options(
            &self.session,
            width,
            height,
            &self.renderers,
            &self.terminal.options,
            now,
        );
        if let Some(selection) = &mut self.selection {
            selection.follow(&next, &self.session);
        }
        Ok(next)
    }

    fn input(&mut self, input: Input) -> io::Result<Option<ExitReason>> {
        match input {
            Input::Key(key) if key.kind != KeyEventKind::Release => return self.key(key),
            Input::Paste(text) => {
                self.terminal.acknowledge_status()?;
                self.redraw.dirty = true;
                self.selection = None;
                self.session.quit_until = None;
                paste(&mut self.session, &text);
            }
            Input::Mouse(mouse)
                if self.terminal.options.selection == crate::SelectionMode::CopyOnSelect =>
            {
                self.mouse(mouse)?;
            }
            Input::Resize(_, _) => {
                self.redraw.dirty = true;
                self.terminal.invalidate();
                self.selection = None;
            }
            _ => {}
        }
        Ok(None)
    }

    fn key(&mut self, key: KeyEvent) -> io::Result<Option<ExitReason>> {
        self.terminal.acknowledge_status()?;
        self.redraw.dirty = true;
        self.selection = None;
        if !keys::is_quit_key(key) {
            self.session.quit_until = None;
        }
        if self.view_key(key) {
            return Ok(None);
        }
        self.clipboard.reset_repeat();
        let now = self.now();
        let outcome = keys::handle_key(
            &mut self.session,
            &self.client,
            key,
            now,
            self.terminal.options.tool_activity,
        );
        Ok((outcome == Outcome::Exit).then_some(ExitReason::User))
    }

    /// Keys that depend on the drawn frame or the terminal rather than on the
    /// session: sidebar layout, an undersized screen and clipboard reads.
    /// Returns whether the key was consumed.
    fn view_key(&mut self, key: KeyEvent) -> bool {
        let Some(frame) = &self.frame else {
            return true;
        };
        let focus = self.session.focus();
        let sidebar_reachable = !matches!(focus, Focus::Permission | Focus::Panel | Focus::Form);
        if key.code == KeyCode::F(7) && self.session.sidebar.is_some() && sidebar_reachable {
            let visible = self
                .session
                .sidebar_visible
                .unwrap_or(self.terminal.options.sidebar_visible);
            self.session.sidebar_visible = Some(!visible);
            if let Some(sidebar) = &mut self.session.sidebar {
                sidebar.focused = false;
            }
            return true;
        }
        if key.code == KeyCode::F(8) && frame.sidebar.is_some() {
            if let Some(sidebar) = &mut self.session.sidebar {
                sidebar.focused = !sidebar.focused;
            }
            return true;
        }
        // An undersized screen shows no input or choices: never act unseen.
        if frame.cursor.is_none()
            && frame.permission_targets.is_empty()
            && (matches!(
                key.code,
                KeyCode::Enter | KeyCode::Tab | KeyCode::Up | KeyCode::Down
            ) || ['y', 'n', 's'].into_iter().any(|c| keys::is_ctrl(key, c)))
        {
            return true;
        }
        if keys::is_ctrl(key, 'v')
            && key.kind == KeyEventKind::Press
            && focus == Focus::Composer
            && !self.session.images_pending()
        {
            let now = self.now();
            self.clipboard.start(&mut self.session, now);
            return true;
        }
        false
    }
}

/// Polling the host/input queues must not imply laying out the transcript.
/// Content changes are immediate; only the clock and spinner need timed frames.
#[derive(Default)]
struct Redraw {
    dirty: bool,
    last_frame: Option<Duration>,
}

impl Redraw {
    fn due(&self, now: Duration, ticking: bool) -> bool {
        self.dirty
            || self
                .last_frame
                .is_none_or(|last| ticking && now.saturating_sub(last) >= TICK)
    }

    fn drawn(&mut self, now: Duration) {
        self.dirty = false;
        self.last_frame = Some(now);
    }
}
