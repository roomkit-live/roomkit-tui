//! Tests of the serialized boundary and measurement semantics.
#![allow(
    clippy::unwrap_used,
    reason = "Invalid wire fixtures must fail these tests"
)]

use roomkit_tui_protocol::*;
use serde_json::json;

fn round_trip<T>(value: &T) -> serde_json::Value
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let encoded = serde_json::to_value(value).unwrap();
    assert_eq!(
        &serde_json::from_value::<T>(encoded.clone()).unwrap(),
        value
    );
    encoded
}

fn request(command: Command) -> Request {
    Request::new(RequestId::new(7), "operation-7", "room-2", command)
}

#[test]
fn every_command_round_trips_with_its_conversation_and_correlation() {
    let commands = [
        (
            Command::Submit {
                agent: "agent".into(),
                text: "Hello".into(),
            },
            "submit",
        ),
        (
            Command::SubmitImages {
                agent: "agent".into(),
                message: Box::new(ImageMessage::new(
                    "Describe",
                    vec![ImageUpload::png("clipboard.png", "AQID", 1, 1)],
                )),
            },
            "submit_images",
        ),
        (
            Command::Answer {
                agent: "agent".into(),
            },
            "answer",
        ),
        (
            Command::Invoke {
                name: "model".into(),
                arguments: "fast".into(),
            },
            "invoke",
        ),
        (
            Command::SubmitForm {
                name: "configure".into(),
                form: "revision".into(),
                values: Box::new([("key", "value")].into_iter().collect()),
            },
            "submit_form",
        ),
        (Command::Interrupt { run: "run".into() }, "interrupt"),
        (
            Command::Decide {
                permission: "decision-4".into(),
                decision: Decision::Deny,
            },
            "decide",
        ),
    ];
    for (command, kind) in commands {
        let encoded = round_trip(&request(command));
        assert_eq!(encoded["id"], 7);
        assert_eq!(encoded["operation"], "operation-7");
        assert_eq!(encoded["conversation"], "room-2");
        assert_eq!(encoded["command"]["kind"], kind);
    }
}

