# roomkit-tui-engine

Local adapter for an application-owned RoomKit engine. It depends on the generic
client protocol and in-process transport, and has no terminal dependency.

Hosts may install `SessionLifecycle` through `LocalSession::with_lifecycle`.
Admission runs before input enters history; preparation can be interrupted.
`started`/`finished` surround the entire logical request, not each model or tool
round. `finished` also releases a refused/interrupted admission that never called
`started`. A view detach does not stop callbacks for accepted work, and does not
mean session shutdown. The host supplies bounded callbacks and owns session end.

The application:

1. Constructs its kit with its own store, agents, providers, tools and policies.
2. Creates a `ConversationChannel`, registers it on the kit and attaches it to
   the authorized room. `ConversationChannel::buffered` creates one that shows
   only committed rows, when output must be moderated before it reaches a person.
3. Builds `SessionConfig` with a room, selected agent and authenticated sender,
   and the agent's `ModelInfo` under the provider name the engine reports.
4. Runs `LocalSession::run` with the host endpoint of a
   `roomkit_tui_transport::connection` while a UI consumes the client endpoint.
5. Owns shutdown of the kit and its runtime independently of each view: it
   awaits `ConversationChannel::idle` so the current turn stores its
   decisions, then closes the kit.

`snapshot()` restores a bounded visible timeline from the engine's store. It
checks read access, hides blocked/internal rows and preserves durable identities.
Tool items carry the identity of their run (`agent:trigger`, as announced by
`RunStarted`), which also keeps provider call IDs of different turns apart.
A run identity too long for a terminal, with the permissions derived from it,
keeps a readable prefix and a stable digest of the whole, so restored history
still matches it.
Tool data is forwarded only from authoritative structured content.
Optional public tool intentions are copied from the engine into `ToolCall.intent`,
including live updates and restored history. The host enables their generation
on its agent with `AiChannelOptions { tool_call_intents: true, .. }`. Intentions
never determine tool state or permission decisions; the adapter does not
normalize them a second time.

The engine records a call's start and its end, nothing between: meanwhile the
call waits for the decisions of its round, then executes with the round. A view
therefore shows a started call of a running turn as `Running`, and as `Pending`
while a permission of that turn is unanswered. A refused or expired decision
keeps its round `Pending` until each of its calls ends, so a call is never shown
running when it may not run. Restored history keeps the engine's states.

The adapter drains engine streams even when a view disconnects. Queue overflow
ends a lagging attachment with `AdapterError::Lagged`, requiring reattachment
after the active response finishes; it does not interrupt the engine. Submission
operation IDs are scoped to the room, channel and host-assigned sender, then
deduplicated by the engine, including across SQLite restarts.

Accepted submissions and `CommandResult::Execute` publish `OperationStarted`
before any run and `OperationFinished` after processing and cleanup. These events
carry the request's stable operation identity and conversation. Errors before
streaming and engine policy blocks settle as failed operations without inventing
a run. Replayed submissions can complete without a new run. Run events retain
their own outcomes; successful processing does not mean every run succeeded.
The conversation stays busy throughout admission and finalization; wait for
`OperationFinished` before restoring an idle snapshot or switching views.

An agent registered with `AiChannelOptions::steer_room_turns` takes a message
sent while its turn runs. The engine reads it from the agent's registered
channel (`RegisteredChannel::steers_room_turns`); the host declares nothing. The
view then accepts a message while that agent's run streams to this console and
is not being interrupted. The message passes the host's
`SessionLifecycle::admit` and is committed at once, without an operation of its
own. RoomKit hands it to the running turn, which reads it at its next step: it
starts no turn, so no `started`, and `Event::MessageSteered` names that turn's
run, as the message is committed. Should the turn no longer read its queue when
the message's delivery reaches it, already writing its last answer or ended by a
cut, the message gets a turn of its own, whose `RunStarted` names it in
`answers`. The host hears that turn start through `started` once the agent has
prepared it (memory, `BEFORE_AI_GENERATION` hooks), before its loop asks the
model or calls a tool; Esc drops a `started` still running. The view can
interrupt the turn, and its decisions are stored. Anything else holding the room
keeps a message busy: the host preparing a turn, work started by a command, a
turn being interrupted, another agent's stream, images. A buffered console never
steers, and its snapshot says so.

