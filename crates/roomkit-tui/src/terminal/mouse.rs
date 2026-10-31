//! Mouse selection, clicks and wheel scrolling. Every click target belongs to
//! the frame it was drawn in and is checked against the current session. A
//! selection in the conversation is painted on the live screen and scrolls
//! with its text; any other one freezes the frame it was made in.

use std::borrow::Cow;
use std::io::{self, Write};
use std::time::Duration;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use roomkit_tui_protocol::Command;

use super::{View, send};
use crate::Session;
use crate::render::{self, Frame};
use crate::selection::{Edge, TextSelection, clipboard_sequence};
use crate::session::Focus;

const WHEEL_LINES: usize = 3;
const MAX_SCROLL: usize = 65_536;
/// Pace of scrolling while a selection is held past the conversation's edge.
const EDGE_SCROLL: Duration = Duration::from_millis(50);

impl View {
    pub(super) fn mouse(&mut self, mouse: MouseEvent) -> io::Result<()> {
        let pressed = matches!(
            mouse.kind,
            MouseEventKind::Down(_)
                | MouseEventKind::ScrollUp
                | MouseEventKind::ScrollDown
                | MouseEventKind::ScrollLeft
                | MouseEventKind::ScrollRight
        );
        // Moving the pointer is not an answer to "press Ctrl+C again".
        if pressed {
            self.terminal.acknowledge_status()?;
            if self.session.quit_until.take().is_some() {
                self.redraw.dirty = true;
            }
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.press(mouse),
            MouseEventKind::Drag(MouseButton::Left) => self.drag(mouse),
            MouseEventKind::Up(MouseButton::Left) => return self.lift(mouse),
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                self.redraw.dirty = true;
                // Only a selection anchored in the conversation scrolls with it.
                if !self.selection.as_ref().is_some_and(TextSelection::scrolls) {
                    self.selection = None;
                }
                self.wheel(mouse);
            }
            _ => {}
        }
        Ok(())
    }

    /// Whether a screen selection holds the screen still.
    pub(super) fn frozen(&self) -> bool {
        self.selection
            .as_ref()
            .is_some_and(|selection| !selection.scrolls())
    }

    /// A press starts a selection in the frame on screen, replacing any other.
    fn press(&mut self, mouse: MouseEvent) {
        self.redraw.dirty = true;
        if let Some(frame) = &self.frame {
            self.selection = Some(TextSelection::new(
                frame.clone(),
                mouse.column,
                mouse.row,
                &self.session,
            ));
        }
    }

    fn drag(&mut self, mouse: MouseEvent) {
        self.redraw.dirty = true;
        if let (Some(selection), Some(frame)) = (&mut self.selection, &self.frame)
            && selection.held()
        {
            selection.hold(mouse.column, mouse.row, frame, &self.session);
        }
    }

    /// Releasing the button clicks or copies. A copied conversation selection
    /// stays highlighted until the next press, key, paste or resize.
    fn lift(&mut self, mouse: MouseEvent) -> io::Result<()> {
        self.redraw.dirty = true;
        let Some(mut selected) = self.selection.take_if(|selection| selection.held()) else {
            return Ok(());
        };
        if let Some(frame) = &self.frame {
            selected.hold(mouse.column, mouse.row, frame, &self.session);
        }
        selected.release();
        self.release(&selected)?;
        if selected.scrolls() && selected.dragged() {
            self.selection = Some(selected);
        }
        Ok(())
    }

    /// While a held conversation selection points past the conversation's
    /// edge, scroll one line towards it per interval. Returns whether it did.
    pub(super) fn scroll_at_edge(&mut self, now: Duration) -> bool {
        let edge = self
            .selection
            .as_ref()
            .zip(self.frame.as_ref())
            .and_then(|(selection, frame)| selection.edge(frame));
        let Some(edge) = edge else {
            return false;
        };
        if now.saturating_sub(self.edge_scrolled) < EDGE_SCROLL {
            return false;
        }
        self.edge_scrolled = now;
        let before = self.session.scroll();
        match edge {
            Edge::Above => self.session.scroll_back(1),
            Edge::Below => self.session.scroll_forward(1),
        }
        self.session.scroll() != before
    }

    /// A click activates the target under it; a drag copies its text.
    fn release(&mut self, selected: &TextSelection) -> io::Result<()> {
        let now = self.now();
        if let Some(target) = selected.permission_click() {
            let current = self
                .session
                .permissions()
                .first()
                .is_some_and(|p| p.id == target.permission);
            if self.session.is_current(&target.conversation)
                && current
                && !self.session.deciding(&target.permission)
                && self.session.approval_armed(now)
            {
                let command = Command::Decide {
                    permission: target.permission.clone(),
                    decision: target.decision,
                };
                send(&mut self.session, &self.client, command, false);
            }
            return Ok(());
        }
        if let Some(target) = selected.action_click() {
            if let Some(command) = self.session.action_command(target) {
                send(&mut self.session, &self.client, command, false);
            }
            return Ok(());
        }
        if let Some(target) = selected.back_to_bottom_click() {
            if self.session.is_current(&target.conversation) {
                self.session.scroll_to_bottom();
            }
            return Ok(());
        }
        if let Some(target) = selected.tool_click() {
            if self.session.is_current(&target.conversation) {
                if self.terminal.options.tool_activity {
                    self.session.toggle_activity_target(target);
                } else {
                    self.session
                        .toggle_tool_details(&target.actor, &target.run, &target.call);
                }
            }
            return Ok(());
        }
        self.copy(selected, now)
    }

    /// Ask the terminal to put the selected text on the clipboard. Lines
    /// off screen are laid out again as the conversation shows them.
    fn copy(&mut self, selected: &TextSelection, now: Duration) -> io::Result<()> {
        let text = selected.text(&self.session, |items, width| {
            let options = &self.terminal.options;
            render::timeline_text(&self.session, &self.renderers, options, now, width, items)
        });
        match text {
            Ok(Some(text)) => {
                self.terminal
                    .stdout
                    .write_all(clipboard_sequence(&text).as_bytes())?;
                self.terminal.stdout.flush()?;
                self.session.notice =
                    "Copy requested · clipboard access is controlled by your terminal".into();
            }
            Ok(None) => {}
            Err(message) => self.session.notice = message.into(),
        }
        Ok(())
    }

    fn wheel(&mut self, mouse: MouseEvent) {
        let backwards = mouse.kind == MouseEventKind::ScrollUp;
        let over_sidebar = self
            .frame
            .as_ref()
            .and_then(|f| f.sidebar.as_ref())
            .is_some_and(|area| {
                usize::from(mouse.column) >= area.start && usize::from(mouse.row) < area.height
            });
        let step = |value: usize, up: bool| {
            if up {
                value.saturating_sub(WHEEL_LINES)
            } else {
                value.saturating_add(WHEEL_LINES).min(MAX_SCROLL)
            }
        };
        if over_sidebar {
            if let Some(sidebar) = &mut self.session.sidebar {
                sidebar.focused = false;
                sidebar.scroll = step(sidebar.scroll, backwards);
            }
            return;
        }
        match self.session.focus() {
            Focus::Permission => {
                self.session.permission_scroll = step(self.session.permission_scroll, backwards);
            }
            Focus::Panel => {
                if let Some(panel) = &mut self.session.panel {
                    panel.scroll = step(panel.scroll, backwards);
                }
            }
            _ if backwards => self.session.scroll_back(WHEEL_LINES),
            _ => self.session.scroll_forward(WHEEL_LINES),
        }
    }
}

/// What the terminal shows of `frame`: a conversation selection is painted
/// on a copy, so the kept frame stays free of it for the next press.
pub(super) fn shown<'a>(
    frame: &'a Frame,
    selection: Option<&TextSelection>,
    session: &Session,
) -> Cow<'a, Frame> {
    match selection.filter(|selection| selection.scrolls()) {
        Some(selection) => {
            let mut shown = frame.clone();
            selection.paint(&mut shown, session);
            Cow::Owned(shown)
        }
        None => Cow::Borrowed(frame),
    }
}
