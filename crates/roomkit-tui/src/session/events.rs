//! Apply host events. Each event is ignored outside its conversation; the
//! host remains the authority for history, runs and decisions.

use std::time::Duration;

use roomkit_tui_protocol::limits::{MAX_LABEL_BYTES, MAX_OPERATION_ID_BYTES};
use roomkit_tui_protocol::{
    BackgroundStatus, Choice, Command, ConversationId, Event, FormSpec, ModeState, OperationId,
    PanelSpec, Rejection, RequestId, RunOutcome, SidebarSpec,
};

use super::Session;
use super::bounds::{bounded, valid_mode};
use crate::{ChoicePicker, FormState, OperationObservation, PanelState, SidebarState};

impl Session {
    /// Apply an event, ignoring events for other conversations and stale runs.
    pub fn apply(&mut self, event: Event) {
        self.apply_observed(event, None);
    }

    /// Apply an event at an elapsed monotonic time relative to this view's epoch.
    /// Use the same epoch for rendering and all calls. This measures observation
    /// time, including tools and human waits, not provider inference duration.
    pub fn apply_at(&mut self, event: Event, observed_at: Duration) {
        self.apply_observed(event, Some(observed_at));
    }

    fn apply_observed(&mut self, event: Event, at: Option<Duration>) {
        let observe = self.observer.is_some()
            && matches!(
                &event,
                Event::Snapshot(_)
                    | Event::ModeChanged { .. }
                    | Event::OperationStarted { .. }
                    | Event::OperationFinished { .. }
                    | Event::RunStarted { .. }
                    | Event::RunFinished { .. }
                    | Event::PermissionRequested { .. }
                    | Event::PermissionResolved { .. }
            );
        self.apply_event(event, at);
        if observe && let Some(observer) = &self.observer {
            observer.publish(self.observation());
        }
    }

    fn apply_event(&mut self, event: Event, at: Option<Duration>) {
        match event {
            Event::Snapshot(snapshot) => self.on_snapshot(snapshot),
            Event::OperationStarted {
                conversation,
                operation,
            } => self.on_operation_started(&conversation, operation),
            Event::OperationFinished {
                conversation,
                operation,
                outcome,
            } => self.on_operation_finished(&conversation, &operation, outcome),
            Event::SidebarChanged {
                conversation,
                sidebar,
            } => self.on_sidebar(&conversation, sidebar),
            Event::ApplicationInfo { conversation, text } => {
                self.on_application_info(&conversation, text);
            }
            Event::ExitWarning { conversation, text } => self.on_exit_warning(&conversation, text),
            Event::BackgroundChanged {
                conversation,
                status,
            } => self.on_background(&conversation, status),
            Event::Panel {
                conversation,
                panel,
            } => self.on_panel(&conversation, panel),
            Event::PanelUpdated {
                conversation,
                panel,
            } => self.on_panel_updated(&conversation, panel),
            Event::ModeChanged { conversation, mode } => self.on_mode(&conversation, mode),
            Event::Choices {
                conversation,
                command,
                title,
                choices,
            } => self.on_choices(&conversation, command, title, choices),
            Event::Form { conversation, form } => self.on_form(&conversation, form),
            Event::Reply { request, error } => self.on_reply(request, error),
            Event::Notice { conversation, text } => {
                if self.is_current(&conversation) {
                    self.notice = bounded(&text, self.limits.text_bytes.get());
                }
            }
            Event::TextDelta {
                conversation,
                message,
                actor,
                run,
                text,
            } => self.on_text_delta(&conversation, message, actor, &run, &text),
            Event::MessageFinalized {
                conversation,
                draft,
                message,
            } => self.on_message_finalized(&conversation, &draft, message),
            Event::Item { conversation, item } => self.on_item(&conversation, item),
            Event::RunStarted { conversation, run } => self.on_run_started(&conversation, run, at),
            Event::RunFinished {
                conversation,
                run,
                outcome,
            } => self.on_run_finished(&conversation, &run, outcome, at),
            Event::PermissionRequested {
                conversation,
                permission,
            } => self.on_permission_requested(&conversation, permission),
            Event::PermissionResolved {
                conversation,
                permission,
            } => self.on_permission_resolved(&conversation, &permission),
            Event::ModelChanged {
                conversation,
                model,
            } => self.on_model_changed(&conversation, model),
            Event::UsageUpdated {
                conversation,
                run,
                provider,
                model,
                context,
                usage,
            } => self.on_usage(&conversation, &run, (&provider, &model), context, usage),
            Event::ContextUpdated {
                conversation,
                run,
                provider,
                model,
                usage,
            } => self.on_context(&conversation, &run, (&provider, &model), usage),
            _ => {}
        }
    }

