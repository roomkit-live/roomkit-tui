//! The conversation history, laid out from the newest item backwards and only
//! as far as the frame needs, so a long session costs what the screen shows.

use roomkit_tui_protocol::{Item, Snapshot, ToolCall, ToolState};
use unicode_width::UnicodeWidthStr;

use super::activity::Group;
use super::{Line, Tone, ToolTarget, View, approval, line, message, span, spinner};

/// Laid-out history, possibly without its oldest part.
#[derive(Default)]
pub(super) struct Timeline {
    pub lines: Vec<Line>,
    pub targets: Vec<ToolTarget>,
    /// Row after the focused tool, when tool focus is active.
    pub focused_end: Option<usize>,
}

/// One item, or consecutive tool calls shown as a single activity.
enum Segment<'a> {
    Group(Group<'a>),
    Item(&'a Item),
}

#[derive(Default)]
struct Block {
    lines: Vec<Line>,
    /// Targets with rows relative to the block.
    targets: Vec<ToolTarget>,
    /// Row after the focused tool within the block, before its gap.
    focused_end: Option<usize>,
}

/// Lay out at least `rows + scroll` lines of the newest history. With tool
/// focus, continue until the focused tool has `rows` lines above its end.
pub(super) fn timeline(view: &View<'_>, cols: usize, rows: usize, scroll: usize) -> Timeline {
    let Some(snapshot) = &view.session.snapshot else {
        return Timeline::default();
    };
    if snapshot.items.is_empty() {
        return Timeline {
            lines: empty_state(view, snapshot),
            ..Timeline::default()
        };
    }
    let segments = segments(view, &snapshot.items);
    let focus = view.session.has_tool_focus();
    let mut blocks = Vec::new();
    let mut total = 0;
    let mut below_focus = None;
    for segment in segments.iter().rev() {
        let block = render(view, snapshot, segment, cols);
        if let Some(end) = block.focused_end.filter(|_| below_focus.is_none()) {
            below_focus = Some(total + block.lines.len() - end);
        }
        total += block.lines.len();
        blocks.push(block);
        let shown = below_focus.is_some_and(|below| total - below >= rows);
        if total >= rows.saturating_add(scroll) && (!focus || shown) {
            break;
        }
    }
    assemble(blocks, below_focus)
}

/// Join blocks laid out newest first; `below_focus` counts lines after the
/// focused tool's end.
fn assemble(blocks: Vec<Block>, below_focus: Option<usize>) -> Timeline {
    let mut timeline = Timeline::default();
    for block in blocks.into_iter().rev() {
        let offset = timeline.lines.len();
        timeline
            .targets
            .extend(block.targets.into_iter().map(|mut target| {
                target.row += offset;
                target
            }));
        timeline.lines.extend(block.lines);
    }
    timeline.focused_end = below_focus.map(|below| timeline.lines.len() - below);
    timeline
}

fn segments<'a>(view: &View<'_>, items: &'a [Item]) -> Vec<Segment<'a>> {
    let grouping = view.options.tool_activity && !view.session.expanded;
    let mut segments = Vec::new();
    let mut index = 0;
    while index < items.len() {
        if grouping && let Some(group) = Group::from_items(&items[index..]) {
            index += group.len;
            segments.push(Segment::Group(group));
        } else {
            segments.push(Segment::Item(&items[index]));
            index += 1;
        }
    }
    segments
}

fn render(view: &View<'_>, snapshot: &Snapshot, segment: &Segment<'_>, cols: usize) -> Block {
    let mut block = match segment {
        Segment::Group(group) => group_block(view, snapshot, group, cols),
        Segment::Item(Item::Approval(record)) => Block {
            lines: approval::receipt(view.session, record, cols),
            ..Block::default()
        },
        Segment::Item(Item::Message(message)) if !message.text.trim().is_empty() => Block {
            lines: message::render(view, message, cols),
            ..Block::default()
        },
        Segment::Item(Item::Tool { actor, run, call }) => {
            tool_block(view, snapshot, (actor, run), call, cols)
        }
        // Blank messages and items from a newer host take no space.
        Segment::Item(_) => return Block::default(),
    };
    let gap = usize::from(view.options.message_gap.min(3));
    block.lines.extend((0..gap).map(|_| Vec::new()));
    block
}

fn group_block(view: &View<'_>, snapshot: &Snapshot, group: &Group<'_>, cols: usize) -> Block {
    let focused = group.focused(view.session);
    let row = group.render(cols, view.options, view.elapsed, focused);
    let mut block = Block::default();
    if view.options.selection == crate::SelectionMode::CopyOnSelect {
        block
            .targets
            .push(group.target(&snapshot.conversation, 0, &row));
    }
    block.lines.push(row);
    let current = group.current();
    let details = view.renderers.tool_details(
        (group.actor, group.run, &current.id),
        current,
        cols,
        view.options,
        false,
    );
    block.lines.extend(details.lines);
    block.focused_end = focused.then_some(block.lines.len());
    block
}

fn tool_block(
    view: &View<'_>,
    snapshot: &Snapshot,
    (actor, run): (&roomkit_tui_protocol::ActorId, &roomkit_tui_protocol::RunId),
    call: &ToolCall,
    cols: usize,
) -> Block {
    let session = view.session;
    let focused = session.tool_focused(actor, run, &call.id);
    let (symbol, state, tone) = tool_state(view, call.state);
    let duration = call.duration_ms.map_or(String::new(), |ms| {
        if ms < 1000 {
            format!(" · {ms} ms")
        } else {
            format!(" · {:.1}s", ms as f64 / 1000.0)
        }
    });
    let mut block = Block::default();
    block.lines.push(vec![
        span(if focused { " › " } else { "   " }, Tone::Accent),
        span(format!("{symbol} {}", call.name), tone),
        span(
            format!(" · {state}{duration} · {}", session.actor_name(actor)),
            Tone::Muted,
        ),
    ]);
    let expand = session.tool_expanded(actor, run, &call.id);
    let details =
        view.renderers
            .tool_details((actor, run, &call.id), call, cols, view.options, expand);
    block.lines.extend(details.lines);
    if details.expandable {
        let summary = match (details.expanded, details.truncated) {
            (true, true) => "F2 collapse · view limited to 100 lines",
            (true, false) => "F2 collapse",
            (false, true) => "F2 expand · more details",
            (false, false) => "F2 expand",
        };
        let clickable = view.options.selection == crate::SelectionMode::CopyOnSelect;
        let hint = format!("   ╰─ {summary}{}", if clickable { " · click" } else { "" });
        if clickable {
            block.targets.push(ToolTarget {
                conversation: snapshot.conversation.clone(),
                row: block.lines.len(),
                start: 3,
                end: hint.width().min(cols),
                actor: actor.clone(),
                run: run.clone(),
                call: call.id.clone(),
            });
        }
        let tone = if focused { Tone::Accent } else { Tone::Muted };
        block.lines.push(line(hint, tone));
    }
    block.focused_end = focused.then_some(block.lines.len());
    block
}

fn tool_state(view: &View<'_>, state: ToolState) -> (&'static str, &'static str, Tone) {
    match state {
        ToolState::Pending => ("◇", "pending", Tone::Warning),
        ToolState::Running => (spinner(view.options, view.elapsed), "running", Tone::Accent),
        ToolState::Succeeded => ("✓", "completed", Tone::Success),
        ToolState::Failed => ("×", "failed", Tone::Error),
        ToolState::Denied => ("⊘", "denied", Tone::Warning),
        ToolState::Cancelled => ("−", "cancelled", Tone::Muted),
        _ => ("?", "unknown state", Tone::Muted),
    }
}

fn empty_state(view: &View<'_>, snapshot: &Snapshot) -> Vec<Line> {
    let session = view.session;
    let mut lines = vec![
        line("", Tone::Text),
        line(" Start a conversation", Tone::Primary),
        line(
            format!(
                " {} is ready when you are.",
                session.actor_name(&snapshot.selected_agent)
            ),
            Tone::Muted,
        ),
    ];
    if !snapshot.commands.is_empty() {
        lines.push(line(
            format!(
                " Type {} to explore this application's commands.",
                session.command_prefix
            ),
            Tone::Muted,
        ));
    }
    lines
}
