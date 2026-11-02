//! The `--check` scenario: the real engine against a local HTTP fixture,
//! then a restart on the same SQLite file. No external service is used.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::{num::NonZeroUsize, path::PathBuf, time::Duration};

use roomkit_openai::OpenAiConfig;
use roomkit_tui::{Session, connection};
use roomkit_tui_engine::LocalSession;
use roomkit_tui_protocol::{Command, Event, Item, Request, RequestId, RunOutcome, ToolState};

use super::{Result, assemble, mock, model_info};

/// Verify the full OpenAI HTTP → RoomKit → adapter → UI projection path and a
/// restart of the engine on the same SQLite file. No external service is used.
pub async fn check() -> Result<()> {
    let mock = mock::Mock::start().await?;
    let database = TemporaryDatabase(std::env::temp_dir().join(format!(
        "roomkit-tui-example-{}.sqlite3",
        uuid::Uuid::new_v4()
    )));
    let provider = || {
        let config = OpenAiConfig {
            base_url: Some(mock.base_url.clone()),
            ..OpenAiConfig::new("", "fixture-model")
        };
        (config, None)
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let (kit, channel, mut view) =
        assemble(&database.0, "example", provider(), calls.clone(), false).await?;
    view.model = Some(model_info("fixture-model"));
    let (client, host) = connection(NonZeroUsize::new(256).ok_or("Invalid capacity")?);
    let task = tokio::spawn(LocalSession::new(kit.clone(), channel.clone(), view).run(host));
    let mut ui = Session::default();
    let events =
        tokio::time::timeout(Duration::from_secs(10), converse(&client, &mut ui)).await??;
    drop(client);
    task.await??;
    channel.idle().await;
    kit.close().await?;
    let answer = verify_view(&ui, calls.load(Ordering::SeqCst))?;
    let (kit, channel, view) = assemble(&database.0, "example", provider(), calls, false).await?;
    let restored = LocalSession::new(kit.clone(), channel, view)
        .snapshot()
        .await?;
    let survived = restored
        .items
        .iter()
        .any(|i| matches!(i, Item::Message(m) if m.id == answer));
    kit.close().await?;
    if !survived {
        return Err("Stored message identity did not survive restart".into());
    }
    verify_requests(&mock)?;
    println!(
        "Verified {events} adapter events, OpenAI-compatible streaming, real tool execution and SQLite restart."
    );
    Ok(())
}

/// Submit one message once the view is attached; return the events seen
/// until its operation completed.
async fn converse(client: &roomkit_tui::Client, ui: &mut Session) -> Result<usize> {
    let mut submitted = false;
    let mut count = 0;
    loop {
        let event = match client.try_recv() {
            Ok(event) => event,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                tokio::time::sleep(Duration::from_millis(1)).await;
                continue;
            }
            Err(_) => return Err("Adapter disconnected before completion".into()),
        };
        count += 1;
        let finished = matches!(
            event,
            Event::OperationFinished {
                outcome: RunOutcome::Completed,
                ..
            }
        );
        ui.apply(event);
        if !submitted && ui.snapshot().is_some() {
            let command = Command::Submit {
                agent: "assistant".into(),
                text: "Look up record 42".into(),
            };
            let request = Request::new(RequestId::new(1), "fixture-lookup", "example", command);
            client
                .try_send(request)
                .map_err(|_| "Request queue unavailable")?;
            submitted = true;
        }
        if finished {
            return Ok(count);
        }
    }
}

/// The view ran the tool once, shows one final answer and the engine's
/// measurements. Returns the answer's stored identity.
fn verify_view(ui: &Session, calls: usize) -> Result<roomkit_tui_protocol::MessageId> {
    // The engine's measurements reach the view only for the model it advertises.
    if ui.usage().is_none() {
        return Err("Token usage from the engine did not reach the view".into());
    }
    let state = ui.snapshot().ok_or("No conversation snapshot")?;
    let succeeded = state
        .items
        .iter()
        .any(|i| matches!(i, Item::Tool { call, .. } if call.state == ToolState::Succeeded));
    let answers: Vec<_> = state
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Message(m) if m.text == "Record 42 is ready." => Some(m.id.clone()),
            _ => None,
        })
        .collect();
    match answers.as_slice() {
        [answer] if calls == 1 && succeeded => Ok(answer.clone()),
        _ => Err("Tool execution or streamed message reconciliation failed".into()),
    }
}

/// The model received the tool's result in its second request.
fn verify_requests(mock: &mock::Mock) -> Result<()> {
    let requests = mock
        .requests
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let tool_result = requests
        .get(1)
        .and_then(|r| r["messages"].as_array())
        .is_some_and(|messages| {
            messages.iter().any(|m| {
                m["role"] == "tool" && m["content"].as_str().is_some_and(|s| s.contains("ready"))
            })
        });
    if requests.len() != 2 || !tool_result {
        return Err("The OpenAI-compatible model did not receive the tool result".into());
    }
    Ok(())
}

struct TemporaryDatabase(PathBuf);
impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
