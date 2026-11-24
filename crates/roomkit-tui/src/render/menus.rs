//! Selectors beside the composer: host choices and command completions.

use unicode_width::UnicodeWidthStr;

use super::text::{pad, panel, rule, sanitize};
use super::{Line, Session, Tone, line};

pub(super) fn choice_picker(picker: &crate::ChoicePicker, cols: usize, height: usize) -> Vec<Line> {
    let entries = picker.entries();
    let mut lines = vec![
        line(
            rule(
                &format!(" {} · {} ", picker.title, entries.len()),
                cols,
                '╭',
                '╮',
            ),
            Tone::Primary,
        ),
        panel(
            &format!("Filter: {}", picker.filter()),
            cols,
            Tone::Text,
            Tone::Border,
        ),
    ];
    let count = height.saturating_sub(3).checked_div(2).unwrap_or(0).max(1);
    let start = picker.selected.saturating_sub(count - 1);
    if entries.is_empty() {
        lines.push(panel(
            if picker.filter().is_empty() {
                "No choices available"
            } else {
                "No matching choices"
            },
            cols,
            Tone::Muted,
            Tone::Border,
        ));
    }
    for (index, entry) in entries.iter().enumerate().skip(start).take(count) {
        lines.push(panel(
            &format!(
                "{} {}",
                if index == picker.selected { "›" } else { " " },
                entry.label
            ),
            cols,
            if index == picker.selected {
                Tone::Selected
            } else {
                Tone::Text
            },
            Tone::Border,
        ));
        lines.push(panel(
            &format!("  {}", entry.description),
            cols,
            Tone::Muted,
            Tone::Border,
        ));
    }
    lines.push(line(
        rule(" ↑↓ choose · Enter confirm · Esc cancel ", cols, '╰', '╯'),
        Tone::Border,
    ));
    lines
}

pub(super) fn menu(session: &Session, cols: usize, height: usize) -> Vec<Line> {
    let menu = &session.menu;
    let label = if menu.arguments {
        "Choices"
    } else {
        "Commands"
    };
    let selected = if menu.entries.is_empty() {
        0
    } else {
        menu.selected + 1
    };
    let mut lines = vec![line(
        rule(
            &format!(" {label} · {selected}/{} ", menu.entries.len()),
            cols,
            '╭',
            '╮',
        ),
        Tone::Border,
    )];
    let count = height.saturating_sub(2).max(1);
    if menu.entries.is_empty() {
        lines.push(panel(
            menu.empty_message
                .as_deref()
                .unwrap_or("No matching choices"),
            cols,
            Tone::Muted,
            Tone::Border,
        ));
    } else {
        let start = menu.selected.saturating_sub(count - 1);
        let name_width = menu
            .entries
            .iter()
            .map(|e| sanitize(&e.label).width())
            .max()
            .unwrap_or(0)
            .min(cols / 3);
        for (index, entry) in menu.entries.iter().enumerate().skip(start).take(count) {
            let chosen = index == menu.selected;
            let marker = if chosen { "›" } else { " " };
            let text = entry_text(
                entry,
                marker,
                name_width,
                cols,
                session.choosing_action_argument(),
            );
            lines.push(panel(
                &text,
                cols,
                if chosen { Tone::Selected } else { Tone::Text },
                Tone::Border,
            ));
        }
    }
    lines.push(line(
        rule(
            if session.choosing_action_argument() {
                " ↑↓ select · Enter/Tab choose · Esc close "
            } else {
                " ↑↓ select · Enter/Tab insert · Esc close "
            },
            cols,
            '╰',
            '╯',
        ),
        Tone::Border,
    ));
    lines
}

fn entry_text(
    entry: &crate::MenuEntry,
    marker: &str,
    aligned_width: usize,
    cols: usize,
    action_choice: bool,
) -> String {
    if cols < 60 {
        return format!("{marker} {}", entry.label);
    }
    // A host choice may differ from another one only at the end of its label.
    let width = if action_choice {
        aligned_width.max(sanitize(&entry.label).width()).min(cols)
    } else {
        aligned_width
    };
    format!(
        "{marker} {}  {}",
        pad(&entry.label, width),
        entry.description
    )
}
