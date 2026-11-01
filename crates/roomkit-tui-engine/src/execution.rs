//! Long-running host commands share the view's execution controls, not its model.
use std::{fmt, future::Future, pin::Pin};

use roomkit_tui_protocol::{ConversationId, Decision, Event, Item, OperationId, RunId, RunOutcome};
use tokio::sync::watch;

use crate::activity::Active;
use crate::approvals::{Approvals, Running};
use crate::{ConversationChannel, PermissionRequest, SessionConfig, cancellation};

type Completion = Result<RunOutcome, String>;
/// Stores the decisions taken during a run; `false` when that failed.
pub(crate) type Persist = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = bool> + Send>> + Send>;
type Task =
    Box<dyn FnOnce(ExecutionContext) -> Pin<Box<dyn Future<Output = Completion> + Send>> + Send>;

/// Work owned by the application, started only after the adapter accepts it.
/// The application supplies persistence, authorization and joined cancellation.
pub struct CommandExecution {
    run: RunId,
    task: Task,
}

impl fmt::Debug for CommandExecution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandExecution")
            .field("run", &self.run)
            .finish_non_exhaustive()
    }
}

impl CommandExecution {
    /// Prepare work without starting it. Return an outcome only after cleanup.
    /// Errors must be safe to display, without credentials or raw service bodies.
    /// `run` is at most [`MAX_LABEL_BYTES`](roomkit_tui_protocol::limits::MAX_LABEL_BYTES):
    /// terminals ignore a longer run, so the adapter rejects the command and
    /// starts nothing. A `run` that repeats the last run of the room is
    /// rejected too, as a terminal ignores it: give each execution a new one.
    pub fn new<F, Fut>(run: RunId, task: F) -> Self
    where
        F: FnOnce(ExecutionContext) -> Fut + Send + 'static,
        Fut: Future<Output = Completion> + Send + 'static,
    {
        Self {
            run,
            task: Box::new(move |context| Box::pin(task(context))),
        }
    }

    /// The identity announced by `RunStarted` and accepted by `Interrupt`.
    pub(crate) fn run(&self) -> &RunId {
        &self.run
    }

    /// Start in a room this view reserved. The run is announced, then
    /// registered with the approval broker, so the work can ask the person
    /// who invoked the command, as a model turn would.
    pub(crate) fn start(
        self,
        channel: ConversationChannel,
        config: &SessionConfig,
        operation: OperationId,
        reserved: Active,
        persist: Persist,
    ) -> Interrupter {
        let conversation: ConversationId = config.conversation().into();
        let operation =
            Operation::start(channel.clone(), conversation.clone(), operation, reserved);
        let (stop, stopped) = watch::channel(false);
        channel.publish(
            conversation.as_str(),
            Event::RunStarted {
                conversation: conversation.clone(),
                run: self.run.clone(),
            },
        );
        let approvals = channel.approvals.start(
            conversation.as_str(),
            config.agent.as_str(),
            self.run.clone(),
            Some(config.sender.clone()),
        );
        let interrupter = Interrupter::Command {
            run: self.run.clone(),
            stop: std::sync::Arc::new(stop),
            approvals: (
                channel.approvals.clone(),
                (conversation.to_string(), config.agent.to_string()),
            ),
        };
        let context = ExecutionContext {
            channel,
            conversation,
            agent: config.agent.to_string(),
            run: self.run.clone(),
            stopped,
        };
        tokio::spawn(self.run_to_end(context, approvals, operation, persist));
        interrupter
    }

