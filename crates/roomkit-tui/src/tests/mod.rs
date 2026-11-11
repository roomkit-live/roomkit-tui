//! Interaction contracts shared by all hosts, exercised through the session
//! and the renderer without a terminal.

mod activity;
mod bounds;
mod commands;
mod composer;
mod estimates;
mod layout;
mod messages;
mod meters;
mod permissions;
mod questions;
mod runs;
mod screens;
mod sidebar;
mod steering;
mod streaming;
mod thinking;
mod timing;
mod tools;
mod unread;
mod usage;

use roomkit_tui_protocol::*;
use serde_json::json;

use crate::Session;

pub(crate) fn snapshot() -> Snapshot {
    {
        let mut snapshot = Snapshot::new("room-a", "Research", Actor::new("agent", "Researcher"));
        snapshot.model = Some(ModelInfo::new("local", "model-a"));
        snapshot.commands = vec![
            CommandSpec::new("summarize", "Summarize a record")
                .with_arguments("[record]")
                .with_choices(vec!["report".into(), "notes".into()]),
        ];
        snapshot.can_interrupt = true;
        snapshot
    }
}

pub(crate) fn session() -> Session {
    let mut session = Session::default();
    session.apply(Event::Snapshot(snapshot()));
    session
}

pub(crate) fn started(session: &mut Session, run: &str) {
    session.apply(Event::RunStarted {
        conversation: "room-a".into(),
        run: run.into(),
        answers: None,
    });
}

pub(crate) fn tool(run: &str, state: ToolState) -> Event {
    Event::Item {
        conversation: "room-a".into(),
        item: Item::Tool {
            actor: "agent".into(),
            run: run.into(),
            call: ToolCall::new(
                "call-1",
                "unknown-domain-tool",
                json!({"query":"example"}),
                state,
            )
            .with_data(json!({"rows":[1,2,3]}))
            .with_text("Three records"),
        },
    }
}

pub(crate) fn image_fixture() -> ImageUpload {
    ImageUpload::png("clipboard.png", "AQIDBA==", 1, 1)
}

pub(crate) fn sidebar_spec() -> SidebarSpec {
    SidebarSpec::new(
        "Session",
        vec![
            SidebarSection::new("Agent", SidebarContent::Agent),
            SidebarSection::new("Context", SidebarContent::Context),
            SidebarSection::new(
                "Records",
                SidebarContent::Entries {
                    entries: (0..30)
                        .map(|i| {
                            SidebarEntry::new(format!("Record {i}"))
                                .with_detail("Example details")
                                .with_tone(SidebarTone::Success)
                                .with_action(SidebarAction::new("summarize", i.to_string()))
                        })
                        .collect(),
                },
            ),
        ],
    )
}