    fn on_operation_started(&mut self, conversation: &ConversationId, operation: OperationId) {
        let id = operation.as_str();
        if !self.is_current(conversation)
            || id.is_empty()
            || id.len() > MAX_OPERATION_ID_BYTES
            || self.is_executing()
        {
            return;
        }
        self.operation = Some(OperationObservation {
            id: operation,
            outcome: None,
        });
        self.observed_run = None;
        // A host action can launch an engine request from a detail panel.
        // Reveal the conversation once that operation has actually started.
        self.panel = None;
        self.picker = None;
        self.form = None;
        self.notice.clear();
    }

    fn on_operation_finished(
        &mut self,
        conversation: &ConversationId,
        operation: &OperationId,
        outcome: RunOutcome,
    ) {
        if self.is_current(conversation)
            && let Some(current) = &mut self.operation
            && current.id == *operation
            && current.outcome.is_none()
        {
            current.outcome = Some(outcome);
        }
    }

    fn on_sidebar(&mut self, conversation: &ConversationId, sidebar: Option<SidebarSpec>) {
        if !self.is_current(conversation) {
            return;
        }
        let Some(spec) = sidebar else {
            self.sidebar = None;
            return;
        };
        let Some(mut next) = SidebarState::new(spec, |name| self.advertises(name)) else {
            return;
        };
        if let Some(old) = &self.sidebar {
            next.scroll = old.scroll;
            next.focused = old.focused;
            // Preserve action identity across process and connection updates.
            if let Some(action) = old.actions().nth(old.selected) {
                let selected = next.actions().position(|a| a == action);
                next.selected = selected.unwrap_or(0);
            }
        }
        self.sidebar = Some(next);
    }

    fn on_background(&mut self, conversation: &ConversationId, status: Option<BackgroundStatus>) {
        if self.is_current(conversation)
            && status.as_ref().is_none_or(|s| {
                s.label.len() <= MAX_LABEL_BYTES
                    && s.arguments.len() <= MAX_LABEL_BYTES
                    && self.advertises(&s.command)
            })
        {
            self.background = status;
        }
    }

    fn on_panel(&mut self, conversation: &ConversationId, panel: PanelSpec) {
        if self.is_current(conversation)
            && self.advertises(&panel.command)
            && let Some(panel) = PanelState::new(panel)
        {
            self.picker = None;
            self.form = None;
            self.panel = Some(panel);
        }
    }

    fn on_panel_updated(&mut self, conversation: &ConversationId, panel: PanelSpec) {
        if self.is_current(conversation)
            && self.advertises(&panel.command)
            && let Some(current) = &mut self.panel
            && current.spec.id == panel.id
            && current.spec.command == panel.command
        {
            current.update(panel);
        }
    }

    fn on_mode(&mut self, conversation: &ConversationId, mode: ModeState) {
        if self.is_current(conversation)
            && let Some(snapshot) = &mut self.snapshot
            && valid_mode(&mode, &snapshot.commands)
        {
            snapshot.mode = Some(mode);
        }
    }

    fn on_choices(
        &mut self,
        conversation: &ConversationId,
        command: String,
        title: String,
        choices: Vec<Choice>,
    ) {
        if self.is_current(conversation) && self.advertises(&command) {
            self.panel = None;
            self.form = None;
            self.picker = Some(ChoicePicker::new(command, title, choices));
        }
    }

    fn on_form(&mut self, conversation: &ConversationId, form: FormSpec) {
        if !self.is_current(conversation) || !self.advertises(&form.command) {
            return;
        }
        match FormState::new(form) {
            Some(form) => {
                self.panel = None;
                self.picker = None;
                self.form = Some(form);
            }
            None => self.notice = "Invalid, oversized or unsupported host form".into(),
        }
    }

    fn on_reply(&mut self, request: RequestId, error: Option<Rejection>) {
        let Some(command) = self.pending.remove(&request) else {
            return;
        };
        let Some(error) = error else {
            self.on_accepted(&command);
            return;
        };
        match command {
            Command::Decide { permission, .. } => {
                self.deciding.remove(&permission);
            }
            Command::Interrupt { .. } => self.interrupting = None,
            _ => {}
        }
        self.notice = bounded(&error.message, self.limits.text_bytes.get());
    }

    fn on_accepted(&mut self, command: &Command) {
        match command {
            Command::SubmitImages { message, .. }
                if self.editor.text() == message.text && self.images == message.images =>
            {
                self.editor.submitted();
                self.images.clear();
                self.draft_epoch = self.draft_epoch.wrapping_add(1);
                self.refresh_menu();
            }
            Command::Invoke { name, .. }
                if self.picker.as_ref().is_some_and(|p| p.command == *name) =>
            {
                self.picker = None;
            }
            Command::SubmitForm { form, .. }
                if self
                    .form
                    .as_ref()
                    .is_some_and(|state| state.spec.id == *form) =>
            {
                self.form = None;
            }
            _ => {}
        }
    }
}
