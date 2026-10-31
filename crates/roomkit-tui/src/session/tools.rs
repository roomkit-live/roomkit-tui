//! Tool detail disclosure and keyboard focus, scoped to call identities so that
//! a stale click or key never affects another call.

use std::time::Duration;

use roomkit_tui_protocol::{ActorId, CallId, Item, RunId};

use super::{Session, ToolKey};

impl Session {
    /// Whether a call's details are expanded, independently of other calls.
    pub fn tool_expanded(&self, actor: &ActorId, run: &RunId, call: &CallId) -> bool {
        self.expanded
            || self
                .expanded_tools
                .contains(&(actor.clone(), run.clone(), call.clone()))
    }

    /// Whether this is the keyboard-focused tool (the newest call by default).
    pub fn tool_focused(&self, actor: &ActorId, run: &RunId, call: &CallId) -> bool {
        let matches = |(a, r, c): &ToolKey| a == actor && r == run && c == call;
        match &self.focused_tool {
            Some(focused) => matches(focused),
            None => self.newest_tool().is_some_and(|key| matches(&key)),
        }
    }

    /// Toggle details for the focused call, leaving other calls unchanged.
    pub fn toggle_tool(&mut self) {
        if let Some((actor, run, call)) = self.focused_tool.clone().or_else(|| self.newest_tool()) {
            self.toggle_tool_details(&actor, &run, &call);
        }
    }

    /// Toggle a visible call by identity. Stale mouse targets cannot affect other calls.
    pub fn toggle_tool_details(&mut self, actor: &ActorId, run: &RunId, call: &CallId) {
        let key = (actor.clone(), run.clone(), call.clone());
        if !self.has_tool(&key) {
            return;
        }
        if self.expanded {
            self.expanded_tools.extend(self.tool_keys());
            self.expanded = false;
        }
        if !self.expanded_tools.remove(&key) {
            self.expanded_tools.insert(key);
        }
    }

    pub(crate) fn toggle_activity_target(&mut self, target: &crate::render::ToolTarget) {
        let key = (
            target.actor.clone(),
            target.run.clone(),
            target.call.clone(),
        );
        if self.is_current(&target.conversation) && self.has_tool(&key) {
            self.focused_tool = Some(key);
            self.toggle_activity();
        }
    }

    pub(crate) fn toggle_activity(&mut self) {
        self.expanded = !self.expanded;
        if !self.expanded {
            self.scroll_to_bottom();
            return;
        }
        if self.focused_tool.is_none() {
            self.focused_tool = self.newest_tool();
        }
        self.scroll = 0;
    }

    /// Move focus between calls and bring the selected card into view.
    pub fn focus_tool(&mut self, backwards: bool) {
        let keys = self.tool_keys();
        let Some(last) = keys.len().checked_sub(1) else {
            return;
        };
        let current = self
            .focused_tool
            .as_ref()
            .and_then(|key| keys.iter().position(|candidate| candidate == key))
            .unwrap_or(last);
        let index = if backwards {
            current.saturating_sub(1)
        } else {
            (current + 1).min(last)
        };
        self.focused_tool = keys.get(index).cloned();
        self.scroll = 0;
    }

    /// Return timeline navigation to the most recent activity.
    pub fn follow_latest(&mut self) {
        self.focused_tool = None;
    }

    /// Leave tool focus and history browsing, following the latest activity.
    /// The current draft and expanded tool details are preserved.
    pub fn scroll_to_bottom(&mut self) {
        self.follow_latest();
        self.scroll = 0;
    }

    /// Whether a tool is running, which animates the view.
    pub(crate) fn has_running_tool(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.items.iter().rev().any(|item| {
                matches!(item, Item::Tool { call, .. } if call.state == roomkit_tui_protocol::ToolState::Running)
            })
        })
    }

    pub(crate) fn has_tool_focus(&self) -> bool {
        self.focused_tool.is_some()
    }

    /// Forget disclosure state of calls no longer retained by the view.
    pub(super) fn forget_missing_tools(&mut self) {
        let keys = self.tool_keys();
        self.expanded_tools.retain(|key| keys.contains(key));
        if self
            .focused_tool
            .as_ref()
            .is_some_and(|key| !keys.contains(key))
        {
            self.focused_tool = None;
        }
    }

    fn tool_items(&self) -> impl DoubleEndedIterator<Item = (&ActorId, &RunId, &CallId)> {
        self.snapshot
            .as_ref()
            .into_iter()
            .flat_map(|s| &s.items)
            .filter_map(|item| match item {
                Item::Tool { actor, run, call } => Some((actor, run, &call.id)),
                _ => None,
            })
    }

    fn newest_tool(&self) -> Option<ToolKey> {
        self.tool_items()
            .next_back()
            .map(|(a, r, c)| (a.clone(), r.clone(), c.clone()))
    }

    fn has_tool(&self, (actor, run, call): &ToolKey) -> bool {
        self.tool_items()
            .any(|(a, r, c)| a == actor && r == run && c == call)
    }

    fn tool_keys(&self) -> Vec<ToolKey> {
        self.tool_items()
            .map(|(a, r, c)| (a.clone(), r.clone(), c.clone()))
            .collect()
    }

    /// Wrapped timeline lines scrolled back from the latest activity.
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// Browse back through history by `lines`, leaving tool focus.
    pub fn scroll_back(&mut self, lines: usize) {
        self.follow_latest();
        self.scroll = self.scroll.saturating_add(lines);
    }

    /// Move towards the latest activity by `lines`.
    pub fn scroll_forward(&mut self, lines: usize) {
        self.follow_latest();
        self.scroll = self.scroll.saturating_sub(lines);
    }

    /// Store the offsets a frame actually displayed, so that scrolling past an
    /// end never requires as many presses back. Call after each render.
    /// `now` is the time the frame is shown; it arms a newly shown approval.
    pub fn sync(&mut self, frame: &crate::render::Frame, now: Duration) {
        if let Some(id) = &frame.approval
            && self
                .approval_drawn
                .as_ref()
                .is_none_or(|(drawn, _)| drawn != id)
        {
            self.approval_drawn = Some((id.clone(), now));
        }
        if let Some(scroll) = frame.scroll {
            self.scroll = scroll;
        }
        if let Some(scroll) = frame.permission_scroll {
            self.permission_scroll = scroll;
        }
        if let Some((id, scroll)) = &frame.panel_scroll
            && let Some(panel) = &mut self.panel
            && panel.spec.id == *id
        {
            panel.scroll = *scroll;
        }
        if let Some(sidebar) = &mut self.sidebar {
            match &frame.sidebar {
                Some(area) => sidebar.scroll = area.scroll,
                None => sidebar.focused = false,
            }
        }
    }

    /// Whether every tool's details are expanded.
    pub fn details_expanded(&self) -> bool {
        self.expanded
    }

    /// Expand or collapse every tool's details.
    pub fn toggle_all_details(&mut self) {
        self.expanded = !self.expanded;
    }
}
