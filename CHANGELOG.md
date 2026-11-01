# Changelog

All notable changes to this project are documented in this file. The format
is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Initial public release.

### Added

- `Event::TitleChanged` renames the displayed conversation in place, even
  while it executes: the header and `SessionObservation::title` follow, and
  the history, draft, scroll position and selection are kept. The engine
  keeps each mode or title change a host publishes, from a command result or
  its subscription, so later snapshots show it.
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
