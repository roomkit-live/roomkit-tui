//! A deterministic host driven by the PTY check, including events in one burst.
#![allow(clippy::unwrap_used, reason = "Invalid fixture state fails the test")]

use roomkit_tui::{Host, TerminalStatus, UiOptions, connection, protocol::*, render::Renderers};
use std::{num::NonZeroUsize, thread, time::Duration};

fn snapshot(room: &str) -> Snapshot {
    {
        let mut snapshot = Snapshot::new(
            room,
            format!("Status fixture {room}"),
            Actor::new("agent", "Agent"),
        );
        snapshot.commands = [
            "stream-failure",
            "admission-failure",
            "switch",
            "disconnect",
            "disconnect-working",
        ]
        .map(|name| CommandSpec::new(name, name))
        .to_vec();
        snapshot.can_interrupt = true;
        snapshot
    }
}

fn publish(host: &Host, events: impl IntoIterator<Item = Event>) {
    for event in events {
        host.try_send(event).unwrap();
    }
}

fn serve(host: Host) {
    let mut room = ConversationId::from("initial");
    let mut active = None;
    publish(&host, [Event::Snapshot(snapshot(room.as_str()))]);
    loop {
        let request = match host.try_recv() {
            Ok(request) => request,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
        };
        assert_eq!(request.conversation, room);
        publish(
            &host,
            [Event::Reply {
                request: request.id,
                error: None,
            }],
        );
        match request.command {
            Command::Invoke { name, .. } if name == "disconnect" => return,
            Command::Invoke { name, .. } if name == "switch" => {
                room = "restored".into();
                publish(&host, [Event::Snapshot(snapshot(room.as_str()))]);
            }
            Command::Invoke { name, .. } => {
                publish(
                    &host,
                    [Event::OperationStarted {
                        conversation: room.clone(),
                        operation: request.operation.clone(),
                    }],
                );
                if name == "disconnect-working" {
                    return;
                }
                if name == "stream-failure" {
                    publish(
                        &host,
                        [
                            Event::RunStarted {
                                conversation: room.clone(),
                                run: "failed-run".into(),
                                answers: None,
                            },
                            Event::RunFinished {
                                conversation: room.clone(),
                                run: "failed-run".into(),
                                outcome: RunOutcome::Failed,
                            },
                        ],
                    );
                }
                publish(
                    &host,
                    [Event::OperationFinished {
                        conversation: room.clone(),
                        operation: request.operation,
                        outcome: if name == "stream-failure" {
                            RunOutcome::Completed
                        } else {
                            RunOutcome::Failed
                        },
                    }],
                );
            }
            Command::Submit { .. } => {
                let run = RunId::from(request.operation.as_str());
                publish(
                    &host,
                    [
                        Event::OperationStarted {
                            conversation: room.clone(),
                            operation: request.operation.clone(),
                        },
                        Event::RunStarted {
                            conversation: room.clone(),
                            run: run.clone(),
                            answers: None,
                        },
                    ],
                );
                for id in ["first", "second"] {
                    publish(
                        &host,
                        [Event::PermissionRequested {
                            conversation: room.clone(),
                            permission: Permission::new(
                                id,
                                "agent",
                                run.clone(),
                                format!("Approve {id} action?"),
                                "Private fixture arguments",
                            ),
                        }],
                    );
                }
                active = Some((request.operation, run));
            }
            Command::Decide { permission, .. } => {
                publish(
                    &host,
                    [Event::PermissionResolved {
                        conversation: room.clone(),
                        permission: permission.clone(),
                    }],
                );
                if permission.as_str() == "second" {
                    let (operation, run) = active.take().unwrap();
                    publish(
                        &host,
                        [
                            Event::RunFinished {
                                conversation: room.clone(),
                                run,
                                outcome: RunOutcome::Completed,
                            },
                            Event::OperationFinished {
                                conversation: room.clone(),
                                operation,
                                outcome: RunOutcome::Completed,
                            },
                        ],
                    );
                }
            }
            _ => unreachable!(),
        }
    }
}

#[test]
#[ignore = "Run by scripts/check-terminal-status.py inside a pseudo-terminal"]
fn status_fixture() {
    let (client, host) = connection(NonZeroUsize::new(64).unwrap());
    let host = thread::spawn(move || serve(host));
    let mut options = UiOptions::default();
    options.terminal_status = Some(TerminalStatus::new("status-fixture").unwrap());
    options.animations = false;
    let result = roomkit_tui::run_with_options(client, Renderers::default(), options);
    host.join().unwrap();
    result.unwrap();
}
