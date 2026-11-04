# Changelog

All notable changes to this project are documented in this file. The format
is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- A model's reasoning (RMK-1325): `Event::ThinkingDelta`, which the engine sends
  from the kit's `StreamItem::Thinking`, and `Item::Thinking`, a block the view
  builds. The block shows its last lines and its time while it comes, and folds to
  `∴ Thought for 12.0s` once the model goes on; a click opens it, or F2 once focused,
  which it only is by default with no call in the conversation. Nothing stores it:
  no snapshot holds it, so it does not come back with a conversation.
  Reasoning also tells when a turn read a message it was handed.

### Changed

- The engine follows roomkit-rs c30375c, whose channel, hook and AI APIs it uses
  unchanged (RMK-1324). A model the kit refuses before any request, for the API
  or the tools it serves, reads as `provider_model_refused`.

### Added

- Cut a turn for a message already handed to it (RMK-1311): its line says
  `Ctrl+S to cut it now` while the turn runs. Ctrl+S on an empty composer cuts the
  turn, and `Command::Answer` starts a turn of its own that reads what it was
  handed in the conversation, with nothing new written. A host advertises it with
  `Snapshot::can_answer`, and refines the gate with
  `SessionLifecycle::admit_answer`, which is `admit` by default. The row below
  names Ctrl+S only for what is typed during a run.

### Fixed

- A message handed to the running turn stays below what the turn writes without
  it, up to the turn's next stop (RMK-1317). What follows goes below it, and its
  line turns to `read by the turn`: `Steering::Read`.
- A message written while the answer streams stays below what it had written; a
  finished answer keeps its draft's place, and an answer after blanks stays below
  the tools of its turn.

## [0.2.0] - 2026-11-03

The first release on crates.io: `roomkit-tui-protocol`, `roomkit-tui-transport`
and `roomkit-tui`. `roomkit-tui-engine` stays on Git, as it depends on roomkit-rs
by Git; a host using it takes `roomkit-tui` from the same commit.

### Added

