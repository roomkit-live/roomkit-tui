# roomkit-tui

Embeddable Rust terminal for application-owned AI conversations. Built on
Crossterm, with a pure session projection and layout renderer.

The application owns execution and cleanup. Closing a terminal disconnects its
view and does not itself promise that a shared engine or an external operation
has stopped. The terminal never executes providers, tools or slash commands.

This is an unpublished 0.1 release. The API may change before 1.0.

## Getting started

Create a bounded `connection`, run the host independently and pass the client
to `run`. The host supplies its agents, commands, model settings and tool events.
Optional `ToolRenderer` implementations format structured domain data. Unknown
tools have a generic presentation.

Use `run_with_options` to provide `UiOptions`: application name, plain-text logo,
semantic `Theme`, command prefix, color policy, animations and mouse selection.
Dark, light and terminal-palette presets are available. Automatic colors honor
a nonempty `NO_COLOR` and `TERM=dumb`; applications can also explicitly disable
color or motion. State labels, selection markers and borders remain readable
without color. Theme selection is explicit rather than guessing the background.
The library does not load configuration files or register commands.

The default presentation uses RoomKit's rounded frame and four indigo tiles,
with indigo and cyan accents elsewhere.

## Messages

Messages render as CommonMark/GFM with headings, emphasis, lists, task lists,
quotes, visible link destinations, tables and fenced code. Bundled syntax
grammars color code without executing it or loading workspace files. Semantic
colors belong to `Theme`; attributes such as bold remain in monochrome mode.
Tables fall back to labeled values when columns cannot fit. Outer blank lines
are removed, while code indentation and interior blank lines are preserved.
The protocol keeps original text; `UiOptions::markdown = false` shows literal
syntax. Selection copies the visible rendered text.

`UiOptions::message_gap` controls space between transcript items.

Hosts can enable `UiOptions::highlight_own_messages` to show submitted messages
on the same surface as the composer, without an author heading. Set
`Snapshot::local_actor` to the identity that submits messages through this view;
other humans and agents keep their names. If the identity is absent, all author
headings remain visible. These messages reuse the composer's prompt marker and
horizontal inset, with a filled blank row above and below the text. This
presentation setting never changes stored messages or authorization.

When the viewport is above the latest activity, a **Back to bottom** control
appears at the right of the activity row. Clicking it or pressing **Ctrl+End**
returns directly to the latest message, preserving the draft and expanded tools.

## Tools

Hosts may implement `ToolRenderer::render_styled` with semantic `Span` values;
plain `render` / `render_expanded` implementations still work. Custom presenters
own their fields: expansion never adds raw output they chose to omit. Unknown
tools use a JSON-aware fallback for display only. The `content` module provides
literal, JSON and code rendering without knowledge of domain tool schemas.
Tool renderers must be deterministic for a given call; register them again to
invalidate changed presentation settings.

By default, tools have explicit state labels and colors, a compact result
preview (`tool_preview_lines`, four rows by default) and independent detail
expansion with F2 or a click on their disclosure link; Alt+Up/Down changes
keyboard focus. Disclosure controls appear only when expanding reveals
additional content, including preview rows clipped at the current width.
Custom presenters can override `has_expanded_details` with a cheap schema-aware
check; by default the library compares their compact and expanded presentations.

Hosts may set `UiOptions::tool_activity` to replace consecutive tool cards with
one updating public intention and current tool preview per actor/run sequence.
Messages and approval receipts separate sequences. F2 or a click reveals the
technical timeline; F2 returns to the compact view. This changes presentation
only: calls and outcomes remain available, including failures and unfinished
parallel work. Without an intention, the current tool's actual name is shown.
The default keeps individual cards.

## Runs and timing

The activity row tracks observed elapsed time through execution, tools and human
waits, then retains the final duration and outcome until the next run. This is
monotonic time observed by the view, not model inference time. Restored runs
without an observed start stay unknown. Timings are not persisted.
`UiOptions::activity_gap` controls spacing before input (one line by default).
Active runs and tools use a small spinner; disabling animation does not delay
streaming output.

When a host publishes the optional operation lifecycle, `Session::is_executing`
also covers admission before `RunStarted` and cleanup after `RunFinished`.
`execution_outcome` retains failure or interruption even if operation processing
itself succeeds. A permission answer stays open until the host resolves it.

Esc closes an open menu or panel first, then interrupts the active run while
preserving the draft. Ctrl+C clears the draft and interrupts; a second press
within two seconds exits. Typing, pasting, mouse input or waiting disarms the
exit hint. In a form, picker or panel, Ctrl+C first dismisses that view.
Ctrl+D detaches; engine cleanup stays with the owning application. When the host
sends `Event::ExitWarning` (for example running processes), the exit hint names
it and Ctrl+D asks for a second press within two seconds; a held key never
confirms.

## Commands, choices and modes