#[test]
fn every_event_round_trips() {
    let permission = Permission::new("p", "agent", "run", "Change?", "Exact change")
        .with_summary("Changed a file");
    let mut snapshot = Snapshot::new("room", "Title", Actor::new("agent", "Agent"));
    snapshot.commands = vec![CommandSpec::new("model", "Switch").with_arguments("[model]")];
    snapshot.items = vec![
        Item::Message(Message::new("m", "agent", "Hello")),
        Item::Tool {
            actor: "agent".into(),
            run: "run".into(),
            call: ToolCall::new("c", "lookup", json!({"id": 1}), ToolState::Succeeded)
                .with_text("ready")
                .with_data(json!({"status": "ready"}))
                .with_duration_ms(12),
        },
        Item::Approval(PermissionDecision::new(
            permission.clone(),
            Decision::Allow,
            "person:alice",
        )),
    ];
    let room = ConversationId::from("room");
    let events = [
        Event::Snapshot(snapshot),
        Event::SidebarChanged {
            conversation: room.clone(),
            sidebar: Some(SidebarSpec::new(
                "Details",
                vec![
                    SidebarSection::new("Agent", SidebarContent::Agent),
                    SidebarSection::new(
                        "Records",
                        SidebarContent::Entries {
                            entries: vec![SidebarEntry::new("42").with_tone(SidebarTone::Accent)],
                        },
                    ),
                ],
            )),
        },
        Event::OperationStarted {
            conversation: room.clone(),
            operation: "op".into(),
        },
        Event::OperationFinished {
            conversation: room.clone(),
            operation: "op".into(),
            outcome: RunOutcome::Completed,
        },
        Event::ApplicationInfo {
            conversation: room.clone(),
            text: Some("v1".into()),
        },
        Event::TitleChanged {
            conversation: room.clone(),
            title: "Renamed".into(),
        },
        Event::ExitWarning {
            conversation: room.clone(),
            text: Some("1 running process will be stopped".into()),
        },
        Event::BackgroundChanged {
            conversation: room.clone(),
            status: Some(BackgroundStatus::new("1 job", "jobs", "")),
        },
        Event::Panel {
            conversation: room.clone(),
            panel: PanelSpec::new("panel", "jobs", "Jobs", "Body")
                .with_actions(vec![Choice::new("stop", "Stop")]),
        },
        Event::Form {
            conversation: room.clone(),
            form: FormSpec::new(
                "rev",
                "configure",
                "Configure",
                vec![FormField::new("key", "Key", FieldKind::Secret).required()],
            ),
        },
        Event::Reply {
            request: RequestId::new(3),
            error: Some(Rejection::busy("Busy")),
        },
        Event::RunFinished {
            conversation: room.clone(),
            run: "run".into(),
            outcome: RunOutcome::Interrupted,
        },
        Event::PermissionRequested {
            conversation: room.clone(),
            permission,
        },
        Event::UsageUpdated {
            conversation: room.clone(),
            run: "run".into(),
            provider: "openai".into(),
            model: "model".into(),
            context: Some(ContextUsage::new(10).with_capacity(100)),
            usage: TokenUsage::complete(8, 2, 1),
        },
        Event::CostUpdated {
            conversation: room.clone(),
            cost: ConversationCost::new(2_870, "USD", false),
        },
        Event::RelatedCostUpdated {
            conversation: room.clone(),
            cost: Some(ConversationCost::new(880_000, "USD", true)),
        },
        Event::ContextExceeded {
            conversation: room.clone(),
            run: "run".into(),
        },
        Event::ContextEstimated {
            conversation: room.clone(),
            provider: "openai".into(),
            model: "model".into(),
            usage: ContextUsage::new(92_000).with_capacity(200_000).estimated(),
        },
        Event::Notice {
            conversation: room,
            text: "Check the provider".into(),
        },
    ];
    for event in events {
        round_trip(&event);
    }
}

#[test]
fn rejections_carry_a_stable_code_and_display_text() {
    let encoded = round_trip(&Event::Reply {
        request: RequestId::new(1),
        error: Some(Rejection::busy("A request is still executing")),
    });
    assert_eq!(encoded["error"]["code"], "busy");
    assert_eq!(
        Rejection::from("Plain host text").code,
        ErrorCode::Refused,
        "Application text is a policy refusal"
    );
    let future: Rejection =
        serde_json::from_value(json!({"code": "quota_exceeded", "message": "Later"})).unwrap();
    assert_eq!(future.code, ErrorCode::Other);
}

#[test]
fn an_older_peer_ignores_what_a_newer_one_adds() {
    let event: Event =
        serde_json::from_value(json!({"kind": "timeline_folded", "conversation": "room"})).unwrap();
    assert_eq!(event, Event::Unknown);
    let item: Item = serde_json::from_value(json!({"kind": "poll", "question": "?"})).unwrap();
    assert_eq!(item, Item::Unknown);
    let command: Command = serde_json::from_value(json!({"kind": "undo", "steps": 2})).unwrap();
    assert_eq!(command, Command::Unknown);
    let state: ToolState = serde_json::from_value(json!("queued_remotely")).unwrap();
    assert_eq!(state, ToolState::Unknown);
    let outcome: RunOutcome = serde_json::from_value(json!("timed_out")).unwrap();
    assert_eq!(outcome, RunOutcome::Unknown);
    let content: SidebarContent =
        serde_json::from_value(json!({"kind": "chart", "points": [1, 2]})).unwrap();
    assert_eq!(content, SidebarContent::Unknown);
    let tone: SidebarTone = serde_json::from_value(json!("critical")).unwrap();
    assert_eq!(tone, SidebarTone::Neutral);
    let field: FieldKind = serde_json::from_value(json!({"kind": "date"})).unwrap();
    assert_eq!(field, FieldKind::Unknown);
    let mut extended = serde_json::to_value(Message::new("m", "agent", "Hi")).unwrap();
    extended["reactions"] = json!(["+1"]);
    assert!(serde_json::from_value::<Message>(extended).is_ok());
}

