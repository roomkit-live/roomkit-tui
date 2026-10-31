//! A host form: its fields, the field being edited and the save controls.
//! Secrets are masked and choices show their labels, never their values.

use std::time::Duration;

use roomkit_tui_protocol::FieldKind;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::footer::quit_hint;
use super::text::{fit, sanitize, wrap};
use super::{Frame, Line, Tone, frame, line};
use crate::{FormState, Session};

/// Rows of the edited field's input area.
const INPUT_ROWS: usize = 3;

pub(super) fn screen(session: &Session, cols: usize, rows: usize, elapsed: Duration) -> Frame {
    let Some(form) = &session.form else {
        return frame(Vec::new(), cols, None);
    };
    let mut lines = vec![
        line(format!(" {}", form.spec.title), Tone::Primary),
        line(" Changes apply when you save", Tone::Muted),
        line("", Tone::Text),
    ];
    lines.extend(fields(form, rows.saturating_sub(12).max(1)));
    let field = &form.spec.fields[form.selected];
    lines.push(line(
        format!(
            " Field {} / {} · Tab next · Shift+Tab previous",
            form.selected + 1,
            form.editors.len()
        ),
        Tone::Muted,
    ));
    lines.push(line(format!(" {}", field.help), Tone::Accent));
    lines.push(line(format!(" {}", field.label), Tone::Primary));
    let (input, (column, row)) = input(form, cols.saturating_sub(4));
    let cursor = Some(((column + 2) as u16, (lines.len() + row) as u16));
    lines.extend(input);
    lines.extend(controls(session, &field.kind, elapsed));
    frame(lines.into_iter().take(rows).collect(), cols, cursor)
}

/// The visible window of fields, keeping the selected one in view.
fn fields(form: &FormState, count: usize) -> Vec<Line> {
    let start = form.selected.saturating_sub(count - 1);
    let fields = form.spec.fields.iter().enumerate().skip(start).take(count);
    fields
        .map(|(index, field)| {
            let value = match &field.kind {
                FieldKind::Secret if form.editors[index].text().is_empty() => {
                    "(unchanged / empty)".into()
                }
                FieldKind::Secret => "••••••••".into(),
                _ => form.display_value(index).replace('\n', " ↵ "),
            };
            let selected = index == form.selected;
            let marker = if selected { "›" } else { " " };
            let required = if field.required { " *" } else { "" };
            let tone = if selected { Tone::Selected } else { Tone::Text };
            line(
                format!(" {marker} {}{required}: {value}", field.label),
                tone,
            )
        })
        .collect()
}

/// The edited field's text and caret `(column, row)` within its area.
fn input(form: &FormState, width: usize) -> (Vec<Line>, (usize, usize)) {
    let field = &form.spec.fields[form.selected];
    let editor = &form.editors[form.selected];
    let prefix = &editor.text()[..editor.cursor()];
    let (text, prefix) = match field.kind {
        FieldKind::Secret => (
            "•".repeat(editor.text().graphemes(true).count()),
            "•".repeat(prefix.graphemes(true).count()),
        ),
        FieldKind::Choice { .. } => {
            let label = sanitize(&form.display_value(form.selected));
            (label.clone(), label)
        }
        _ => (sanitize(editor.text()), sanitize(prefix)),
    };
    let mut rows = wrap(&text, width);
    let before = wrap(&prefix, width);
    let mut caret = (
        before.last().map_or(0, |line| line.width()),
        before.len().saturating_sub(1),
    );
    if caret.0 == width {
        caret = (0, caret.1 + 1);
    }
    while rows.len() <= caret.1 {
        rows.push(String::new());
    }
    let offset = caret.1.saturating_sub(INPUT_ROWS - 1);
    let lines = (offset..offset + INPUT_ROWS)
        .map(|index| {
            let text = fit(rows.get(index).map_or("", String::as_str), width);
            let padding = " ".repeat(width.saturating_sub(text.width()));
            line(format!("  {text}{padding}  "), Tone::Input)
        })
        .collect();
    (lines, (caret.0, caret.1 - offset))
}

fn controls(session: &Session, kind: &FieldKind, elapsed: Duration) -> [Line; 3] {
    let editing = match kind {
        FieldKind::Choice { .. } => " ← / → choose a value",
        FieldKind::Multiline => " Alt+Enter newline · Ctrl+U clear field",
        _ => " Ctrl+U clear field",
    };
    let saving = quit_hint(session, elapsed).unwrap_or_else(|| {
        if session.form_pending() {
            "Saving…".into()
        } else {
            "Ctrl+S save · Esc cancel · Enter next field".into()
        }
    });
    [
        line(editing, Tone::Muted),
        line(format!(" {}", session.notice), Tone::Warning),
        line(format!(" {saving}"), Tone::Accent),
    ]
}
