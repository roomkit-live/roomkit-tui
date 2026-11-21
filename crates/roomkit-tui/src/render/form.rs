//! A host form: its fields, the field being edited and the save controls.
//! Secrets are masked and choices show their labels, never their values.

use std::time::Duration;

use roomkit_tui_protocol::FieldKind;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::chrome::screen_notice;
use super::footer::quit_hint;
use super::text::{caret_window, fit, sanitize};
use super::{Frame, Line, Renderers, Tone, View, frame, line};
use crate::{FormState, Session};

/// Rows of the edited field's input area.
const INPUT_ROWS: usize = 3;

pub(super) fn screen(view: &View<'_>, cols: usize, rows: usize) -> Frame {
    let session = view.session;
    let Some(form) = &session.form else {
        return frame(Vec::new(), cols, None);
    };
    // The notice's rows beyond one come out of the window of instructions
    // or fields, which keeps a row: the rows past 12.
    let notice = screen_notice(&session.notice, cols, rows.saturating_sub(12));
    let mut lines = heading(form, cols, rows + 1 - notice.len(), view.renderers);
    let field = &form.spec.fields[form.selected];
    if form.spec.description.is_empty() || form.spec.fields.len() > 1 {
        lines.push(line(
            format!(
                " Field {} / {} · Tab next · Shift+Tab previous",
                form.selected + 1,
                form.editors.len()
            ),
            Tone::Muted,
        ));
    }
    lines.push(super::link_text(line(
        format!(" {}", field.help),
        Tone::Accent,
    )));
    lines.push(line(format!(" {}", field.label), Tone::Primary));
    let (input, (column, row)) = input(form, cols.saturating_sub(4));
    let cursor = Some(((column + 2) as u16, (lines.len() + row) as u16));
    lines.extend(input);
    lines.extend(controls(session, &field.kind, view.elapsed, notice));
    frame(lines.into_iter().take(rows).collect(), cols, cursor)
}

/// Keep guidance and the selected input visible together, even on short screens.
fn heading(form: &FormState, cols: usize, rows: usize, renderers: &Renderers) -> Vec<Line> {
    let mut lines = vec![line(format!(" {}", form.spec.title), Tone::Primary)];
    if form.spec.description.is_empty() {
        lines.push(line(" Changes apply when you save", Tone::Muted));
        lines.push(Vec::new());
        lines.extend(fields(form, rows.saturating_sub(12).max(1)));
        return lines;
    }
    let body = description(form, cols.saturating_sub(2), renderers);
    let extra = usize::from(form.spec.fields.len() > 1);
    let height = rows.saturating_sub(10 + extra).max(1);
    let start = form
        .description_scroll
        .min(body.len().saturating_sub(height));
    lines.extend(body.iter().skip(start).take(height).map(|row| {
        let mut row = row.clone();
        row.insert(0, super::span(" ", Tone::Text));
        row
    }));
    if body.len() > height {
        lines.push(line(
            format!(
                " PgUp/PgDn instructions · {}–{} / {}",
                start + 1,
                (start + height).min(body.len()),
                body.len()
            ),
            Tone::Muted,
        ));
    } else {
        lines.push(Vec::new());
    }
    lines
}

fn description(form: &FormState, width: usize, renderers: &Renderers) -> Vec<Line> {
    if form.spec.description_markdown {
        return renderers.markdown("form:description", &form.spec.description, width, true);
    }
    crate::content::wrap(&crate::content::plain(&form.spec.description), width, false)
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
    let (rows, caret) = caret_window(&text, &prefix, width, INPUT_ROWS);
    let lines = (0..INPUT_ROWS)
        .map(|index| {
            let text = fit(rows.get(index).map_or("", String::as_str), width);
            let padding = " ".repeat(width.saturating_sub(text.width()));
            line(format!("  {text}{padding}  "), Tone::Input)
        })
        .collect();
    (lines, caret)
}

/// The editing keys, the notice and the save keys.
fn controls(
    session: &Session,
    kind: &FieldKind,
    elapsed: Duration,
    notice: Vec<Line>,
) -> Vec<Line> {
    let editing = match kind {
        FieldKind::Choice { .. } => " ← / → choose a value",
        FieldKind::Multiline => " Alt+Enter newline · Ctrl+U clear field",
        _ => " Ctrl+U clear field",
    };
    let saving = quit_hint(session, elapsed).unwrap_or_else(|| {
        if session.form.as_ref().is_some_and(|form| form.submitted) {
            "Waiting for completion… · Esc cancel".into()
        } else if session.form_pending() {
            "Saving…".into()
        } else if session.form.as_ref().is_some_and(FormState::enter_saves) {
            "Enter save · Esc cancel".into()
        } else {
            "Ctrl+S save · Esc cancel · Enter next field".into()
        }
    });
    let mut rows = vec![line(editing, Tone::Muted)];
    rows.extend(notice);
    rows.push(line(format!(" {saving}"), Tone::Accent));
    rows
}
