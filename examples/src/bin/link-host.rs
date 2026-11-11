//! Web links in conversation and form instructions, without a browser or network.

use std::num::NonZeroUsize;
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::Duration;

use roomkit_tui::protocol::{
    Actor, Command, CommandSpec, Event, FieldKind, FormField, FormSpec, Item, Message, Snapshot,
};
use roomkit_tui::render::Renderers;
use roomkit_tui::{ColorMode, Host, UiOptions, connection, run_with_options};

fn serve(host: &Host) {
    let mut snapshot = Snapshot::new("room", "Links", Actor::new("agent", "Agent"));
    snapshot
        .commands
        .push(CommandSpec::new("connect", "Show a link and return field"));
    snapshot.items.push(Item::Message(Message::new(
        "message", "agent",
        "[Open guide](https://example.com/guide?view=full&lang=en)\n\nBare https://example.com/~person/guide_(terminal).\n\nUse /connect to open the form.",
    )));
    drop(host.try_send(Event::Snapshot(snapshot)));
    loop {
        let request = match host.recv_timeout(Duration::from_secs(1)) {
            Ok(request) => request,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        drop(host.try_send(Event::Reply {
            request: request.id,
            error: None,
        }));
        if matches!(request.command, Command::Invoke { ref name, .. } if name == "connect") {
            let url = format!(
                "https://example.com/authorize?state={}&scope=read",
                "x".repeat(240)
            );
            let form = FormSpec::new(
                "login",
                "connect",
                "Browser sign-in",
                vec![FormField::new("callback", "Return URL", FieldKind::Secret)],
            )
            .with_description(format!(
                "Cmd-click the link:\n\n{url}\n\nPaste the return URL below."
            ));
            drop(host.try_send(Event::Form {
                conversation: "room".into(),
                form,
            }));
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let capacity = NonZeroUsize::new(8).ok_or("A connection holds events")?;
    let (client, host) = connection(capacity);
    thread::spawn(move || serve(&host));
    let mut options = UiOptions::default();
    options.color = ColorMode::Never;
    options.animations = false;
    run_with_options(client, Renderers::default(), options)?;
    Ok(())
}
