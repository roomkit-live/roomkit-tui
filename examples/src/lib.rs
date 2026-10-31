//! Scripted application hosts exercising the terminal without a model or network.
//!
//! These fixtures own their actors, model catalogue, commands and domain tools.
//! They do not execute real tools or send external messages.

mod appearance;
mod check;
mod demo;
pub mod engine;

use std::collections::VecDeque;
use std::error::Error;
use std::num::NonZeroUsize;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{TryRecvError, TrySendError},
};
use std::thread;
use std::time::{Duration, Instant};

use roomkit_tui::{
    Host,
    render::{Renderers, ToolRenderer},
};
use roomkit_tui_protocol::*;

pub use check::check;
pub use demo::Demo;

/// Example application, with its own tool and command registrations.
#[derive(Clone, Copy, Debug)]
pub enum Profile {
    /// A generic conversation fixture.
    Assistant,
    /// A fictional order-support application.
    Orders,
}

#[derive(Debug)]
struct OrderRenderer;

impl ToolRenderer for OrderRenderer {
    fn render_expanded(&self, call: &ToolCall) -> Vec<String> {
        let mut details = vec![format!("args: {}", call.arguments)];
        details.extend(self.render(call));
        if let Some(data) = &call.data {
            details.push(serde_json::to_string_pretty(data).unwrap_or_default());
        }
        details
    }

    fn render(&self, call: &ToolCall) -> Vec<String> {
        let Some(data) = &call.data else {
            return vec!["Waiting for the fictional order lookup".into()];
        };
        vec![format!(
            "Order {} · {} · {} items",
            data["reference"].as_str().unwrap_or("?"),
            data["status"].as_str().unwrap_or("?"),
            data["items"]
        )]
    }
}

/// Run either the interactive example or its offline checks.
pub fn launch(profile: Profile) -> Result<(), Box<dyn Error>> {
    let (arguments, options) = appearance::parse(std::env::args().skip(1))?;
    if !arguments.is_empty() {
        match arguments.as_slice() {
            [argument] if argument == "--check" => {
                let events = check(profile)?;
                println!(
                    "Verified {} ordered events: permission, structured tool result, streaming, model context, context breakdown and completion.",
                    events.len()
                );
            }
            [argument] if argument == "--json" => {
                for event in check(profile)? {
                    println!("{}", serde_json::to_string(&event)?);
                }
            }
            [argument] if argument == "--help" => println!(
                "Run interactively, or use --check for an offline scenario and --json for its NDJSON events.\n{}",
                appearance::HELP
            ),
            _ => return Err("Unknown option; use --help".into()),
        }
        return Ok(());
    }
    let capacity = NonZeroUsize::new(64).ok_or("Invalid capacity")?;
    let (client, host) = roomkit_tui::connection(capacity);
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let demo = Demo::new(profile).with_command_prefix(options.command_prefix);
    let worker = thread::spawn(move || host_loop(host, demo, &worker_stop));
    let mut renderers = Renderers::default();
    if matches!(profile, Profile::Orders) {
        renderers.register("lookup_order", OrderRenderer);
    }
    let result = roomkit_tui::run_with_options(client, renderers, options);
    stop.store(true, Ordering::Release);
    worker.join().map_err(|_| "Example host thread failed")?;
    result?;
    Ok(())
}

fn host_loop(host: Host, mut demo: Demo, stop: &AtomicBool) {
    let mut outgoing = VecDeque::from([demo.initial(), demo.sidebar()]);
    let mut next_step = Instant::now();
    while !stop.load(Ordering::Acquire) {
        if let Some(event) = outgoing.pop_front() {
            match host.try_send(event) {
                Ok(()) => {}
                Err(TrySendError::Full(event)) => outgoing.push_front(*event),
                Err(TrySendError::Disconnected(_)) => break,
            }
        }
        if outgoing.len() < 32 {
            match host.try_recv() {
                Ok(request) => outgoing.extend(demo.handle(request)),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => break,
            }
            if Instant::now() >= next_step {
                outgoing.extend(demo.advance());
                next_step = Instant::now() + Duration::from_millis(55);
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
}