    /// Run the work, withdraw its unanswered approvals, report how it ended,
    /// then store its decisions before the operation settles, as a turn does.
    /// A host panic must not leave the client permanently busy.
    async fn run_to_end(
        self,
        context: ExecutionContext,
        approvals: Running,
        operation: Operation,
        persist: Persist,
    ) {
        let (channel, conversation) = (context.channel.clone(), context.conversation.clone());
        let Self { run, task } = self;
        let result = tokio::spawn(async move { task(context).await }).await;
        drop(approvals);
        let outcome = match result {
            Ok(Ok(outcome)) => outcome,
            result => {
                let text = match result {
                    Ok(Err(text)) => text,
                    _ => "The application command failed unexpectedly; inspect host diagnostics"
                        .into(),
                };
                let notice = Event::Notice {
                    conversation: conversation.clone(),
                    text,
                };
                channel.publish(conversation.as_str(), notice);
                RunOutcome::Failed
            }
        };
        channel.publish(
            conversation.as_str(),
            Event::RunFinished {
                conversation: conversation.clone(),
                run,
                outcome,
            },
        );
        if !persist().await {
            let text = "Decision accepted, but its history could not be saved. This does not change the work.";
            let notice = Event::Notice {
                conversation: conversation.clone(),
                text: text.into(),
            };
            channel.publish(conversation.as_str(), notice);
        }
        operation.finish(outcome);
    }
}

/// Admission and delivery remain busy even before or after a streamed run.
/// Dropping an unsettled task reports failure without treating it as success.
pub(crate) struct Operation {
    channel: ConversationChannel,
    conversation: ConversationId,
    id: OperationId,
    outcome: RunOutcome,
    active: Option<Active>,
}

impl Operation {
    /// Announce accepted work in a room this view reserved.
    pub fn start(
        channel: ConversationChannel,
        conversation: ConversationId,
        id: OperationId,
        reserved: Active,
    ) -> Self {
        let active = Some(reserved);
        channel.publish(
            conversation.as_str(),
            Event::OperationStarted {
                conversation: conversation.clone(),
                operation: id.clone(),
            },
        );
        Self {
            channel,
            conversation,
            id,
            outcome: RunOutcome::Failed,
            active,
        }
    }

    pub fn finish(mut self, outcome: RunOutcome) {
        self.outcome = outcome;
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        drop(self.active.take());
        self.channel.publish(
            self.conversation.as_str(),
            Event::OperationFinished {
                conversation: self.conversation.clone(),
                operation: self.id.clone(),
                outcome: self.outcome,
            },
        );
    }
}

/// Scoped presentation and cooperative interruption for a host command.
/// The host must retain any resources needed to stop and join its execution.
#[derive(Debug)]
pub struct ExecutionContext {
    channel: ConversationChannel,
    conversation: ConversationId,
    agent: String,
    run: RunId,
    stopped: watch::Receiver<bool>,
}

impl ExecutionContext {
    /// Publish an authoritative item in this command's conversation.
    /// Publish persisted identities when the item must survive resume.
    pub fn publish(&self, item: Item) {
        self.channel.publish(
            self.conversation.as_str(),
            Event::Item {
                conversation: self.conversation.clone(),
                item,
            },
        );
    }

    /// Ask the person who invoked this command for a one-time decision before
    /// a concrete action. The request belongs to this run whatever else runs
    /// in the room meanwhile, and only that person may answer. It is refused
    /// without an attached view, once the work is interrupted, and at its
    /// timeout.
    pub async fn ask_permission(&self, request: PermissionRequest) -> Decision {
        let (room, run) = (self.conversation.as_str(), Some(self.run.clone()));
        self.channel
            .ask(room, &self.agent, request, None, run)
            .await
    }

    /// Wait for explicit interruption. Detaching a view does not cancel work.
    pub async fn cancelled(&mut self) {
        if self.stopped.wait_for(|stop| *stop).await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// Interrupts the work a view started. Clones target the same work.
#[derive(Clone)]
pub(crate) enum Interrupter {
    Engine(cancellation::Control),
    Command {
        run: RunId,
        stop: std::sync::Arc<watch::Sender<bool>>,
        /// The broker and `(room, agent)` whose pending requests of this run
        /// an interruption withdraws.
        approvals: (Approvals, (String, String)),
    },
}

impl Interrupter {
    pub async fn interrupt(&self, run: RunId) -> Result<(), &'static str> {
        match self {
            Self::Engine(control) => control.interrupt(run).await,
            Self::Command {
                run: active,
                stop,
                approvals: (broker, scope),
            } if active == &run => {
                stop.send(true)
                    .map_err(|_| "This execution has already ended")?;
                // As for a turn: interrupted work can no longer be approved.
                broker.stop_run(scope, &run);
                Ok(())
            }
            Self::Command { .. } => Err("This execution is no longer active in this attachment"),
        }
    }
}
