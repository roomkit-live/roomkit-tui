//! Replies to the requests this view sent: what an accepted request
//! settles, and what a refused one gives back to the person.

use std::time::Duration;

use roomkit_tui_protocol::{Command, Rejection, RequestId};

use super::Session;
use super::bounds::bounded;

impl Session {
    pub(super) fn on_reply(
        &mut self,
        request: RequestId,
        error: Option<Rejection>,
        at: Option<Duration>,
    ) {
        let Some(command) = self.pending.remove(&request) else {
            return;
        };
        if matches!(command, Command::CompleteInput { .. }) {
            if let Some(error) = error {
                self.refuse_action_choices(request, &error.message);
            }
            return;
        }
        let silent = self.answered_waited(request, error.as_ref(), at);
        let Some(error) = error else {
            self.on_accepted(&command);
            return;
        };
        match command {
            Command::Decide { permission, .. } => {
                self.deciding.remove(&permission);
            }
            // Refused: the card takes answers again.
            Command::AnswerQuestion { question, .. } => {
                if let Some(card) = self
                    .questions
                    .iter_mut()
                    .find(|card| *card.id() == question)
                {
                    card.sent = false;
                }
            }
            // A late refusal for a run already over leaves the interruption
            // of the run under way pending.
            Command::Interrupt { run } if self.interrupting.as_ref() == Some(&run) => {
                self.interrupting = None;
            }
            _ => {}
        }
        if !silent {
            self.notice = bounded(&error.message, self.limits.text_bytes.get());
        }
    }

    fn on_accepted(&mut self, command: &Command) {
        match command {
            Command::SubmitImages { message, .. }
                if self.editor.expanded() == message.text && self.images == message.images =>
            {
                self.editor.submitted();
                self.images.clear();
                self.draft_epoch = self.draft_epoch.wrapping_add(1);
                self.refresh_menu();
            }
            Command::Invoke { name, .. }
                if self.picker.as_ref().is_some_and(|p| p.command == *name) =>
            {
                self.picker = None;
            }
            Command::SubmitForm { form, .. } => {
                if let Some(current) = self
                    .form
                    .as_mut()
                    .filter(|current| current.spec.id == *form)
                {
                    if current.spec.wait_for_completion {
                        current.submitted = true;
                    } else {
                        self.form = None;
                    }
                }
            }
            _ => {}
        }
    }
}
