# roomkit-tui-protocol

Runtime-independent, serializable interaction contracts for application-owned
AI conversations. This crate performs no I/O and has no terminal, engine,
provider or async runtime dependency.

Hosts publish snapshots, messages, tool states, permissions and model context.
Clients send correlated requests. A reply acknowledges a request; a separate
event confirms execution completion. Tool data remains structured and identities
are scoped to conversations and executions.

Each request also carries a stable `OperationId`, kept when the client retries,
so hosts can deduplicate retried work. A retry must keep its original payload.
Permission answers are scoped to one outstanding decision, never a persistent
grant.

The optional `OperationStarted` / `OperationFinished` pair correlates accepted
work with the request's stable `OperationId` and conversation. Publish a start
after acceptance, before any run; publish exactly one finish after processing
and cleanup, including errors or policy blocks before streaming. A rejected
request starts no operation. Immediate control commands need no lifecycle.
An operation can finish without a run, for example an idempotent replay.
Its outcome describes processing, while `RunFinished` describes the run; a
completed operation must not erase a failed run. Hosts that only publish run
events remain supported. Idle snapshots do not replay historical completions.

A host advertising `Snapshot::can_steer` accepts a `Command::Submit` while its
agent's turn runs and hands the message to that turn. `Event::MessageSteered`
names the run it went to; should that run no longer read it, the run that
answers it says so in `RunStarted::answers`. Older hosts send neither, and a
terminal treats an absent `answers` as a run that answers no message.

Hosts can open a conversation-scoped `FormSpec` through `Event::Form`. Text,
multiline, choice and secret fields use product-independent definitions.
Submitting sends `Command::SubmitForm` to its advertised command, with an opaque
host-issued revision token. Values are separate from messages and their `Debug`
representation is redacted. Transport serialization necessarily carries values;
remote hosts must protect their transport and never log raw requests. Hosts
validate token, conversation, fields and business rules, and return a safe error
through `Reply`. Secret defaults must be empty; the TUI discards them defensively.

Forms can include plain text `description` guidance and a `dismiss` argument for
host cleanup. `with_async_completion()` keeps an accepted form visible and
stops editing while background work continues. Acknowledge submissions promptly
so the host can process cancellation. Publish `Event::FormCompleted` with the
original form ID and a result panel; the view only replaces that still-open
form in the matching conversation, never a dismissed or newer form. Ordinary
forms still close on acceptance.

Every public type is `#[non_exhaustive]`: build structs with their
constructors (`Snapshot::new`, `ToolCall::new(...).with_text(...)`, …) and
match enums with a wildcard arm. A refused request carries a `Rejection` with a
stable `ErrorCode` and display text. Enums a terminal receives have an `Unknown`
variant, so an older peer ignores what a newer one adds instead of failing to
parse; `Decision` has none, so a newer answer is never read as an approval.
Shared bounds are in `limits`. The rules terminals apply are public, so a host
can check what it sends: `Snapshot::validate` says why a whole snapshot would be
refused (`SnapshotError`, with the position of the refused entry),
`ModeState::fits` covers a mode change and `Permission::fits` an approval
request.

This is an early release; the contract can change before 1.0.
