//! The scripted conversation: initial state, a permissioned tool call and a
//! streamed answer.

use roomkit_tui_protocol::*;
use serde_json::json;

use super::{Demo, reply};
use crate::Profile;

pub(super) fn snapshot(profile: Profile) -> Snapshot {
    let (title, command) = match profile {
        Profile::Assistant => ("Conversation example · scripted host", "inspect"),
        Profile::Orders => ("Order support · fictional data", "order-status"),
    };
    let mut snapshot = Snapshot::new("example", title, Actor::new("assistant", "Assistant"));
    snapshot.local_actor = Some("person".into());
    snapshot.actors.insert(0, Actor::new("person", "You"));
    snapshot.model = Some(ModelInfo::new("scripted", "balanced").with_effort("medium"));
    snapshot.commands = vec![
        CommandSpec::new("help", "Show this application's commands"),
        CommandSpec::new("model", "Choose a simulated model")
            .with_arguments("[balanced|fast]")
            .with_choices(vec!["balanced".into(), "fast".into()]),
        CommandSpec::new("effort", "Choose simulated reasoning effort")
            .with_arguments("[low|medium|high]")
            .with_choices(vec!["low".into(), "medium".into(), "high".into()]),
        CommandSpec::new(
            command,
            "Inspect a fictional record, with explicit permission",
        )
        .with_arguments("[reference]")
        .with_choices(vec!["42".into()]),
    ];
    let welcome = format!(
        "This example uses a scripted host and fictional records. No API or external action is involved. Type a message or /{command} 42 to inspect a record; the host will ask for permission. /help lists application commands."
    );
    snapshot.items = vec![Item::Message(Message::new("welcome", "assistant", welcome))];
    snapshot.can_interrupt = true;
    snapshot
}

impl Demo {
    /// The application's sidebar: model settings, the context split into
    /// fictional estimates of the host's fixed parts, and a help link.
    /// The terminal measures the remainder after each usage report.
    pub fn sidebar(&self) -> Event {
        let breakdown = SidebarContent::ContextBreakdown {
            parts: vec![
                MeterSegment::new("System prompt", 1_200, MeterColor::Series(0)).estimated(),
                MeterSegment::new("Tool schemas", 2_400, MeterColor::Series(1)).estimated(),
                MeterSegment::new("Skills", 180, MeterColor::Series(2)).estimated(),
            ],
            remainder: "Messages".into(),
        };
        let help = SidebarEntry::new("Help and commands")
            .with_detail("Fictional records · scripted host")
            .with_tone(SidebarTone::Accent)
            .with_action(SidebarAction::new("help", String::new()));
        let sections = vec![
            SidebarSection::new("Agent / model", SidebarContent::Agent)
                .with_action(SidebarAction::new("model", String::new())),
            SidebarSection::new("Context", breakdown),
            SidebarSection::new(
                "Application",
                SidebarContent::Entries {
                    entries: vec![help],
                },
            ),
        ];
        Event::SidebarChanged {
            conversation: self.snapshot.conversation.clone(),
            sidebar: Some(SidebarSpec::new("Session", sections)),
        }
    }

    pub(super) fn start(&mut self, id: RequestId, text: String) -> Vec<Event> {
        if self.snapshot.active_run.is_some() {
            return vec![reply(id, Some("An execution is already active"))];
        }
        self.counter += 1;
        let run = RunId::new(format!("run-{}", self.counter));
        self.snapshot.active_run = Some(run.clone());
        let permission = Permission::new(
            format!("permission-{}", self.counter),
            "assistant",
            run.clone(),
            "Read fictional example record",
            "Allow this scripted read of record 42? No files, network calls or external messages are involved.",
        )
        .with_summary("Read record 42");
        self.pending_permission = Some(permission.clone());
        let conversation = self.snapshot.conversation.clone();
        let message = Message::new(format!("user-{}", self.counter), "person", text);
        vec![
            reply(id, None),
            self.item(Item::Message(message)),
            Event::RunStarted {
                conversation: conversation.clone(),
                run: run.clone(),
                answers: None,
            },
            self.tool_event(run, ToolState::Pending),
            Event::PermissionRequested {
                conversation,
                permission,
            },
        ]
    }

    pub(super) fn tool_event(&self, run: RunId, state: ToolState) -> Event {
        let name = match self.profile {
            Profile::Assistant => "inspect_record",
            Profile::Orders => "lookup_order",
        };
        let mut call = ToolCall::new("lookup", name, json!({"reference":"42"}), state);
        if state == ToolState::Succeeded {
            call = call
                .with_data(json!({"reference":"42", "status":"ready", "items":3}))
                .with_text("Fictional record found");
        }
        let actor = "assistant".into();
        self.item(Item::Tool { actor, run, call })
    }

    pub(super) fn schedule_answer(&mut self, run: RunId) {
        let conversation = self.snapshot.conversation.clone();
        self.scheduled
            .push_back(self.tool_event(run.clone(), ToolState::Running));
        self.scheduled
            .push_back(self.tool_event(run.clone(), ToolState::Succeeded));
        let text = match self.profile {
            Profile::Assistant => {
                "The fictional record is ready. This response, tool, permission and model selection all come from the host application."
            }
            Profile::Orders => {
                "Order 42 is ready with three items. This order-support application uses the same terminal, with its own tool presentation and slash command."
            }
        };
        // Like an engine, stream a draft, then store it under its own id.
        let draft = MessageId::new(format!("draft:{}:0", run.as_str()));
        for word in text.split_inclusive(' ') {
            self.scheduled.push_back(Event::TextDelta {
                conversation: conversation.clone(),
                message: draft.clone(),
                actor: "assistant".into(),
                run: run.clone(),
                text: word.into(),
            });
        }
        let answer = Message::new(format!("assistant-{}", self.counter), "assistant", text);
        self.scheduled.push_back(Event::MessageFinalized {
            conversation: conversation.clone(),
            draft,
            message: answer,
        });
        if let Some(model) = &self.snapshot.model {
            self.scheduled.push_back(Event::ContextUpdated {
                conversation,
                run: run.clone(),
                provider: model.provider.clone(),
                model: model.model.clone(),
                usage: ContextUsage::new(4096).with_capacity(32_768).estimated(),
            });
        }
        self.scheduled
            .push_back(self.finished(run, RunOutcome::Completed));
    }
}
