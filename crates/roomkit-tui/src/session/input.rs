//! Composer input and correlated requests. Nothing here executes work: a
//! request is a proposal the host accepts or refuses.

use roomkit_tui_protocol::limits::{MAX_IMAGE_BASE64_BYTES, MAX_IMAGES};
use roomkit_tui_protocol::{
    Command, ImageMessage, ImageUpload, InputAction, OperationId, Request, RequestId,
};

use super::Session;

const MAX_PENDING_REQUESTS: usize = 64;

/// Why the view refused to build or send a request. Display text is meant
/// for the person at the terminal.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SessionError {
    /// No snapshot has arrived yet.
    #[error("Waiting for the host")]
    WaitingForHost,
    /// Too many requests await a reply.
    #[error("Waiting for pending host responses")]
    TooManyPendingRequests,
    /// Request identifiers are exhausted for this view.
    #[error("Request identifiers exhausted")]
    RequestIdsExhausted,
    /// The composer is empty.
    #[error("Enter a message")]
    EmptyMessage,
    /// An action prefix was typed without text after it.
    #[error("Enter text after {0}")]
    EmptyActionText(char),
    /// The command is not advertised by the host.
    #[error("Unknown command {prefix}{name}; press Tab to see host commands")]
    UnknownCommand {
        /// Prefix used to invoke commands.
        prefix: char,
        /// The command as typed.
        name: String,
    },
    /// Another execution is active or a message is awaiting its reply.
    #[error("An execution is active; interrupt it before submitting another message")]
    Busy,
    /// Images are waiting for host validation.
    #[error("Waiting for image submission")]
    ImagesPending,
    /// Images cannot accompany a command.
    #[error("Remove attached images before running a command")]
    ImagesWithCommand,
    /// An image cannot be attached while the draft is a command.
    #[error("Images belong to messages; leave command mode before attaching one")]
    CommandMode,
    /// The host or selected agent does not accept images.
    #[error("This agent cannot receive images; select a vision-capable agent or remove them")]
    ImagesUnsupported,
    /// The composer already holds the maximum images, or one is too large.
    #[error("Attachment limit: four images, up to 4 MiB each")]
    ImageLimit,
    /// The form is awaiting host validation.
    #[error("Waiting for form validation")]
    FormPending,
    /// A required form field is empty.
    #[error("{0} is required")]
    RequiredField(String),
    /// The run cannot be interrupted through this view.
    #[error("This execution cannot be interrupted through this connection")]
    NotInterruptible,
    /// An interruption is already awaiting its reply.
    #[error("Interruption already requested")]
    AlreadyInterrupting,
    /// The permission is no longer waiting for an answer.
    #[error("This permission is no longer waiting for an answer")]
    PermissionSettled,
}

impl Session {
    /// Add a normalized clipboard image without submitting it to the host.
    pub fn add_image(&mut self, image: ImageUpload) -> Result<(), SessionError> {
        if self.snapshot.as_ref().is_none_or(|s| !s.can_attach_images) {
            return Err(SessionError::ImagesUnsupported);
        }
        if self.input_action().is_some() || self.editor.text().starts_with(self.command_prefix) {
            return Err(SessionError::CommandMode);
        }
        if self.images.len() >= MAX_IMAGES || image.data.len() > MAX_IMAGE_BASE64_BYTES {
            return Err(SessionError::ImageLimit);
        }
        self.images.push(image);
        Ok(())
    }

    /// Remove the most recently attached image.
    pub fn remove_last_image(&mut self) {
        self.images.pop();
    }

    /// Whether an image submission is waiting for host validation and persistence.
    pub fn images_pending(&self) -> bool {
        self.pending
            .values()
            .any(|command| matches!(command, Command::SubmitImages { .. }))
    }

    /// Whether the current form is awaiting validation by the host.
    pub fn form_pending(&self) -> bool {
        self.pending
            .values()
            .any(|c| matches!(c, Command::SubmitForm { .. }))
    }

    pub(crate) fn clear_composer(&mut self) {
        self.editor.clear();
        self.images.clear();
        self.draft_epoch = self.draft_epoch.wrapping_add(1);
        self.refresh_menu();
    }

    /// Composer action selected by the current draft, if advertised by the host.
    pub fn input_action(&self) -> Option<&InputAction> {
        self.snapshot.as_ref()?.input_actions.iter().find(|action| {
            action.prefix != self.command_prefix && self.editor.text().starts_with(action.prefix)
        })
    }

    /// Interpret input using only host-advertised commands.
    pub fn input_command(&self) -> Result<Command, SessionError> {
        let snapshot = self.snapshot.as_ref().ok_or(SessionError::WaitingForHost)?;
        let text = self.editor.text().trim();
        if text.is_empty() && self.images.is_empty() {
            return Err(SessionError::EmptyMessage);
        }
        if self.images_pending() {
            return Err(SessionError::ImagesPending);
        }
        if !self.images.is_empty()
            && (self.input_action().is_some() || text.starts_with(self.command_prefix))
        {
            return Err(SessionError::ImagesWithCommand);
        }
        if let Some(action) = self.input_action() {
            let arguments = &self.editor.text()[action.prefix.len_utf8()..];
            if arguments.trim().is_empty() {
                return Err(SessionError::EmptyActionText(action.prefix));
            }
            return Ok(Command::Invoke {
                name: action.command.clone(),
                arguments: arguments.into(),
            });
        }
        if let Some(command) = text.strip_prefix(self.command_prefix) {
            return self.invocation(command);
        }
        // During a turn, only a message the host hands to that turn.
        let steering = self.can_steer() && self.images.is_empty() && self.interrupting.is_none();
        if (self.is_executing() && !steering) || self.message_pending() {
            return Err(SessionError::Busy);
        }
        if !self.images.is_empty() {
            if !snapshot.can_attach_images {
                return Err(SessionError::ImagesUnsupported);
            }
            return Ok(Command::SubmitImages {
                agent: snapshot.selected_agent.clone(),
                message: Box::new(ImageMessage::new(self.editor.text(), self.images.clone())),
            });
        }
        Ok(Command::Submit {
            agent: snapshot.selected_agent.clone(),
            text: self.unescaped_draft().into(),
        })
    }