`Command::Answer` starts a turn for what the conversation already holds, as for
a message handed to a turn that was cut before reading it. The agent is told so
by an instruction, which is never stored or shown, and reads the message where
it is. The host's gate holds as for any turn: `SessionLifecycle::admit_answer`,
which is `admit` unless the host refines it, then `started` and `finished`. While
anything holds the room it is refused as busy, before any host call, and never
handed to the running turn. A message still under delivery holds the room. A
`RunStarted` naming it in `answers` says a turn of its own answers it: a client
that asks again past it gets a second answer.

Applications can set `SessionConfig::commands` and use `run_with_commands` with
their own `CommandHandler`. Commands are never sent to the model and default to
idle-only. A handler may opt individual control commands into `allow_while_busy`;
conversation switches remain idle-only, and `Event::TitleChanged` renames the view
while it executes. A handler can publish generic choice selectors
or request a different `SessionConfig`; the adapter validates the replacement
before changing the view. Failed switches preserve the current conversation. A
view whose snapshot a terminal would refuse, such as a command with a space in
its name, is never sent: a switch to it is rejected as invalid, and attaching it
makes `run` fail with `AdapterError::InvalidSnapshot`.
The application owns command authorization, persistence and retry deduplication.

`SessionConfig::input_actions` optionally maps composer prefixes to advertised
commands. The TUI sends the remaining input verbatim; a leading backslash escapes
an action prefix. Hosts without this capability receive ordinary text.

A handler can return `CommandResult::Execute(CommandExecution::new(...))` for
long-running work. The callback starts only after acceptance and receives an
`ExecutionContext` for item updates and cooperative cancellation. Its run
identity is at most 226 bytes, so the approvals derived from it fit a
terminal's identity bound: a longer one is rejected and nothing starts. A run
identity that repeats the last run of the room, while a view watches it, is
rejected too: a terminal ignores it.
Return the outcome after cleanup and durable writes. The adapter publishes run
activity and completion, rejects concurrent submissions, and keeps opted-in
controls available.
The host owns resource shutdown, authorization and operation deduplication; neither
view detachment nor dropping the adapter cancels this application-owned work.

For streaming channels, a host's `BEFORE_TOOL_USE` hook, registered with
`kit.hook(HookTrigger::BeforeToolUse, ..)`, can await
`ConversationChannel::ask_tool_permission(&event, request)` with the hook's
`ToolCallEvent` before allowing the concrete action:

```rust
let request = PermissionRequest::new("Delete the build?", "rm -rf build", Duration::from_secs(60))
    .with_summary("Deleted the build directory");
let decision = channel.ask_tool_permission(&event, request).await;
```

This retains the call identity so its confirmed decision appears beside the
tool when restoring history. `ask_permission(room, agent, request)` serves
actions without an engine tool call, and goes to the most recent run of that
room and agent. Work a `CommandExecution` runs asks through
`ExecutionContext::ask_permission` instead: the request stays in its run
whatever else runs in the room, such as a turn for another channel's message,
and only the person who invoked the command may answer. Interrupting the work
denies its pending requests and every request it asks afterwards, and its
decisions are stored once it settles. The bundled terminal follows every live
run of the room, so the work's requests stay on screen while such a turn runs.
The broker scopes answers to the current room, agent, run and one decision.
When a person started the run from a view, only that person may answer; a
decision is recorded only once the waiting action has received it.
No live view, timeout, cancellation, oversized detail or the last view's
disconnection refuses the waiting action. A request too large to show in full,
beyond the pending limit the channel's rooms share, or made while approval
history cannot be stored, is refused with a notice saying why, once per run and
reason, so the denial is not mistaken for the person's. An action already
allowed continues independently of the view. A hook must keep its timeout above
the requested approval timeout. Buffered channels refuse requests for
interactive approval. The broker does not implement tool policy; the host
chooses which calls need it.
Explicit decisions are saved after execution settles, outside model context.
Restoration changes their display order only: older receipts without a call
identity appear before their turn's final answer. If the linked action is outside
the visible history, the receipt keeps its stored position; it is never attached
to a different action by matching its label.

`SessionConfig::mode` optionally advertises a generic `ModeState`. Its command
accepts an explicit choice value; publish `Event::ModeChanged` after applying
policy, from a command result or from `CommandHandler::subscribe`. The adapter
retains the mode, and the title of an `Event::TitleChanged`, for its next snapshot
and the configuration its handler receives. It keeps neither when a terminal
would ignore it: a mode must fit beside the advertised commands
(`ModeState::fits`), a title must fit in 256 bytes. A model change needs a
replacement `SessionConfig`, because the configured model carries its context
capacity. Mode assignments and retry handling belong to the application, as does
the meaning of each choice.

