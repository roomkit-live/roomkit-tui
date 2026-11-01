//! A deterministic scripted host: it owns its actors, models, commands and
//! a fictional tool. It executes nothing real and sends nothing outside.

mod script;

use std::collections::VecDeque;

use roomkit_tui_protocol::*;

use crate::Profile;

/// Deterministic host state used by both the interactive and headless examples.
#[derive(Debug)]
pub struct Demo {
    pub(crate) snapshot: Snapshot,
    counter: u64,
    pending_permission: Option<Permission>,
    scheduled: VecDeque<Event>,
    /// The reply drafted so far; an interruption stores it, as an engine does.
    streamed: Option<(MessageId, String)>,
    profile: Profile,
    command_prefix: char,
}

impl Demo {
    /// Build a host with explicitly selected domain capabilities.
    pub fn new(profile: Profile) -> Self {
        Self {
            profile,
            command_prefix: '/',
            counter: 0,
            pending_permission: None,
            scheduled: VecDeque::new(),
            streamed: None,
            snapshot: script::snapshot(profile),
        }
    }

    /// Initial host state; the terminal does not construct it.
    pub fn initial(&self) -> Event {
        Event::Snapshot(self.snapshot.clone())
    }

    pub(crate) fn with_command_prefix(mut self, prefix: char) -> Self {
        self.command_prefix = prefix;
        if let Some(Item::Message(welcome)) = self.snapshot.items.first_mut() {
            welcome.text = welcome.text.replace('/', &prefix.to_string());
        }
        self
    }

    /// Process one correlated request, with host-side scope and state checks.
    pub fn handle(&mut self, request: Request) -> Vec<Event> {
        if request.conversation != self.snapshot.conversation {
            return vec![reply(request.id, Some("Unknown conversation"))];
        }
        match request.command {
            Command::Submit { agent, text } if agent == self.snapshot.selected_agent => {
                self.start(request.id, text)
            }
            Command::Submit { .. } => vec![reply(request.id, Some("Unknown agent"))],
            Command::Invoke { name, arguments } => self.invoke(request.id, &name, &arguments),
            Command::Interrupt { run } => self.interrupt(request.id, run),
            Command::Decide {
                permission,
                decision,
            } => self.decide(request.id, &permission, decision),
            Command::SubmitImages { .. } => {
                vec![reply(
                    request.id,
                    Some("This example does not accept images"),
                )]
            }
            _ => vec![reply(
                request.id,
                Some("This example does not support this request"),
            )],
        }
    }

    /// Advance one scripted step. No wall clock or network is needed in checks.
    pub fn advance(&mut self) -> Option<Event> {
        let event = self.scheduled.pop_front()?;
        if matches!(event, Event::RunFinished { .. }) {
            self.snapshot.active_run = None;
        }
        self.track_draft(&event);
        Some(event)
    }

    /// Keep the text of the reply being drafted until it is stored.
    fn track_draft(&mut self, event: &Event) {
        match event {
            Event::TextDelta { message, text, .. } => {
                let draft = self
                    .streamed
                    .get_or_insert_with(|| (message.clone(), String::new()));
                draft.1.push_str(text);
            }
            Event::MessageFinalized { .. } => self.streamed = None,
            _ => {}
        }
    }

    fn invoke(&mut self, id: RequestId, name: &str, arguments: &str) -> Vec<Event> {
        match (name, self.profile) {
            ("help", _) => vec![reply(id, None), self.help()],
            ("model" | "effort", _) => self.configure(id, name, arguments),
            ("inspect", Profile::Assistant) => {
                self.start(id, format!("Inspect record {arguments}"))
            }
            ("order-status", Profile::Orders) => {
                self.start(id, format!("Look up order {arguments}"))
            }
            _ => vec![reply(id, Some("Unknown application command"))],
        }
    }