- Send a message now (RMK-1285, RMK-1303): Ctrl+S, or Ctrl+Enter where the
  terminal reports it, interrupts whatever runs, a message's own turn included,
  and sends the draft once nothing runs any more, in a turn of its own. A
  message whose turn is cut stays in the conversation, and that next turn reads
  it. The draft stays in the composer until the host takes it, and only unedited;
  Esc keeps it, and a host that refuses it as busy is asked again for up to 30
  seconds without a run. The terminal asks for the kitty keyboard protocol's
  `DISAMBIGUATE_ESCAPE_CODES` and removes it once on exit or on a panic. In
  terminals with the protocol, Shift+Enter inserts a line break, and Ctrl+[,
  Ctrl+M and Ctrl+I no longer stand for Esc, Enter and Tab.
- Write to the model while it works (RMK-1161). For an agent registered with
  `AiChannelOptions::steer_room_turns`, a streaming console advertises
  `Snapshot::can_steer`: the engine reads it from the agent's registered
  channel, and the host declares nothing (RMK-1256). A message
  submitted while that agent's turn streams, and is not being interrupted,
  goes to the turn, which reads it at its next step, instead of being refused
  as busy. It passes the host's admission like any message.
  `Event::MessageSteered` names the turn, and `Event::RunStarted::answers`
  names the message a run answers: a message the turn no longer read gets a
  turn of its own, which the view shows, can interrupt, and whose decisions
  are stored. The host hears that turn start through
  `SessionLifecycle::started`, before its loop asks the model (RMK-1232).
  Host preparation, command work, another agent's stream, images and
  buffered consoles stay busy. `roomkit-chat` enables it, and
  `scripts/check-steering.py` drives it on a real pseudo-terminal.

### Changed

- `Event::RunStarted` has an `answers` field; constructing it now names one
  (`None` when the run answers no message).
- The engine follows roomkit-rs 9830bc4, whose channel, hook and AI APIs
  follow Python's, and whose OpenAI provider sends an output cap only when
  configured. A round of tool calls that opens by repeating the text of the
  round before reaches the room, and the view, from where it goes on, as in
  Python. A stopped answer is stored once its turns are over: a buffered
  console may receive it after its run ended. Tool approvals are asked from a
  `BEFORE_TOOL_USE` hook registered with `kit.hook`, and `roomkit-openai`
  reports its provider as `OpenAIAIProvider`, the name a view's `ModelInfo`
  must carry for its context and token usage to show.
- `ConversationChannel::ask_tool_permission` refuses a call made outside a
  room: no view can answer it.
- `ConversationChannel` declares a window of 50 recent events, so a turn's
  requester is still found beside an agent declaring no window
  (`max_context_events: 0`), which then reads those 50 events, as every
  agent did before roomkit-rs 93e757c.
- `roomkit-chat` sends RoomKit's default temperature, 0.7, as Python does.

## [0.1.0] - 2026-11-01

Initial public release.

### Added

- `Event::TitleChanged` renames the displayed conversation in place, even
  while it executes: the header and `SessionObservation::title` follow, and
  the history, draft, scroll position and selection are kept. The engine
  keeps each mode or title change a host publishes, from a command result or
  its subscription, so later snapshots show it; a change the terminal would
  ignore is not kept.
- `ExecutionContext::ask_permission` lets host work ask the person who
  invoked the command. The request stays in that run whatever else runs in
  the room, is denied once the work is interrupted, and its decision is stored
  when the work settles.
- A `CommandExecution` is rejected, and nothing starts, when its run identity
  is longer than 226 bytes or repeats the last run of a room a view watches: a
  terminal could not follow it.
- `Snapshot::validate` (with `SnapshotError`, which names the refused
  command, composer action or permission by position), `ModeState::fits`,
  `Permission::fits` and their limits in `limits` let hosts check what they
  send against the rules terminals apply. The engine validates every snapshot
  it sends: a `CommandResult::Switch` to a view a terminal would refuse is
  rejected as invalid and the current view kept, and attaching such a view
  fails with `AdapterError::InvalidSnapshot` before anything is sent.
- `roomkit-tui-protocol`: runtime-independent, serializable requests, events
  and structured results, with scoped identities, stable operation IDs for
  retries, an optional operation lifecycle, host forms, panels and sidebars.
- `roomkit-tui-transport`: bounded, ordered in-process request and event
  queues shared by terminals and hosts.
- `roomkit-tui`: an embeddable Crossterm terminal with:
  - streaming CommonMark/GFM messages with syntax-highlighted code;
  - tool cards with structured results, custom renderers and an optional
    compact activity view;
  - one-time approval cards and host-confirmed decision receipts;
  - host commands with argument menus, choice pickers, interaction modes,
    forms with masked secrets, live panels and background-work status;
  - elapsed run time, interruption, and context and token usage display;
  - several live runs per room, such as host work and an agent turn for
    another channel's message, each with its approval requests on screen;
  - branding, themes, `NO_COLOR` support, a configurable composer, an optional
    sidebar and welcome screen;
  - sidebar meters: a stacked bar and legend for any proportion, and a context
    breakdown whose remainder follows each measurement, with categorical theme
    colors, or distinct glyphs without color;
  - mouse tool expansion, OSC 52 copy, clipboard image attachments and
    optional OSC 7501 program status. A selection in the conversation
    scrolls it, by the wheel or past its edge while the button is held, and
    copies lines no longer on screen; once copied it stays highlighted;
  - passive session observation and deterministic headless rendering;
  - a host `ExitWarning` stating what exiting stops: the exit hint shows it,
    and Ctrl+D then asks for a second press within two seconds.
- `roomkit-tui-engine`: an adapter for application-owned RoomKit engines,
  with streaming, history restoration, approvals bridged from tool hooks,
  scoped cancellation, application commands and long-running host work, safe
  engine diagnostics, usage measurements and host lifecycle callbacks. Started
  tool calls show as running, and as pending while a decision of their turn
  is unanswered.
- Examples: two scripted hosts that need no credentials or network, and
  `roomkit-chat`, which runs the RoomKit engine with an OpenAI-compatible
  endpoint and SQLite persistence. Each has a headless `--check` mode.

### Security

- A new approval card ignores answers until it has been on screen for half a
  second, and Shift+Tab never changes the host's mode while a decision is on
  screen. A card that cannot be shown in full asks for a larger terminal.
- Approvals are shown only in full; bidirectional text controls are removed.
- Only the person who started a run can answer its approvals, and a decision
  is recorded only once the waiting action received it.
- A view reserves its room atomically, so concurrent views never start two
  operations in one room.
- Host snapshots, approvals and command arguments are bounded.
- A panic on the terminal thread restores the terminal before it is reported.
- `roomkit-chat` refuses to send an API key over plain HTTP to another host.

[Unreleased]: https://github.com/roomkit-live/roomkit-tui/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/roomkit-live/roomkit-tui/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/roomkit-live/roomkit-tui/releases/tag/v0.1.0