Scoped interruption uses the detached delivery handle of the execution started
by this attachment. Wrong-room, stale and foreign run requests are refused.
Acknowledgment only requests cancellation; the terminal outcome follows engine
cleanup, including pending permissions and tool finalizers, once the cut turns
are over and the run's stream to this console has ended. RoomKit stores the
stopped answer after its turns are over: a buffered console, which has no
stream, may receive it after the run and its operation have finished. Completed
external effects and application-owned background processes remain. The shared
kit stays open, and detaching still does not implicitly interrupt.

The adapter projects provider-reported measurements from AI turn records,
distinguishing cumulative consumption from the last generation, and from each
generation as it ends on a streaming console (a buffered one reads them at the
turn's end). It restores idle
conversations; a remote service, automatic event replay and authentication are
not implemented. It is not an authentication boundary for untrusted callers.

A view reserves its room atomically before starting work, so two views never
start two operations in one room, and it keeps forwarding events while it awaits
the application's command handler. Before closing the engine, an application
can await `ConversationChannel::idle` so the current turn settles and stores its
decisions. `SessionConfig.model` must name the provider the engine reports in
its usage metadata (`OpenAIAIProvider` for `roomkit-openai`); otherwise context
and token usage are not shown. Use `ConversationChannel::buffered` for rooms
whose output hooks must review an answer before it is displayed.

The API is unpublished and may change before 1.0. Engine dependencies use one
pinned revision from `roomkit-rs/main`; no adjacent checkout is required.

## Measurements

Each `StreamItem::Usage` becomes an `Event::ContextUpdated` for the running turn.
A stopped turn keeps what its generations told: its record reaches the view as
`Event::UsageUpdated`, and a later restore reads it again. A run whose request
the provider refused past the model's context window, which the kit's
compaction could not fix, sends `Event::ContextExceeded` before it ends, with
its `context_limit` notice: no measurement of that request comes.

A host that changes the context outside a turn, such as compacting the
conversation between turns, publishes its estimate as `Event::ContextEstimated`,
from a command result, from `CommandHandler::subscribe` or while work run as
`CommandResult::Execute` goes. The adapter gives it the configured capacity, as
it does the engine's measurements. Stop publishing it once a request of the
agent was measured, for instance when `SessionLifecycle::finished` reports a
completed turn (a refused or interrupted one may have measured nothing): a
view attached afterwards reads that newer measurement from its snapshot, and
would show the estimate instead.

`SessionConfig.context_capacity` is the host-confirmed capacity of its configured
model. With matching `ai_model`/`ai_provider` metadata, `ai_last_usage` supplies
the last completed generation's input (including cached tokens) plus output.
This is the last measured exchange, not a prediction of the next request or a
sum of the session. Unknown counters or capacity remain unknown. `ai_usage`
feeds a separate latest-execution total, marked partial when accounting is
incomplete. Neither figure compacts or changes the model's context.

A snapshot restores the measurements of the agent's latest visible row that speaks
for them: its latest message, or a later row holding a turn's record, which rides
the turn's last message, or its last row when it wrote none. When that row holds no
record, or one on another model, the snapshot restores no measurement.
New runs and model changes clear old measurements. Hosts configuring OpenAI
use the provider identity `OpenAIAIProvider`, matching the engine's attribution.

`TokenUsage` counts the tokens a turn read from the prompt cache and, apart, those
it wrote there (`cache_creation_input_tokens`), both inside its input.

`SessionConfig::pricing` is the configured model's list price, the kit's
`ModelPricing`. With it, a snapshot holds the conversation's cost, every turn of its
history read again, and each turn's report sends `Event::CostUpdated`: the whole
conversation's cost, in millionths of the price's currency. A turn of another model,
or one with no whole report, has no price and leaves the cost partial.
`conversation_cost` gives a host the same figure, for an exit report, and
`turn_cost` one turn's. A price that changes past a request's input, such as
OpenAI's long-context rate, applies to the whole turn when its last request went
past it, and to none of it otherwise: such a turn's cost is an estimate. No price,
no cost: a host leaves it unset for a subscription billed by plan.


`CommandHandler` also receives `SubmitForm` for advertised commands, with the
same conversation and idle checks as other configuration controls. It must
validate its own issued form token and return errors safe for the terminal.
`SessionConfig::submission_error` disables ordinary messages during application
setup without disabling configuration commands. `actor_names` supplies display
names while stable engine channel identities continue to own history.