#[test]
fn an_unknown_decision_is_never_read_as_an_answer() {
    let encoded = json!({
        "id": 1, "operation": "op", "conversation": "room",
        "command": {"kind": "decide", "permission": "p", "decision": "allow_always"}
    });
    assert!(serde_json::from_value::<Request>(encoded).is_err());
}

#[test]
fn a_title_change_keeps_its_wire_shape() {
    let encoded = round_trip(&Event::TitleChanged {
        conversation: "room".into(),
        title: "Release notes".into(),
    });
    assert_eq!(
        encoded,
        json!({"kind": "title_changed", "conversation": "room", "title": "Release notes"})
    );
}

#[test]
fn entries_keep_their_wire_shape() {
    let encoded = round_trip(&SidebarContent::Entries {
        entries: vec![SidebarEntry::new("label")],
    });
    assert_eq!(encoded["kind"], "entries");
    assert_eq!(encoded["entries"][0]["label"], "label");
    assert_eq!(round_trip(&SidebarContent::Agent), json!({"kind": "agent"}));
}

#[test]
fn meters_keep_their_wire_shape() {
    let meter = SidebarContent::Meter {
        segments: vec![
            MeterSegment::new("Images", 921, MeterColor::Series(2)),
            MeterSegment::new("Other", 40, MeterColor::Muted).estimated(),
        ],
        capacity: Some(4096),
    };
    let encoded = round_trip(&meter);
    assert_eq!(
        encoded,
        json!({"kind": "meter", "capacity": 4096, "segments": [
            {"label": "Images", "value": 921, "color": {"series": 2}, "estimated": false},
            {"label": "Other", "value": 40, "color": "muted", "estimated": true},
        ]})
    );
    let breakdown = SidebarContent::ContextBreakdown {
        parts: vec![MeterSegment::new("Tools", 9_900, MeterColor::Series(0)).estimated()],
        remainder: "Messages".into(),
    };
    let encoded = round_trip(&breakdown);
    assert_eq!(encoded["kind"], "context_breakdown");
    assert_eq!(encoded["remainder"], "Messages");
    let without_capacity: SidebarContent =
        serde_json::from_value(json!({"kind": "meter", "segments": []})).unwrap();
    assert_eq!(
        without_capacity,
        SidebarContent::Meter {
            segments: Vec::new(),
            capacity: None
        }
    );
    let context: SidebarContent = serde_json::from_value(json!({"kind": "context"})).unwrap();
    assert_eq!(context, SidebarContent::Context);
}

#[test]
fn a_meter_color_from_a_newer_host_reads_as_muted() {
    for future in [
        json!("gradient"),
        json!({"rgb": [1, 2, 3]}),
        json!({"series": 300}),
        json!(4),
        json!(null),
    ] {
        let color: MeterColor = serde_json::from_value(future.clone()).unwrap();
        assert_eq!(color, MeterColor::Muted, "{future}");
    }
    let color: MeterColor = serde_json::from_value(json!({"series": 9})).unwrap();
    assert_eq!(color, MeterColor::Series(9), "The terminal wraps the slot");
    let segment: SidebarContent = serde_json::from_value(json!({
        "kind": "context_breakdown",
        "remainder": "Messages",
        "parts": [{"label": "MCP", "value": 1, "color": "neon", "estimated": true}],
    }))
    .unwrap();
    assert!(
        matches!(&segment, SidebarContent::ContextBreakdown { parts, .. } if parts[0].color == MeterColor::Muted),
        "A breakdown with a future color stays a breakdown: {segment:?}"
    );
}