    fn invocation(&self, command: &str) -> Result<Command, SessionError> {
        let (name, arguments) = command
            .split_once(char::is_whitespace)
            .unwrap_or((command, ""));
        if !self.advertises(name) {
            return Err(SessionError::UnknownCommand {
                prefix: self.command_prefix,
                name: name.chars().take(64).collect(),
            });
        }
        Ok(Command::Invoke {
            name: name.into(),
            arguments: arguments.trim_start().into(),
        })
    }

    /// A leading backslash escapes an advertised action prefix into text.
    fn unescaped_draft(&self) -> &str {
        let text = self.editor.text();
        text.strip_prefix('\\')
            .filter(|rest| {
                self.snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot
                        .input_actions
                        .iter()
                        .any(|action| rest.starts_with(action.prefix))
                })
            })
            .unwrap_or(text)
    }

    fn message_pending(&self) -> bool {
        self.pending
            .values()
            .any(|c| matches!(c, Command::Submit { .. } | Command::SubmitImages { .. }))
    }

    /// Whether `command` invokes an advertised command that takes no choices,
    /// so pressing Enter on its completion can send it directly.
    pub(crate) fn is_complete_invocation(&self, command: &Command) -> bool {
        matches!(command, Command::Invoke { name, .. } if self.snapshot.as_ref().is_some_and(
            |snapshot| snapshot.commands.iter().any(|spec| spec.name == *name && spec.choices.is_empty())
        ))
    }

    /// Construct a scoped request. Call [`Self::sent`] only after queue acceptance.
    pub fn request(&mut self, command: Command) -> Result<Request, SessionError> {
        let snapshot = self.snapshot.as_ref().ok_or(SessionError::WaitingForHost)?;
        if self.pending.len() >= MAX_PENDING_REQUESTS {
            return Err(SessionError::TooManyPendingRequests);
        }
        match &command {
            Command::SubmitForm { .. } if self.form_pending() => {
                return Err(SessionError::FormPending);
            }
            Command::Interrupt { run } => {
                if !snapshot.can_interrupt || !self.is_live(run) {
                    return Err(SessionError::NotInterruptible);
                }
                if self.interrupting.is_some() {
                    return Err(SessionError::AlreadyInterrupting);
                }
            }
            Command::Decide { permission, .. }
                if !snapshot.permissions.iter().any(|p| &p.id == permission)
                    || self.deciding(permission) =>
            {
                return Err(SessionError::PermissionSettled);
            }
            _ => {}
        }
        self.next_request = self
            .next_request
            .checked_add(1)
            .ok_or(SessionError::RequestIdsExhausted)?;
        Ok(Request::new(
            RequestId::new(self.next_request),
            OperationId::new(uuid::Uuid::new_v4().to_string()),
            snapshot.conversation.clone(),
            command,
        ))
    }

    /// Record a request only after it was accepted by the connection.
    pub fn sent(&mut self, request: &Request) {
        match &request.command {
            Command::Interrupt { run } => self.interrupting = Some(run.clone()),
            Command::Decide { permission, .. } => {
                self.deciding.insert(permission.clone());
            }
            _ => {}
        }
        self.pending.insert(request.id, request.command.clone());
    }

    /// Complete command names or first arguments from host-supplied choices.
    pub fn complete(&mut self) {
        if self
            .snapshot
            .as_ref()
            .is_none_or(|snapshot| snapshot.commands.is_empty())
        {
            return;
        }
        if self.editor.text().is_empty() {
            self.editor.set(self.command_prefix.to_string());
            self.menu.reopen();
            self.refresh_menu();
            return;
        }
        self.menu.reopen();
        self.refresh_menu();
        self.accept_completion();
    }

    /// Refresh completion state after the composer changed.
    pub(crate) fn refresh_menu(&mut self) {
        let commands = self
            .snapshot
            .as_ref()
            .map_or(&[][..], |s| s.commands.as_slice());
        self.menu.refresh(
            commands,
            self.editor.text(),
            self.editor.cursor(),
            self.command_prefix,
        );
    }

    /// Insert the highlighted completion. This never sends an execution request.
    pub fn accept_completion(&mut self) -> bool {
        if !self.menu.visible {
            return false;
        }
        let Some(entry) = self.menu.entries.get(self.menu.selected).cloned() else {
            return false;
        };
        self.editor.set(entry.replacement);
        if entry.argument {
            self.menu.suppress(self.editor.text());
        }
        self.refresh_menu();
        true
    }

    /// Move the completion highlight.
    pub fn navigate_completions(&mut self, backwards: bool) {
        self.menu.navigate(backwards);
    }

    /// Close the completion menu until the draft changes.
    pub fn dismiss_completions(&mut self) {
        self.menu.dismiss();
    }
}
