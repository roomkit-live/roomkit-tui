//! Moving a view to another room, which work may be running in, and back.

use roomkit::ConversationStore;
use roomkit_tui_protocol::{Event, Rejection, Request, Snapshot};
use tokio::sync::broadcast;

use super::requests::Step;
use super::serve::{Parked, Pending, View, decides};
use super::{AdapterError, CommandHandler, LocalSession, SessionConfig};
use crate::cost::Costs;

/// The room events a view reads.
pub(super) type Frames = broadcast::Receiver<(String, Event)>;

/// The frames queued in `frames`, without those of `target`: its snapshot and
/// the runs read with them count those.
fn queued_frames(frames: &mut Frames, target: &str) -> Vec<(String, Event)> {
    let mut kept = Vec::new();
    loop {
        match frames.try_recv() {
            // Decisions are the current view's too, and shown once by id.
            Ok((room, event)) if room == target && !decides(&event) => {}
            Ok(frame) => kept.push(frame),
            Err(broadcast::error::TryRecvError::Lagged(_)) => {}
            Err(_) => return kept,
        }
    }
}

impl<'h, S: ConversationStore, H: CommandHandler> View<'h, S, H> {
    pub(super) fn switch(
        &mut self,
        request: Request,
        config: SessionConfig,
        frames: &mut Frames,
    ) -> Result<Option<Pending<'h>>, AdapterError> {
        let catch_up = self.place_among_frames(config.conversation(), frames)?;
        let (kit, channel) = (self.session.kit.clone(), self.session.channel.clone());
        self.switching = true;
        Ok(Some(Box::pin(async move {
            let replacement = LocalSession::new(kit, channel, config);
            let attachment = replacement.attachment_of_work().await;
            let replacement =
                attachment.map(|(snapshot, costs)| (replacement.config, snapshot, costs));
            Step::Switched {
                request,
                replacement,
                catch_up,
            }
        })))
    }

    /// What `target`'s view must be told of the work under way there, read
    /// at a known place among the events: the frames queued until then are
    /// served to the current view, or dropped for `target`, whose snapshot
    /// and these runs count them; the frames that follow, kept while the
    /// snapshot is read, count from here. Anything but a frame of `target`
    /// is the current view's, which stays on screen until the switch ends.
    fn place_among_frames(
        &mut self,
        target: &str,
        frames: &mut Frames,
    ) -> Result<Vec<Event>, AdapterError> {
        let channel = self.session.channel.clone();
        let runs = channel.live_runs.lock();
        for frame in queued_frames(frames, target) {
            self.forward(Ok(frame))?;
        }
        Ok(runs.catch_up(target))
    }

    pub(super) fn switched(
        &mut self,
        request: &Request,
        replacement: Result<(SessionConfig, Snapshot, Costs), AdapterError>,
        catch_up: Vec<Event>,
    ) -> Result<(), AdapterError> {
        self.switching = false;
        let (config, snapshot, costs) = match replacement {
            Ok(replacement) => replacement,
            Err(error) => return self.reply(request, Some(refused_switch(&error))),
        };
        self.change_rooms(&config);
        self.session.config = config;
        self.reply(request, None)?;
        self.attached(snapshot, costs)?;
        self.caught_up(catch_up)
    }

    /// Sets the view's approvals to `target`, and what stops work to what it
    /// left there. A room left is kept while work runs in it, or while the
    /// view stays in its group: dropping its approvals would refuse what its
    /// agent asks next.
    fn change_rooms(&mut self, target: &SessionConfig) {
        let channel = self.session.channel.clone();
        let approvals = channel.approvals.attach_in(
            target.conversation(),
            target.agent.as_str(),
            target.group_name(),
        );
        let leaving = self.session.config.conversation().to_owned();
        let group = self.session.config.group_name().to_owned();
        let left = std::mem::replace(&mut self.approvals, approvals);
        if leaving != target.conversation() {
            let returned = self.parked.remove(target.conversation());
            let (work, steering) = returned.map_or((None, Vec::new()), |p| (p.work, p.steering));
            let parked = Parked {
                group,
                work: std::mem::replace(&mut self.work, work),
                steering: std::mem::replace(&mut self.steering, steering),
                _approvals: left,
            };
            self.parked.insert(leaving, parked);
        }
        let kept = target.group_name();
        self.parked
            .retain(|room, parked| parked.group == kept || channel.busy(room));
    }
}

/// Why a replacement view was not adopted: invalid when the terminal would
/// refuse its snapshot, unavailable for any other failure.
fn refused_switch(error: &AdapterError) -> Rejection {
    match error {
        AdapterError::InvalidSnapshot(reason) => {
            Rejection::invalid(format!("{reason}; the current session is unchanged"))
        }
        _ => Rejection::unavailable(
            "The requested conversation is unavailable; the current session is unchanged",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(room: &str, text: &str) -> (String, Event) {
        let conversation = room.into();
        let text = text.into();
        (room.to_owned(), Event::Notice { conversation, text })
    }

    /// A frame of the room being reached is counted by what is read there;
    /// the others are the view's still, in their order.
    #[test]
    fn the_frames_of_the_target_are_set_aside_and_the_others_kept_in_order() {
        let (sender, mut frames) = broadcast::channel(16);
        for frame in [
            notice("here", "first"),
            notice("target", "counted"),
            notice("here", "second"),
            notice("elsewhere", "third"),
        ] {
            sender.send(frame).unwrap_or_default();
        }
        let kept: Vec<String> = queued_frames(&mut frames, "target")
            .into_iter()
            .map(|(room, event)| match event {
                Event::Notice { text, .. } => format!("{room}:{text}"),
                _ => String::new(),
            })
            .collect();
        assert_eq!(kept, ["here:first", "here:second", "elsewhere:third"]);
        assert!(matches!(
            frames.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }
}
