//! View state a host publishes without reloading the view: the title and the
//! mode it sends, by a command allowed while busy or by its subscription,
//! survive a later snapshot built from the configuration the host receives.

use super::*;
use roomkit::model::HookResult;
use roomkit_tui_engine::{CommandHandler, CommandResult};
use tokio::sync::watch;

/// Renames on `rename`, even while busy; reloads the view it was given on `reload`.
struct Renamer {
    updates: Option<watch::Receiver<Vec<Event>>>,
}

impl CommandHandler for Renamer {
    fn subscribe(&self, _: SessionConfig) -> Option<watch::Receiver<Vec<Event>>> {
        self.updates.clone()
    }

    fn allow_while_busy(&self, name: &str) -> bool {
        name == "rename"
    }

    async fn handle(
        &self,
        request: Request,
        view: SessionConfig,
    ) -> Result<CommandResult, Rejection> {
        let Command::Invoke { name, arguments } = request.command else {
            return Err("Unsupported".into());
        };
        if name == "reload" {
            return Ok(CommandResult::Switch(Box::new(view)));
        }
        Ok(CommandResult::Events(vec![
            title(&arguments),
            mode("renamed"),
        ]))
    }
}

fn title(title: &str) -> Event {
    Event::TitleChanged {
        conversation: "room".into(),
        title: title.into(),
    }
}

fn mode(selected: &str) -> Event {
    let choices = vec![Choice::new(selected, selected)];
    Event::ModeChanged {
        conversation: "room".into(),
        mode: ModeState::new("mode", selected, choices),
    }
}

fn view() -> SessionConfig {
    let mut view = config();
    view.commands = vec![
        CommandSpec::new("rename", "Rename this conversation").with_arguments("<title>"),
        CommandSpec::new("reload", "Reload this conversation"),
        CommandSpec::new("mode", "Change the mode"),
    ];
    view
}

fn invoke(id: u64, name: &str, arguments: &str) -> Request {
    let command = Command::Invoke {
        name: name.into(),
        arguments: arguments.into(),
    };
    Request::new(RequestId::new(id), format!("{name}-{id}"), "room", command)
}

/// Reload the view through the host and return the snapshot it shows.
async fn reload(client: &Client, id: u64) -> Snapshot {
    client.try_send(invoke(id, "reload", "")).unwrap();
    loop {
        if let Event::Snapshot(snapshot) = next(client).await {
            return snapshot;
        }
    }
}

/// The title and selected mode a snapshot shows.
fn shown(snapshot: &Snapshot) -> (&str, &str) {
    let mode = snapshot
        .mode
        .as_ref()
        .map_or("", |mode| mode.selected.as_str());
    (snapshot.title.as_str(), mode)
}

#[tokio::test]
async fn a_rename_while_busy_is_published_at_once_and_kept_by_a_later_snapshot() {
    let kit = RoomKit::new(Default::default());
    let rounds = vec![ScriptedRound::answering("Done")];
    let channel = prepare(&kit, ScriptedProvider::new(rounds), Arc::default(), false).await;
    let (open, gate) = watch::channel(false);
    kit.before_broadcast("hold", move |_, _| {
        let mut gate = gate.clone();
        async move {
            let _ = gate.wait_for(|open| *open).await;
            Ok(HookResult::allow(Default::default()))
        }
    })
    .unwrap();
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let session = LocalSession::new(kit.clone(), channel, view());
    let task = tokio::spawn(session.run_with_commands(host, Renamer { updates: None }));
    assert!(matches!(next(&client).await, Event::Snapshot(s) if s.title == "room"));
    client.try_send(request("held")).unwrap();
    while !matches!(next(&client).await, Event::OperationStarted { .. }) {}

    client
        .try_send(invoke(2, "rename", "Quarterly review"))
        .unwrap();
    assert!(matches!(
        next(&client).await,
        Event::Reply { error: None, .. }
    ));
    assert_eq!(next(&client).await, title("Quarterly review"));
    assert_eq!(next(&client).await, mode("renamed"));
    // A title the terminal would ignore is not kept for later snapshots.
    let oversized = "x".repeat(257);
    client.try_send(invoke(3, "rename", &oversized)).unwrap();
    while next(&client).await != title(&oversized) {}
    open.send_replace(true);
    collect_operation(&client).await;

    let restored = reload(&client, 4).await;
    assert_eq!(shown(&restored), ("Quarterly review", "renamed"));
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}

#[tokio::test]
async fn a_title_and_mode_published_by_the_host_subscription_are_kept_by_a_later_snapshot() {
    let kit = RoomKit::new(Default::default());
    let channel = prepare(&kit, ScriptedProvider::new([]), Arc::default(), false).await;
    let (publish, updates) = watch::channel(vec![title("Named on attach"), mode("attached")]);
    let (client, host) = connection(NonZeroUsize::new(256).unwrap());
    let session = LocalSession::new(kit.clone(), channel, view());
    let handler = Renamer {
        updates: Some(updates),
    };
    let task = tokio::spawn(session.run_with_commands(host, handler));
    assert!(matches!(next(&client).await, Event::Snapshot(s) if s.title == "room"));
    assert_eq!(next(&client).await, title("Named on attach"));

    let restored = reload(&client, 1).await;
    assert_eq!(shown(&restored), ("Named on attach", "attached"));

    publish.send_replace(vec![title("Named later"), mode("later")]);
    while next(&client).await != title("Named later") {}
    let restored = reload(&client, 2).await;
    assert_eq!(shown(&restored), ("Named later", "later"));
    drop(client);
    task.await.unwrap().unwrap();
    kit.close().await.unwrap();
}