Typing the configured prefix opens only the host's advertised commands; typing
an argument filters its advertised choices. Enter or Tab inserts a highlighted
choice, and a subsequent Enter sends it to the host. Escape closes the menu
without losing the draft. An exact command without inline argument choices can
also be submitted directly with Enter. No model setting is changed locally by a
selection; model changes remain host-confirmed.

Hosts can send `Event::Choices` to open a separate filterable selector for any
advertised command. Each entry has a label, description and opaque value;
Enter invokes the command with that value, while Escape preserves the draft and
closes the selector. Filtering is local and never sends text to a model.

Hosts can supply `Snapshot::mode` with an advertised command, selected value
and ordered choices. **Shift+Tab** requests the next value, preserving drafts;
`Event::ModeChanged` confirms the effective mode without resetting history or
active executions. Labels and policy meanings belong entirely to the host.
Mode changes do not settle approval requests.

## Composer

The composer is a filled rectangle with a small prompt marker and no title.
Its default height is three rows (one editable row and one padding row per side).
The draft is vertically centered while short; newlines and soft wrapping grow
the surface automatically, up to five editable rows before scrolling internally.
`input_min_lines` and `input_padding` control its height; padding shrinks before
essential controls on small terminals. `Theme::surface` and `Theme::input_text`
set its background and foreground. Monochrome mode uses reverse video to keep
the input surface visible without emitting palette colors.

Applications can supply `input_placeholder`, shown only while the draft is
empty, and `input_prompt`, a marker clipped to four display columns. Both are
plain presentation text, sanitized and never included in submitted messages.
Bracketed pastes preserve newlines and never submit automatically.

## Human approvals

Pending permissions replace the composer with a filled decision card. Review the
full details with PageUp/PageDown or the mouse wheel. Use Up/Down and Enter,
click a choice, or use Ctrl+Y/Ctrl+N. A new card accepts answers only after it
has been shown for half a second (`APPROVAL_ARMING_DELAY`), so a key typed for
the composer never answers a request unseen. Esc interrupts the run without
granting permission. The draft and attachments are preserved; approval keys
never submit them. Shift+Tab changes the host's mode only from the composer,
never while a decision is on screen.

Hosts must keep a request's title within 256 bytes, its details within 64 KiB
and its summary within 512 bytes: the view refuses to show what it cannot show
in full, and the request then expires. Bidirectional text controls are removed,
so the text approved is the text read.

Hosts may provide a concise `Permission.summary` alongside the exact details.
Only a host-confirmed `Item::Approval` produces a history receipt. A reply,
withdrawal or timeout alone does not imply a human decision. Receipts identify
the person, agent and one-time action; authorization does not mean execution
succeeded.

## Forms

Host forms use a separate bounded editor: **Tab / Shift+Tab** change fields,
**Left / Right** cycle choices, **Alt+Enter** adds a line break, **Ctrl+U** clears
the current field, **Ctrl+S** submits, and **Esc** cancels locally. A rejected
submission keeps the draft. Pending submissions cannot be edited or submitted
again. Secrets are masked in frames and clipboard selections, omitted from
Debug output and never entered in conversation input history. The host owns
persistence and validates all submitted values. Limits are 32 fields, 8 KiB per
value and 500 choices per field; undersized terminals request a resize.

## Panels and background work

Hosts can publish `BackgroundChanged` with an advertised command. Its compact
status appears on the activity row; clicking it or pressing F6 invokes the host.
`Panel` opens a bounded read-only view with explicit `Choice` actions.
`PanelUpdated` refreshes the same opaque panel identity without reopening a
dismissed view. Navigation uses arrows/Tab, Enter, PgUp/PgDn and Esc; the
conversation draft stays intact. Application actions remain scoped to the
current conversation.

## Sidebar

Hosts opt in by sending a scoped `Event::SidebarChanged` with a `SidebarSpec`.
Sections can project the current `Agent` or `Context` state, or supply generic
`Entries` with semantic status and optional advertised command actions. The
terminal owns no process, MCP or configuration logic, and a sidebar never
executes an action automatically.

F7 shows or hides the sidebar; F8 focuses it. Up/Down or Tab selects an action,
Enter opens it, and Escape returns to the unchanged composer. Mouse clicks open
entries; the wheel scrolls only the pane under the pointer. Copy selections stay
within the pane where the drag began.

`UiOptions::sidebar_visible`, `sidebar_width` and `sidebar_min_width` configure
presentation. Defaults are visible, 34 columns and a 112-column terminal;
at least 72 columns remain for the conversation. Short or narrow terminals and
modal views temporarily hide the sidebar. Agent, model, effort and context stay
in the footer only when their sidebar sections are absent or the sidebar is
hidden. A snapshot clears sidebar data; the host resends it for the new view.

The projection accepts at most 16 sections and 256 entries, with bounded labels,
details and action arguments. Invalid or out-of-scope updates are ignored.

## Welcome screen

Set `UiOptions::welcome` to a `WelcomeScreen` to center the composer in an
empty, idle conversation. Hosts provide their own logo, wordmark and preferred
width. Empty wordmark artwork uses the application name. All artwork is
sanitized and clipped; it requires no terminal image protocol or external fonts.

