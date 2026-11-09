//! Request routing. Control requests are answered at once; commands and
//! submissions are validated, then started in a room this view reserved.

use roomkit::ConversationStore;
use roomkit::model::EventContent;
use roomkit_tui_protocol::limits::{
    MAX_FORM_FIELDS, MAX_FORM_NAME_BYTES, MAX_FORM_VALUE_BYTES, MAX_LABEL_BYTES,
    MAX_OPERATION_ID_BYTES, MAX_TEXT_BYTES,
};
use roomkit_tui_protocol::{Command, ErrorCode, Event, Rejection, Request, RunId, Snapshot};
use tokio::sync::broadcast;

use super::serve::{CatchUnwind, Parked, Pending, View, send};
use super::{AdapterError, CommandHandler, CommandResult, LocalSession, SessionConfig, StopRun};
use crate::CommandExecution;
use crate::activity::{Active, Work};
use crate::cost::Costs;
use crate::execution::Interrupter;

/// What application code returned for a request.
#[allow(
    clippy::large_enum_variant,
    reason = "One value per request, moved once out of its future"
)]
pub(crate) enum Step {
    Handled {
        request: Request,
        result: Option<Result<CommandResult, Rejection>>,
    },
    Images {
        request: Request,
        reserved: Active,
        content: Option<Result<EventContent, Rejection>>,
    },
    Switched {
        request: Request,
        replacement: Result<(SessionConfig, Snapshot, Costs), AdapterError>,
        /// What its snapshot will not hold of the work under way there.
        catch_up: Vec<Event>,
    },
}

/// The room events a view reads.
pub(super) type Frames = broadcast::Receiver<(String, Event)>;

/// Requests answered without waiting for application code.
pub(super) fn is_control(request: &Request) -> bool {
    matches!(
        request.command,
        Command::Decide { .. } | Command::Interrupt { .. }
    )
}

/// Interrupt `run` through whichever of `works` runs it: each accepts only
/// its own run. Several are asked at once, so one busy elsewhere, as in its
/// host's admission, does not delay the others' answer; a single one is
/// asked directly, its answer reaching the view as before.
async fn first_to_stop(works: Vec<Interrupter>, run: RunId) -> Result<(), &'static str> {
    if let [work] = works.as_slice() {
        return work.interrupt(run).await;
    }
    let mut asked = tokio::task::JoinSet::new();
    for work in works {
        let run = run.clone();
        asked.spawn(async move { work.interrupt(run).await });
    }
    let mut refusal = "This execution has already ended";
    while let Some(answer) = asked.join_next().await {
        match answer {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(error)) => refusal = error,
            Err(_) => {}
        }
    }
    Err(refusal)
}

/// Stop `run` through `works`, else through the host.
async fn stop(
    works: Vec<Interrupter>,
    run: RunId,
    host_stop: Option<StopRun>,
) -> Result<(), String> {
    let refused = if works.is_empty() {
        "This execution has already ended".to_owned()
    } else {
        match first_to_stop(works, run).await {
            Ok(()) => return Ok(()),
            Err(error) => error.to_owned(),
        }
    };
    match host_stop {
        Some(stop) => stop.await,
        None => Err(refused),
    }
}

fn panicked() -> Rejection {
    Rejection::new(
        ErrorCode::Failed,
        "The application command failed unexpectedly; inspect host diagnostics",
    )
}

