//! Long-running host commands share the view's execution controls, not its model.
use std::{fmt, future::Future, pin::Pin};

use roomkit_tui_protocol::{ConversationId, Event, Item, OperationId, RunId, RunOutcome};
use tokio::sync::watch;

use crate::activity::Active;
use crate::{ConversationChannel, cancellation};

type Completion = Result<RunOutcome, String>;
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
    /// starts nothing. Give each execution a new `run`: a terminal ignores a
    /// run that repeats the last one it saw.
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

    /// Start in a room this view reserved.
    pub(crate) fn start(
        self,
        channel: ConversationChannel,
        conversation: ConversationId,
        operation: OperationId,
        reserved: Active,
    ) -> Interrupter {
        let operation =
            Operation::start(channel.clone(), conversation.clone(), operation, reserved);
        let (stop, stopped) = watch::channel(false);
        let context = ExecutionContext {
            channel: channel.clone(),
            conversation: conversation.clone(),
            stopped,
        };
        channel.publish(
            conversation.as_str(),
            Event::RunStarted {
                conversation: conversation.clone(),
                run: self.run.clone(),
            },
        );
        let run = self.run.clone();
        tokio::spawn(async move {
            // A host panic must not leave the client permanently busy.
            let result = tokio::spawn(async move { (self.task)(context).await }).await;
            let outcome = match result {
                Ok(Ok(outcome)) => outcome,
                result => {
                    let text = match result {
                        Ok(Err(text)) => text,
                        _ => {
                            "The application command failed unexpectedly; inspect host diagnostics"
                                .into()
                        }
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
            operation.finish(outcome);
        });
        Interrupter::Command {
            run: self.run,
            stop: std::sync::Arc::new(stop),
        }
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
    },
}

impl Interrupter {
    pub async fn interrupt(&self, run: RunId) -> Result<(), &'static str> {
        match self {
            Self::Engine(control) => control.interrupt(run).await,
            Self::Command { run: active, stop } if active == &run => stop
                .send(true)
                .map_err(|_| "This execution has already ended"),
            Self::Command { .. } => Err("This execution is no longer active in this attachment"),
        }
    }
}