The welcome screen temporarily hides the sidebar. The first message or active
execution restores the conversation layout; new empty snapshots restore the
welcome screen. Small windows fall back to the conversation layout without
losing the draft. `UiOptions::bottom_margin` reserves up to three blank rows
below the interface.

## Mouse selection and clipboard

The default `SelectionMode::CopyOnSelect` enables tool clicks and captures
visible text drags, sending an OSC 52 clipboard request on release, subject to
terminal support and permissions (including across SSH or multiplexers). A copy
request is not reported as confirmed clipboard success. Selection is limited to
the visible frame and 64 KiB. Resizing or typing cancels a drag, a drag never
triggers a tool click, and exiting restores the terminal's mouse modes.
`SelectionMode::Native` leaves all mouse behavior to the terminal and disables
in-app clicks and clipboard requests.

## Clipboard images

Hosts opt in with `Snapshot::can_attach_images`. The RoomKit adapter sets it
from `SessionConfig::can_attach_images` and calls
`CommandHandler::prepare_images` to validate, authorize and persist PNG uploads;
its default handler refuses images.
The transport carries base64 PNG bytes, never client filesystem paths.

With a capable host, Ctrl+V reads an image (or text) from the native clipboard.
The composer shows attachment labels; Alt+Backspace removes the last image.
Up to four images, each at most 4 MiB and 16 million pixels, may accompany a
message, including an image-only message. Validation errors preserve the draft.
Images cannot accompany slash commands or composer actions. Native clipboard
access needs a local desktop session; SSH alone cannot expose the clipboard of
the connecting computer. Linux X11, Wayland data-control and macOS backends are
provided by `arboard`.

## Context and usage

Provider reports share the model footer when they fit, with context
tokens/capacity, used and remaining percentages and latest-execution
input/output totals. Narrow windows give these measurements their own line so
long model names do not hide them. Context is the host's last measurement;
missing capacity or counters stay unknown, and estimated values show `~`.
While awaiting a new measurement, the footer retains the previous context with
a `last report` label, and clears it when the model or conversation changes.
These indicators do not implement compaction or account for the next unsent
message.

## Terminal status (OSC 7501)

Set `UiOptions::terminal_status` to `Some(TerminalStatus::new("your-app")?)` to
opt into [OSC 7501 program status](https://www.superlogical.com/rex/docs/build/program-status).
The identity is validated at construction. Reports include no transcript,
permission details, titles or tool arguments. The terminal emits changes on its
own thread after each host event, even when rendering coalesces several updates.
Permissions remain blocked until resolved; active operations cover admission
and finalization. Results persist through redraws and become idle on user input
after display, without changing the session's confirmed outcome.

Reporting is independent of `SessionObserver`, colors and animations. Default
options, `TERM=dumb`, headless renderers and noninteractive invocations emit
nothing. Terminal cleanup attempts to clear the root status on normal exit,
disconnection and recoverable errors; it cannot be guaranteed after a forced
kill or an unusable output stream.

## Session observation

For optional session observation, create a `SessionObserver` and supply a clone
to `run_with_observer(client, renderers, options, observer.clone())`. A headless
consumer uses `Session::default().with_observer(observer.clone())`, or reads
`Session::observation()` directly. `observer.latest()` returns validated current
facts: displayed conversation, title, selected agent, mode, latest operation and
run with their separate outcomes, and outstanding permission IDs. It omits
messages, tool arguments and permission details.

The observer retains only the latest value and starts no callback, thread or
I/O. The application chooses when to poll and owns consumer cleanup; the final
value remains readable after the view closes. Use one handle per session. This
is a current-state view, not a delivery guarantee for every transition or an
audit log. The `order-support --check` example exercises it without a terminal.

## Headless rendering

`render::screen_with_options` produces deterministic styled frames from explicit
UI state and elapsed time, without a terminal or engine. Plain text remains
available through `Frame::lines` for headless consumers and layout checks.
Headless consumers use `Session::apply_at` and the renderer with the same
monotonic epoch; plain `apply` leaves timing unknown. Read state through the
session's accessors (`snapshot`, `permissions`, `draft`, `focus`, …) and call
`Session::sync` with each frame and the time it is shown, so scroll offsets
stay within what was shown and a new approval card arms only once displayed.
`SessionError` explains why a request could not be built.

## Performance and limits

The terminal redraws on input and host events, with a 10 Hz refresh for active
timers and animations. Idle polling does not rebuild frames. A frame lays out
history from the newest item and stops once the screen is full. Message and tool
bodies use separate bounded caches (4 MiB and 500 entries each) with one entry
per message or call, so streaming a long answer never evicts the rest. Source highlighting is limited to
64 KiB and skips syntax parsing for lines over 2 KiB. Markdown views stop at
4096 rows with a notice.

The default view retains 500 items and 64 KiB of text per item; hosts retain the
durable history. Tool details show at most 100 lines when expanded. These are
view limits, not model-context or tool-execution limits.
