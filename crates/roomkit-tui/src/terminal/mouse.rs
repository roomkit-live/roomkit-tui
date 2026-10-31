//! Mouse selection, clicks and wheel scrolling. Every click target belongs to
//! the frame it was drawn in and is checked against the current session.

use std::io::{self, Write};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use roomkit_tui_protocol::Command;

use super::{View, send};
use crate::selection::{TextSelection, clipboard_sequence};
use crate::session::Focus;

const WHEEL_LINES: usize = 3;
const MAX_SCROLL: usize = 65_536;

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
            MouseEventKind::Down(MouseButton::Left) => {
                self.redraw.dirty = true;
                if let Some(frame) = &self.frame {
                    self.selection =
                        Some(TextSelection::new(frame.clone(), mouse.column, mouse.row));
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                self.redraw.dirty = true;
                if let Some(selection) = &mut self.selection {
                    selection.extend(mouse.column, mouse.row);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.redraw.dirty = true;
                if let Some(mut selected) = self.selection.take() {
                    selected.extend(mouse.column, mouse.row);
                    self.release(&selected)?;
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                self.redraw.dirty = true;
                self.selection = None;
                self.wheel(mouse);
            }
            _ => {}
        }
        Ok(())
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
        match selected.text() {
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