    fn help(&self) -> Event {
        let text = self
            .snapshot
            .commands
            .iter()
            .map(|c| {
                format!(
                    "{}{} {} — {}",
                    self.command_prefix, c.name, c.arguments, c.description
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.item(Item::Message(Message::new(
            "command-help",
            "assistant",
            text,
        )))
    }

    fn interrupt(&mut self, id: RequestId, run: RunId) -> Vec<Event> {
        if self.snapshot.active_run.as_ref() != Some(&run) {
            return vec![reply(id, Some("Execution is no longer active"))];
        }
        self.scheduled.clear();
        self.snapshot.active_run = None;
        let mut events = vec![reply(id, None)];
        if let Some(permission) = self.pending_permission.take() {
            events.push(self.resolved(permission.id));
        }
        if let Some((draft, text)) = self.streamed.take() {
            let message = Message::new(format!("assistant-{}", self.counter), "assistant", text);
            events.push(Event::MessageFinalized {
                conversation: self.snapshot.conversation.clone(),
                draft,
                message,
            });
        }
        events.push(self.tool_event(run.clone(), ToolState::Cancelled));
        events.push(self.finished(run, RunOutcome::Interrupted));
        events
    }

    fn decide(
        &mut self,
        id: RequestId,
        permission: &PermissionId,
        decision: Decision,
    ) -> Vec<Event> {
        if self.pending_permission.as_ref().map(|p| &p.id) != Some(permission) {
            return vec![reply(id, Some("Permission is no longer pending"))];
        }
        let Some(run) = self.snapshot.active_run.clone() else {
            return vec![reply(id, Some("No active execution"))];
        };
        let mut events = vec![reply(id, None), self.resolved(permission.clone())];
        if let Some(reviewed) = self.pending_permission.take() {
            let record = PermissionDecision::new(reviewed, decision, "person");
            events.push(self.item(Item::Approval(record)));
        }
        if decision == Decision::Allow {
            self.schedule_answer(run);
            return events;
        }
        self.scheduled.clear();
        self.snapshot.active_run = None;
        events.push(self.tool_event(run.clone(), ToolState::Denied));
        events.push(self.finished(run, RunOutcome::Completed));
        events
    }

    fn configure(&mut self, id: RequestId, name: &str, argument: &str) -> Vec<Event> {
        if self.snapshot.active_run.is_some() {
            let reason = "Finish or interrupt the current execution before changing settings";
            return vec![reply(id, Some(reason))];
        }
        if argument.is_empty() {
            let prefix = self.command_prefix;
            let text = format!(
                "Use {prefix}model balanced|fast or {prefix}effort low|medium|high (simulated settings)"
            );
            let conversation = self.snapshot.conversation.clone();
            return vec![reply(id, None), Event::Notice { conversation, text }];
        }
        let Some(model) = &mut self.snapshot.model else {
            return vec![reply(id, Some("Model unavailable"))];
        };
        match (name, argument) {
            ("model", "balanced" | "fast") => {
                model.model = argument.into();
                model.effort = (argument != "fast").then(|| "medium".into());
            }
            ("effort", _) if model.model == "fast" => {
                return vec![reply(
                    id,
                    Some("This simulated model does not support reasoning effort"),
                )];
            }
            ("effort", "low" | "medium" | "high") => model.effort = Some(argument.into()),
            _ => return vec![reply(id, Some("Unsupported model or effort"))],
        }
        let conversation = self.snapshot.conversation.clone();
        let model = model.clone();
        vec![
            reply(id, None),
            Event::ModelChanged {
                conversation,
                model,
            },
        ]
    }

    fn item(&self, item: Item) -> Event {
        let conversation = self.snapshot.conversation.clone();
        Event::Item { conversation, item }
    }

    fn resolved(&self, permission: PermissionId) -> Event {
        let conversation = self.snapshot.conversation.clone();
        Event::PermissionResolved {
            conversation,
            permission,
        }
    }

    fn finished(&self, run: RunId, outcome: RunOutcome) -> Event {
        let conversation = self.snapshot.conversation.clone();
        Event::RunFinished {
            conversation,
            run,
            outcome,
        }
    }
}

pub(crate) fn reply(request: RequestId, error: Option<&str>) -> Event {
    Event::Reply {
        request,
        error: error.map(Rejection::refused),
    }
}