#[test]
fn unknown_capacity_is_not_a_zero_percent_measurement() {
    assert_eq!(ContextUsage::new(100).estimated().percent(), None);
    assert_eq!(ContextUsage::new(100).with_capacity(0).percent(), None);
    assert_eq!(
        ContextUsage::new(150).with_capacity(100).percent(),
        Some(150.0)
    );
}

#[test]
fn form_submission_round_trip_preserves_values_without_debug_disclosure() {
    let request = request(Command::SubmitForm {
        name: "configure".into(),
        form: "host-revision".into(),
        values: Box::new([("key", "fixture-secret")].into_iter().collect()),
    });
    let encoded = round_trip(&request);
    assert_eq!(encoded["command"]["values"]["key"], "fixture-secret");
    assert!(!format!("{request:?}").contains("fixture-secret"));
    let field = FormField::new("key", "Key", FieldKind::Secret).with_value("fixture-secret");
    assert!(!format!("{field:?}").contains("fixture-secret"));
}

#[test]
fn snapshot_constructor_lists_its_selected_agent() {
    let snapshot = Snapshot::new("room", "Title", Actor::new("agent", "Agent"));
    assert_eq!(snapshot.selected_agent.as_str(), "agent");
    assert!(
        snapshot
            .actors
            .iter()
            .any(|a| a.id == snapshot.selected_agent)
    );
}

/// RMK-1161: a message handed to a running turn, and the run that answers a
/// message, keep their wire shape; an older host's start names no message.
#[test]
fn steering_keeps_its_wire_shape() {
    let steered = Event::MessageSteered {
        conversation: "room".into(),
        message: "m2".into(),
        run: "agent:m1".into(),
    };
    assert_eq!(
        round_trip(&steered),
        json!({"kind": "message_steered", "conversation": "room", "message": "m2", "run": "agent:m1"})
    );
    let answering = Event::RunStarted {
        conversation: "room".into(),
        run: "agent:m2".into(),
        answers: Some("m2".into()),
    };
    assert_eq!(round_trip(&answering)["answers"], json!("m2"));
    let plain = Event::RunStarted {
        conversation: "room".into(),
        run: "r".into(),
        answers: None,
    };
    assert!(
        round_trip(&plain).get("answers").is_none(),
        "An absent message is omitted"
    );
    let older: Event =
        serde_json::from_value(json!({"kind": "run_started", "conversation": "room", "run": "r"}))
            .unwrap();
    assert_eq!(
        older,
        Event::RunStarted {
            conversation: "room".into(),
            run: "r".into(),
            answers: None,
        }
    );
    let mut snapshot =
        serde_json::to_value(Snapshot::new("room", "T", Actor::new("a", "A"))).unwrap();
    snapshot.as_object_mut().unwrap().remove("can_steer");
    let snapshot: Snapshot = serde_json::from_value(snapshot).unwrap();
    assert!(!snapshot.can_steer, "an older host does not steer");
}

/// RMK-1461: an older host states neither cache writes nor a cost, which
/// read as none.
#[test]
fn an_older_host_s_usage_and_snapshot_read_without_cache_writes_or_cost() {
    let usage: TokenUsage = serde_json::from_value(serde_json::json!({
        "input_tokens": 8,
        "output_tokens": 2,
        "cached_input_tokens": 1,
        "complete": true,
    }))
    .unwrap();
    assert_eq!(usage, TokenUsage::complete(8, 2, 1));
    let snapshot = Snapshot::new("room", "Title", Actor::new("agent", "Agent"));
    let mut older = serde_json::to_value(&snapshot).unwrap();
    assert!(older.get("cost").is_none(), "No cost is omitted");
    older.as_object_mut().unwrap().remove("cost");
    let read: Snapshot = serde_json::from_value(older).unwrap();
    assert_eq!(read.cost, None);
    let mut priced = snapshot;
    priced.cost = Some(ConversationCost::new(420_000, "USD", true));
    round_trip(&priced);
}
