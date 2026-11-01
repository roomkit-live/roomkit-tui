//! Request routing. Control requests are answered at once; commands and
//! submissions are validated, then started in a room this view reserved.

use roomkit::ConversationStore;
use roomkit::model::EventContent;
use roomkit_tui_protocol::limits::{
    MAX_FORM_FIELDS, MAX_FORM_NAME_BYTES, MAX_FORM_VALUE_BYTES, MAX_LABEL_BYTES,
    MAX_OPERATION_ID_BYTES, MAX_TEXT_BYTES,
};
use roomkit_tui_protocol::{Command, ErrorCode, Event, Rejection, Request, Snapshot};

use super::serve::{CatchUnwind, Pending, View, send};
use super::{AdapterError, CommandHandler, CommandResult, LocalSession, SessionConfig};
use crate::CommandExecution;
use crate::activity::Active;

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
        replacement: Result<(SessionConfig, Snapshot), AdapterError>,
    },
}

/// Requests answered without waiting for application code.
pub(super) fn is_control(request: &Request) -> bool {
    matches!(
        request.command,
        Command::Decide { .. } | Command::Interrupt { .. }
    )
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
            Command::Submit { .. } | Command::SubmitImages { .. } => self.submit(request),
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
        let Some(work) = self.work.clone() else {
            let error = Rejection::unavailable("This attachment has no active execution");
            return self.reply(request, Some(error));
        };
        let (host, id) = (self.host.clone(), request.id);
        tokio::spawn(async move {
            let error = work.interrupt(run).await.err().map(Rejection::unavailable);
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
        let Some(reserved) = self
            .session
            .channel
            .reserve(self.session.config.conversation())
        else {
            let error = Rejection::busy("A request is still executing");
            return self.reply(&request, Some(error)).map(|()| None);
        };
        let message = match &request.command {
            Command::SubmitImages { message, .. } => message.clone(),
            Command::Submit { text, .. } => {
                let content = roomkit::model::TextContent::new(text.clone()).into();
                self.start_turn(&request, content, reserved)?;
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
    pub fn complete(&mut self, step: Step) -> Result<Option<Pending<'h>>, AdapterError> {
        match step {
            Step::Handled { request, result } => match result.unwrap_or_else(|| Err(panicked())) {
                Ok(result) => self.handled(request, result),
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
            } => {
                self.switched(&request, replacement)?;
                Ok(None)
            }
        }
    }

    fn handled(
        &mut self,
        mut request: Request,
        result: CommandResult,
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
            CommandResult::Switch(config) => self.switch(request, *config),
        }
    }

    /// Start host work in a room this view reserves. A run identity longer
    /// than terminals track would leave the view unable to show or stop it.
    fn execute(
        &mut self,
        request: Request,
        execution: CommandExecution,
    ) -> Result<Option<Pending<'h>>, AdapterError> {
        if execution.run().as_str().len() > MAX_LABEL_BYTES {
            let error = Rejection::invalid(format!(
                "The execution's run identity exceeds {MAX_LABEL_BYTES} bytes; nothing was started"
            ));
            return self.reply(&request, Some(error)).map(|()| None);
        }
        let room = self.session.config.conversation();
        let Some(reserved) = self.session.channel.reserve(room) else {
            let error = Rejection::busy("Cannot start another execution while busy");
            return self.reply(&request, Some(error)).map(|()| None);
        };
        self.reply(&request, None)?;
        let (channel, operation) = (self.session.channel.clone(), request.operation);
        let work = execution.start(channel, &self.session.config, operation, reserved);
        self.work = Some(work);
        Ok(None)
    }

    fn switch(
        &mut self,
        request: Request,
        config: SessionConfig,
    ) -> Result<Option<Pending<'h>>, AdapterError> {
        if self
            .session
            .channel
            .busy(self.session.config.conversation())
        {
            let error = Rejection::busy("Cannot switch an executing conversation");
            return self.reply(&request, Some(error)).map(|()| None);
        }
        let (kit, channel) = (self.session.kit.clone(), self.session.channel.clone());
        self.switching = true;
        Ok(Some(Box::pin(async move {
            let replacement = LocalSession::new(kit, channel, config);
            let snapshot = replacement.snapshot().await;
            let replacement = snapshot.map(|snapshot| (replacement.config, snapshot));
            Step::Switched {
                request,
                replacement,
            }
        })))
    }

    fn switched(
        &mut self,
        request: &Request,
        replacement: Result<(SessionConfig, Snapshot), AdapterError>,
    ) -> Result<(), AdapterError> {
        self.switching = false;
        let (config, snapshot) = match replacement {
            Ok(replacement) => replacement,
            Err(error) => return self.reply(request, Some(refused_switch(&error))),
        };
        let approvals = &self.session.channel.approvals;
        self.approvals = approvals.attach(config.conversation(), config.agent.as_str());
        self.session.config = config;
        self.reply(request, None)?;
        self.attached(snapshot)
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
