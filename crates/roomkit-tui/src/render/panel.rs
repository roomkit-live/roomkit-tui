use super::text::{sanitize, wrap};
use super::{ActionTarget, Frame, Line, Tone, frame, line};
use crate::Session;

pub(super) fn screen(session: &Session, cols: usize, rows: usize) -> Frame {
    let Some(panel) = &session.panel else {
        return frame(Vec::new(), cols, None);
    };
    let slots = panel.spec.actions.len().min(6).min(rows.saturating_sub(9));
    let body_height = rows.saturating_sub(slots + 6);
    let body = wrap(&sanitize(&panel.spec.body), cols.saturating_sub(2));
    let scroll = panel.scroll.min(body.len().saturating_sub(body_height));
    let mut lines = vec![
        line(format!(" {}", panel.spec.title), Tone::Primary),
        Vec::new(),
    ];
    lines.extend(
        body.iter()
            .skip(scroll)
            .take(body_height)
            .map(|text| line(format!(" {text}"), Tone::Text)),
    );
    while lines.len() < body_height + 2 {
        lines.push(Vec::new());
    }
    lines.push(line(
        format!(
            " Lines {}–{} / {}",
            scroll + 1,
            (scroll + body_height).min(body.len()),
            body.len()
        ),
        Tone::Muted,
    ));
    let (rows, targets, cursor) = actions(session, cols, slots, lines.len());
    lines.extend(rows);
    lines.push(line(format!(" {}", session.notice), Tone::Warning));
    lines.push(line(
        " ↑↓/Tab actions · Enter select · PgUp/PgDn output · Esc close",
        Tone::Muted,
    ));
    let mut result = frame(lines, cols, cursor);
    result.action_targets = targets;
    result.panel_scroll = Some((panel.spec.id.clone(), scroll));
    result
}

/// The visible window of actions, their click targets and the cursor row.
fn actions(
    session: &Session,
    cols: usize,
    slots: usize,
    top: usize,
) -> (Vec<Line>, Vec<ActionTarget>, Option<(u16, u16)>) {
    let Some(panel) = &session.panel else {
        return (Vec::new(), Vec::new(), None);
    };
    let first = panel.selected.saturating_sub(slots.saturating_sub(1));
    let (mut lines, mut targets, mut cursor) = (Vec::new(), Vec::new(), None);
    let actions = panel
        .spec
        .actions
        .iter()
        .enumerate()
        .skip(first)
        .take(slots);
    for (i, action) in actions {
        let row = top + lines.len();
        let selected = i == panel.selected;
        if selected {
            cursor = Some((1, row as u16));
        }
        if let Some(snapshot) = &session.snapshot {
            targets.push(ActionTarget {
                conversation: snapshot.conversation.clone(),
                command: panel.spec.command.clone(),
                arguments: action.value.clone(),
                panel: Some(panel.spec.id.clone()),
                sidebar: false,
                row,
                start: 0,
                end: cols,
            });
        }
        let marker = if selected { "›" } else { " " };
        let tone = if selected {
            Tone::Selected
        } else {
            Tone::Accent
        };
        let text = format!(" {marker} {}  {}", action.label, action.description);
        lines.push(line(text, tone));
    }
    (lines, targets, cursor)
}
