//! A scripted host for `scripts/check-steering.py` (RMK-1303). It refuses the
//! first messages as busy with no run to show, as an engine closing a turn
//! does: the terminal's own loop must send them again, on its own clock.

use std::num::NonZeroUsize;
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::{Duration, Instant};

use roomkit_tui::protocol::{Actor, Command, Event, Rejection, Snapshot};
use roomkit_tui::render::Renderers;
use roomkit_tui::{Host, connection, run};

const REFUSALS: u32 = 2;

fn serve(host: &Host) {
    let mut snapshot = Snapshot::new("room", "Busy host", Actor::new("agent", "Agent"));
    snapshot.can_interrupt = true;
    drop(host.try_send(Event::Snapshot(snapshot)));
    let mut refusals = 0;
    let mut first = None;
    loop {
        let request = match host.recv_timeout(Duration::from_secs(1)) {
            Ok(request) => request,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let message = matches!(request.command, Command::Submit { .. });
        let refused = message && refusals < REFUSALS;
        let error = refused.then(|| Rejection::busy("A request is still executing"));
        if refused {
            refusals += 1;
            first.get_or_insert_with(Instant::now);
        }
        let reply = Event::Reply {
            request: request.id,
            error,
        };
        drop(host.try_send(reply));
        if message && !refused {
            let waited = first.map_or(0, |at: Instant| at.elapsed().as_millis());
            let text = format!("Taken after {refusals} refusals, {waited} ms apart");
            let notice = Event::Notice {
                conversation: "room".into(),
                text,
            };
            drop(host.try_send(notice));
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let capacity = NonZeroUsize::new(64).ok_or("A connection holds events")?;
    let (client, host) = connection(capacity);
    thread::spawn(move || serve(&host));
    run(client, Renderers::default())?;
    Ok(())
}
