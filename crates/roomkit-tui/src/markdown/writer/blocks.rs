//! Blocks written once complete: fenced code and tables.

use unicode_width::UnicodeWidthStr;

use super::Writer;
use crate::content;
use crate::render::{Span, Tone};

impl Writer {
    pub(super) fn finish_code(&mut self) {
        let Some((language, source)) = self.code.take() else {
            return;
        };
        let mut header = self.prefix(false);
        header.push(Span::new(
            format!(
                "╭─ {}",
                if language.is_empty() {
                    "code"
                } else {
                    &language
                }
            ),
            Tone::Comment,
        ));
        let indent: usize = self
            .prefix(false)
            .iter()
            .map(|span| span.text.width())
            .sum();
        self.lines.push(header);
        for row in content::wrap(
            &content::code(&source, &language),
            self.width.saturating_sub(indent + 2).max(1),
            false,
        ) {
            let mut line = self.prefix(false);
            line.push(Span::new("│ ", Tone::Border));
            line.extend(row);
            self.lines.push(line);
        }
        let mut footer = self.prefix(false);
        footer.push(Span::new("╰─", Tone::Border));
        self.lines.push(footer);
        self.blank();
    }

    pub(super) fn finish_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return;
        }
        let width = self.width.saturating_sub((columns - 1) * 3) / columns;
        for (index, row) in table.rows.iter().enumerate() {
            if self.lines.len() > 4096 {
                break;
            }
            if width < 8 {
                for (column, cell) in row.iter().enumerate().take(columns) {
                    let mut value = Vec::new();
                    if index > 0 {
                        value.extend(
                            table
                                .rows
                                .first()
                                .and_then(|head| head.get(column))
                                .cloned()
                                .unwrap_or_default(),
                        );
                        value.push(Span::new(": ", Tone::Muted));
                    }
                    value.extend(cell.clone());
                    self.lines.extend(content::wrap(&[value], self.width, true));
                    if self.lines.len() > 4096 {
                        break;
                    }
                }
                self.blank();
            } else {
                let cells: Vec<_> = row
                    .iter()
                    .take(columns)
                    .map(|cell| content::wrap(std::slice::from_ref(cell), width, true))
                    .collect();
                let height = cells.iter().map(Vec::len).max().unwrap_or(1);
                for line_index in 0..height {
                    let mut line = Vec::new();
                    for column in 0..columns {
                        if column > 0 {
                            line.push(Span::new(" │ ", Tone::Border));
                        }
                        let content = cells
                            .get(column)
                            .and_then(|cell| cell.get(line_index))
                            .cloned()
                            .unwrap_or_default();
                        let used: usize = content.iter().map(|span| span.text.width()).sum();
                        line.extend(content);
                        line.push(Span::new(
                            " ".repeat(width.saturating_sub(used)),
                            Tone::Text,
                        ));
                    }
                    self.lines.push(line);
                }
                if index == 0 {
                    self.lines
                        .push(vec![Span::new("─".repeat(self.width), Tone::Border)]);
                }
            }
        }
        self.blank();
    }
}
