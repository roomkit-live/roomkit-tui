//! CommonMark/GFM to semantic terminal lines. Source remains in the protocol.

mod writer;

use writer::render;

use crate::{content, render::Line};

/// Rendered messages, one entry per message identity.
#[derive(Default)]
pub(crate) struct Cache(crate::render::cache::Cache<Entry>);

struct Entry {
    source: String,
    width: usize,
    markdown: bool,
    lines: Vec<Line>,
}

impl Cache {
    /// Lines of the message identified by `key`, re-rendered only when its
    /// text or the layout changed.
    pub(crate) fn render(
        &mut self,
        key: &str,
        source: &str,
        width: usize,
        markdown: bool,
    ) -> Vec<Line> {
        if let Some(entry) = self.0.get(key)
            && entry.width == width
            && entry.markdown == markdown
            && entry.source == source
        {
            return entry.lines.clone();
        }
        let content = if markdown {
            render(source, width)
        } else {
            content::plain(source.trim_matches('\n'))
        };
        let lines = content::wrap(&content, width, true);
        let bytes = source.len() + crate::render::cache::line_bytes(&lines);
        let entry = Entry {
            source: source.into(),
            width,
            markdown,
            lines: lines.clone(),
        };
        self.0.insert(key.into(), entry, bytes);
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::Tone;
    use unicode_width::UnicodeWidthStr;

    fn text(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|line| line.iter().map(|span| span.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn paragraphs_lists_and_outer_spacing_are_compact_without_losing_code_whitespace() {
        let lines = render(
            "\n\n\n## Report\n\nHello **world**.\n\n\n- First\n- Second\n\n```rust\nfn main() {\n\n\n    // comment\n}\n```\n\n\n",
            80,
        );
        let plain = text(&lines);
        assert_eq!(plain.first().map(String::as_str), Some("Report"));
        assert_eq!(plain.last().map(String::as_str), Some("╰─"));
        assert!(plain.windows(2).any(|rows| rows == ["• First", "• Second"]));
        assert!(plain.windows(2).any(|rows| rows == ["│ ", "│ "]));
        assert!(plain.iter().any(|line| line == "│     // comment"));
        assert!(!plain.windows(2).any(|rows| rows == ["", ""]));
        assert!(
            lines
                .iter()
                .flatten()
                .any(|span| span.text == "world" && span.emphasis.bold)
        );
        assert!(
            lines
                .iter()
                .flatten()
                .any(|span| span.tone == Tone::Keyword)
        );
        assert!(
            lines
                .iter()
                .flatten()
                .any(|span| span.tone == Tone::Comment)
        );
    }

    #[test]
    fn inline_attributes_compose_and_literal_code_stays_literal() {
        let lines = render(
            "***Strong italic*** and `**literal**` and ~~removed~~. [Guide](https://example.com/docs).",
            24,
        );
        assert!(
            lines
                .iter()
                .flatten()
                .any(|span| span.emphasis.bold && span.emphasis.italic)
        );
        assert!(
            lines
                .iter()
                .flatten()
                .any(|span| span.tone == Tone::Code && span.text.contains("**literal**"))
        );
        assert!(lines.iter().flatten().any(|span| span.emphasis.strike));
        assert!(
            lines
                .iter()
                .flatten()
                .any(|span| span.tone == Tone::Link && span.emphasis.underline)
        );
        assert!(text(&lines).join("").contains("https://example.com/docs"));
    }

    #[test]
    fn tables_keep_all_cells_on_narrow_terminals_and_controls_stay_inert() {
        let source =
            "| Name | Value |\n| --- | --- |\n| café界 | **ready** |\n\n\u{1b}]52;c;bad\u{7}";
        for width in [12, 40, 80] {
            let lines = render(source, width);
            let plain = text(&lines).join("\n");
            assert!(plain.contains("café界") && plain.contains("ready"));
            assert!(!plain.contains('\u{1b}') && !plain.contains('\u{7}'));
            assert!(text(&lines).iter().all(|line| line.width() <= width));
        }
    }

    #[test]
    fn streamed_incomplete_markdown_and_resizing_recompute_without_changing_source() {
        let source = "Résumé **important**\n\n```json\n{\"name\": \"café界\", \"count\": 42}\n```";
        let mut cache = Cache::default();
        for (end, _) in source.char_indices() {
            cache.render("message", &source[..end], 40, true);
        }
        for width in [40, 20, 40] {
            assert_eq!(
                cache.render("message", source, width, true),
                content::wrap(&render(source, width), width, true)
            );
            assert_eq!(
                cache.render("message", source, width, false),
                content::wrap(&content::plain(source), width, true)
            );
        }
        assert!(
            cache
                .render("message", source, 40, true)
                .iter()
                .flatten()
                .any(|span| span.tone == Tone::Number)
        );
        assert_eq!(cache.0.len(), 1, "Streaming keeps one entry per message");
    }
}
