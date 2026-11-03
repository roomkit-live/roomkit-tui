//! An application-owned RoomKit engine, OpenAI-compatible provider and SQLite
//! store, assembled with the generic terminal adapter. `--check` uses a local
//! HTTP fixture; interactive mode reads endpoint credentials from environment.

mod check;
mod mock;

use std::sync::{Arc, atomic::AtomicUsize};
use std::{error::Error, num::NonZeroUsize, path::Path, path::PathBuf, time::Duration};

use roomkit::model::{ChannelId, RoomId};
use roomkit::{CreateRoomOptions, GetRoomOptions, ListBindingsOptions, RoomKit, RoomKitOptions};
use roomkit_ai::{AiChannel, AiChannelOptions, AiTool, ToolEntry, ToolHandler, ToolResult};
use roomkit_openai::{OpenAiAiProvider, OpenAiConfig};
use roomkit_sqlite::{SqliteStore, SqliteStoreOptions};
use roomkit_tui::{connection, render::Renderers};
use roomkit_tui_engine::{ConversationChannel, LocalSession, SessionConfig};
use roomkit_tui_protocol::ModelInfo;
use serde_json::json;

pub use check::check;

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

/// The provider name the engine reports for the OpenAI provider, which a
/// view's model must carry for its measurements: Python's class name.
const PROVIDER: &str = "OpenAIAIProvider";
const SYSTEM_PROMPT: &str =
    "You are a helpful assistant. You can look up fictional records when asked.";
/// How long a detached view waits for the current turn before closing.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(30);

const HELP: &str = "roomkit-chat --database PATH [--room ID] [--buffered] [--allow-insecure-http]
roomkit-chat --check

Set ROOMKIT_BASE_URL and ROOMKIT_MODEL. ROOMKIT_API_KEY is optional; it is
only sent over HTTPS or to this machine unless --allow-insecure-http is given.
The example exposes only a fictional record lookup tool. Ctrl+D detaches;
the application then waits for the current turn and closes its engine.";

/// Command-line choices of the interactive example.
struct Arguments {
    database: PathBuf,
    room: String,
    buffered: bool,
    insecure: bool,
}

/// Run the application example, or its local HTTP integration check.
pub async fn launch() -> Result<()> {
    let (args, options) = crate::appearance::parse(std::env::args().skip(1))?;
    let Some(args) = arguments(args)? else {
        return check().await;
    };
    let base = std::env::var("ROOMKIT_BASE_URL")
        .map_err(|_| "Set ROOMKIT_BASE_URL to an OpenAI-compatible API root")?;
    let model = std::env::var("ROOMKIT_MODEL")
        .map_err(|_| "Set ROOMKIT_MODEL to a model served by the endpoint")?;
    let key = std::env::var("ROOMKIT_API_KEY").unwrap_or_default();
    if !key.is_empty() && !args.insecure && exposes_credentials(&base) {
        return Err("Refusing to send ROOMKIT_API_KEY over plain HTTP to another machine; use HTTPS or pass --allow-insecure-http".into());
    }
    let config = OpenAiConfig {
        base_url: Some(base),
        ..OpenAiConfig::new(key, &model)
    };
    let (kit, channel, mut view) = assemble(
        &args.database,
        &args.room,
        (config, Some(SYSTEM_PROMPT)),
        Arc::default(),
        args.buffered,
    )
    .await?;
    view.model = Some(model_info(&model));
    let (client, host) = connection(NonZeroUsize::new(256).ok_or("Invalid queue capacity")?);
    let adapter = tokio::spawn(LocalSession::new(kit.clone(), channel.clone(), view).run(host));
    let terminal = tokio::task::spawn_blocking(move || {
        roomkit_tui::run_with_options(client, Renderers::default(), options)
    })
    .await;
    let attached = adapter.await;
    // Decisions of a running turn are stored when it settles.
    if tokio::time::timeout(SETTLE_TIMEOUT, channel.idle())
        .await
        .is_err()
    {
        eprintln!("The current turn did not finish in time; closing the engine cancels it.");
    }
    let closed = kit.close().await;
    terminal??;
    attached??;
    closed?;
    Ok(())
}

/// Parse arguments; `None` selects the `--check` scenario.
fn arguments(args: Vec<String>) -> Result<Option<Arguments>> {
    let mut args = args.into_iter();
    let mut database = None;
    let mut parsed = Arguments {
        database: PathBuf::new(),
        room: "conversation".into(),
        buffered: false,
        insecure: false,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => return Ok(None),
            "--database" => database = Some(args.next().ok_or("--database needs a path")?),
            "--room" => parsed.room = args.next().ok_or("--room needs an identity")?,
            "--buffered" => parsed.buffered = true,
            "--allow-insecure-http" => parsed.insecure = true,
            "--help" => {
                println!("{HELP}\n{}", crate::appearance::HELP);
                std::process::exit(0);
            }
            _ => return Err("Unknown argument; use --help".into()),
        }
    }
    parsed.database = database
        .ok_or("Choose --database PATH to store this conversation")?
        .into();
    Ok(Some(parsed))
}