impl<'h, S: ConversationStore, H: CommandHandler> View<'h, S, H> {
    /// Route a request; returns application code to await, if any.
    pub fn receive(&mut self, request: Request) -> Result<Option<Pending<'h>>, AdapterError> {
        let config = &self.session.config;
        let operation = request.operation.as_str();
        if request.conversation.as_str() != config.conversation()
            || operation.is_empty()
            || operation.len() > MAX_OPERATION_ID_BYTES
        {
            let error = Rejection::invalid("Unknown conversation or invalid operation identity");
            return self.reply(&request, Some(error)).map(|()| None);
        }
        match &request.command {
            Command::Decide {
                permission,
                decision,
            } => {
                let scope = (config.conversation(), config.agent.as_str());
                let answer = self.session.channel.approvals.decide(
                    scope,
                    permission,
                    *decision,
                    &config.sender,
                );
                self.reply(&request, answer.err())?;
                Ok(None)
            }
            Command::Interrupt { run } => {
                self.interrupt(&request, run.clone())?;
                Ok(None)
            }
            Command::Invoke { .. } | Command::SubmitForm { .. } => self.invoke(request),
            Command::Submit { .. } | Command::SubmitImages { .. } | Command::Answer { .. } => {
                self.submit(request)
            }
            _ => {
                let error = Rejection::new(
                    ErrorCode::Unsupported,
                    "This host does not support this request",
                );
                self.reply(&request, Some(error)).map(|()| None)
            }
        }
    }

    /// Answer from a separate task: a stopping engine can take a moment.
    fn interrupt(
        &self,
        request: &Request,
        run: roomkit_tui_protocol::RunId,
    ) -> Result<(), AdapterError> {
        // The view's own work first, then the turns of its steered messages,
        // then the host, for a run this view did not start.
        let works: Vec<_> = self
            .work
            .iter()
            .chain(self.steering.iter().filter(|work| work.is_live()))
            .cloned()
            .collect();
        let host_stop = self.handler.stop_run(&self.session.config, &run);
        if works.is_empty() && host_stop.is_none() {
            let error = Rejection::unavailable("This attachment has no active execution");
            return self.reply(request, Some(error));
        }
        let (host, id) = (self.host.clone(), request.id);
        tokio::spawn(async move {
            let error = stop(works, run, host_stop)
                .await
                .err()
                .map(Rejection::unavailable);
            let _ = send(&host, Event::Reply { request: id, error });
        });
        Ok(())
    }

    fn invoke(&mut self, request: Request) -> Result<Option<Pending<'h>>, AdapterError> {
        if let Err(error) = invocation(&request, &self.session.config) {
            return self.reply(&request, Some(error)).map(|()| None);
        }
        let busy = self
            .session
            .channel
            .busy(self.session.config.conversation());
        let allowed = matches!(&request.command, Command::Invoke { name, .. } if self.handler.allow_while_busy(name));
        if busy && !allowed {
            let error = Rejection::busy("A request is still executing");
            return self.reply(&request, Some(error)).map(|()| None);
        }
        let (handler, config) = (self.handler, self.session.config.clone());
        Ok(Some(Box::pin(async move {
            let call = CatchUnwind(Box::pin(handler.handle(request.clone(), config)));
            let result = call.await;
            Step::Handled { request, result }
        })))
    }

    fn submit(&mut self, request: Request) -> Result<Option<Pending<'h>>, AdapterError> {
        if let Some(reason) = &self.session.config.submission_error {
            let error = Rejection::new(ErrorCode::Unavailable, reason.clone());
            return self.reply(&request, Some(error)).map(|()| None);
        }
        if let Err(error) = super::turn::validate(&request, &self.session.config) {
            return self.reply(&request, Some(error)).map(|()| None);
        }
        let reserved = match self
            .session
            .channel
            .reserve(self.session.config.conversation(), Work::Turn)
        {
            Ok(reserved) => reserved,
            Err(held) => {
                // A turn holds the room: the message may go to it.
                let target = (!held.hosting)
                    .then(|| self.steering_target(&request))
                    .flatten();
                if let Some(run) = target {
                    return self.steer(&request, run).map(|()| None);
                }
                let error = Rejection::busy("A request is still executing");
                return self.reply(&request, Some(error)).map(|()| None);
            }
        };
        let message = match &request.command {
            Command::SubmitImages { message, .. } => message.clone(),
            Command::Submit { text, .. } => {
                let content = roomkit::model::TextContent::new(text.clone()).into();
                self.start_turn(&request, content, reserved)?;
                return Ok(None);
            }
            Command::Answer { .. } => {
                self.start_answer(&request, reserved)?;
                return Ok(None);
            }
            _ => return Ok(None),
        };
        let (handler, config) = (self.handler, self.session.config.clone());
        Ok(Some(Box::pin(async move {
            let prepare = handler.prepare_images(&message.text, &message.images, &config);
            let content = CatchUnwind(Box::pin(prepare)).await;
            Step::Images {
                request,
                reserved,
                content,
            }
        })))
    }

    /// Act on what application code returned.
    pub fn complete(
        &mut self,
        step: Step,
        frames: &mut Frames,
    ) -> Result<Option<Pending<'h>>, AdapterError> {
        match step {
            Step::Handled { request, result } => match result.unwrap_or_else(|| Err(panicked())) {
                Ok(result) => self.handled(request, result, frames),
                Err(error) => self.reply(&request, Some(error)).map(|()| None),
            },
            Step::Images {
                request,
                reserved,
                content,
            } => {
                match content.unwrap_or_else(|| Err(panicked())) {
                    Ok(content) => self.start_turn(&request, content, reserved)?,
                    Err(error) => self.reply(&request, Some(error))?,
                }
                Ok(None)
            }
            Step::Switched {
                request,
                replacement,
                catch_up,
            } => {
                self.switched(&request, replacement, catch_up)?;
                Ok(None)
            }
        }
    }

    fn handled(
        &mut self,
        mut request: Request,
        result: CommandResult,
        frames: &mut Frames,
    ) -> Result<Option<Pending<'h>>, AdapterError> {
        match result {
            CommandResult::Events(events) => {
                self.reply(&request, None)?;
                self.publish(events)?;
                Ok(None)
            }
            CommandResult::Submit(text) => {
                let agent = self.session.config.agent.as_str().into();
                request.command = Command::Submit { agent, text };
                self.submit(request)
            }
            CommandResult::Execute(execution) => self.execute(request, execution),
            CommandResult::Switch(config) => self.switch(request, *config, frames),
        }
    }

    /// Start host work in a room this view reserves. A run identity longer
    /// than terminals track would leave the view unable to show or stop it.
    fn execute(
        &mut self,
        request: Request,
        execution: CommandExecution,
    ) -> Result<Option<Pending<'h>>, AdapterError> {
        let room = self.session.config.conversation();
        let last = self.session.channel.last_run(room);
        if let Some(error) = execution.refusal(last.as_ref()) {
            return self.reply(&request, Some(error)).map(|()| None);
        }
        let Ok(reserved) = self.session.channel.reserve(room, Work::Host) else {
            let error = Rejection::busy("Cannot start another execution while busy");
            return self.reply(&request, Some(error)).map(|()| None);
        };
        self.reply(&request, None)?;
        let (channel, operation) = (self.session.channel.clone(), request.operation);
        let persist = self.session.history_writer();
        let work = execution.start(channel, &self.session.config, operation, reserved, persist);
        self.work = Some(work);
        Ok(None)
    }

    fn switch(
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
        loop {
            match frames.try_recv() {
                Ok((room, _)) if room == target => {}
                Ok(frame) => {
                    self.forward(Ok(frame))?;
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
        Ok(runs.catch_up(target))
    }

    fn switched(
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
    /// left there. A room left while work runs is kept until the view returns
    /// or the work ends: dropping its approvals would withdraw its decisions.
    fn change_rooms(&mut self, target: &SessionConfig) {
        let channel = self.session.channel.clone();
        let approvals = channel
            .approvals
            .attach(target.conversation(), target.agent.as_str());
        let leaving = self.session.config.conversation().to_owned();
        let left = std::mem::replace(&mut self.approvals, approvals);
        if leaving != target.conversation() {
            let returned = self.parked.remove(target.conversation());
            let (work, steering) = returned.map_or((None, Vec::new()), |p| (p.work, p.steering));
            let parked = Parked {
                work: std::mem::replace(&mut self.work, work),
                steering: std::mem::replace(&mut self.steering, steering),
                _approvals: left,
            };
            if channel.busy(&leaving) {
                self.parked.insert(leaving, parked);
            }
        }
        self.parked.retain(|room, _| channel.busy(room));
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

/// Bounds and advertisement of an application command.
fn invocation(request: &Request, config: &SessionConfig) -> Result<(), Rejection> {
    match &request.command {
        Command::Invoke { name, arguments } if config.advertises(name) => {
            if arguments.len() > MAX_TEXT_BYTES {
                return Err(Rejection::invalid("Command arguments exceed 64 KiB"));
            }
            Ok(())
        }
        Command::SubmitForm { name, form, values } if config.advertises(name) => {
            let oversized = form.is_empty()
                || form.len() > MAX_LABEL_BYTES
                || values.len() > MAX_FORM_FIELDS
                || values.iter().any(|(key, value)| {
                    key.len() > MAX_FORM_NAME_BYTES || value.len() > MAX_FORM_VALUE_BYTES
                });
            if oversized {
                return Err(Rejection::invalid("Invalid or oversized form submission"));
            }
            Ok(())
        }
        _ => Err(Rejection::invalid(
            "Unknown conversation or application command",
        )),
    }
}