/// Whether credentials sent to `base` would cross the network unencrypted.
/// The URL is read with the parser the HTTP client uses; anything it cannot
/// read is treated as exposed.
fn exposes_credentials(base: &str) -> bool {
    let Ok(url) = url::Url::parse(base.trim()) else {
        return true;
    };
    let loopback = match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    };
    url.scheme() != "https" && !loopback
}

fn model_info(model: &str) -> ModelInfo {
    ModelInfo::new(PROVIDER, model)
}

/// The engine, its store at `path`, the agent from `provider` (an endpoint
/// and an optional system prompt) and this view's channel, attached to `room`.
async fn assemble(
    path: &Path,
    room: &str,
    (provider, system_prompt): (OpenAiConfig, Option<&str>),
    calls: Arc<AtomicUsize>,
    buffered: bool,
) -> Result<(RoomKit<SqliteStore>, ConversationChannel, SessionConfig)> {
    let store = SqliteStore::new(SqliteStoreOptions {
        path: path.to_path_buf(),
    });
    let kit = RoomKit::new(RoomKitOptions::with_store(store));
    let agent = ChannelId::new("assistant")?;
    let schema = json!({"type":"object","properties":{"reference":{"type":"string"}},"required":["reference"],"additionalProperties":false});
    let lookup = AiTool {
        name: "lookup_record".to_owned(),
        description: "Read a fictional record by reference".to_owned(),
        parameters: schema,
    };
    let handler = ToolHandler::new(move |_, arguments| {
        calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        async move {
            Ok(ToolResult::Json(
                json!({"reference":arguments.get("reference"),"status":"ready","fictional":true}),
            ))
        }
    });
    let options = AiChannelOptions {
        system_prompt: system_prompt.map(str::to_owned),
        tools: vec![ToolEntry::Definition(lookup)],
        tool_handler: Some(handler),
        // A message written while the agent works goes to its turn.
        steer_room_turns: true,
        ..Default::default()
    };
    let provider = OpenAiAiProvider::new(provider)?;
    kit.register_channel(AiChannel::new(agent.clone(), provider, options)?)?;
    let id = ChannelId::new("terminal")?;
    let channel = if buffered {
        ConversationChannel::buffered(id)
    } else {
        ConversationChannel::new(id)
    };
    kit.register_channel(channel.clone())?;
    let room = RoomId::new(room)?;
    open_room(&kit, &room, [channel.id(), &agent]).await?;
    Ok((kit, channel, SessionConfig::new(room, agent, "local-user")))
}

/// Create the room if needed and attach any missing channel, so an earlier
/// run interrupted between those steps is repaired rather than unusable.
async fn open_room(
    kit: &RoomKit<SqliteStore>,
    room: &RoomId,
    channels: [&ChannelId; 2],
) -> Result<()> {
    match kit.get_room(room, GetRoomOptions::default()).await {
        Ok(_) => {}
        Err(roomkit::RoomKitError::RoomNotFound(_)) => {
            let options = CreateRoomOptions {
                room_id: Some(room.clone()),
                ..Default::default()
            };
            kit.create_room(options).await?;
        }
        Err(error) => return Err(error.into()),
    }
    let bound = kit
        .list_bindings(room, ListBindingsOptions::default())
        .await?;
    for id in channels {
        if !bound.iter().any(|binding| &binding.channel_id == id) {
            kit.attach_channel(room, id, Default::default()).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::exposes_credentials;

    #[test]
    fn credentials_stay_on_encrypted_or_local_connections() {
        for local in [
            "https://api.example.com/v1",
            "http://localhost:11434/v1",
            "http://127.0.0.1:8080",
            "http://[::1]:8080/v1",
        ] {
            assert!(!exposes_credentials(local), "{local}");
        }
        // Review: scheme case, leading spaces and backslashes used to hide
        // the real host from this check.
        for remote in [
            "http://api.example.com/v1",
            "http://user@10.0.0.5:8080",
            "HTTP://api.example.com/v1",
            " http://api.example.com",
            "http://evil.example\\@localhost/",
            "not a url",
        ] {
            assert!(exposes_credentials(remote), "{remote}");
        }
    }
}
