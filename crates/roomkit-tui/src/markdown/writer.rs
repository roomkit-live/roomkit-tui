//! A CommonMark event stream written as wrapped, styled lines.

mod blocks;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use unicode_width::UnicodeWidthStr;

use crate::content;
use crate::render::{Emphasis, Line, Span, Tone};

struct List {
    next: Option<u64>,
    marker: String,
    first: bool,
}

#[derive(Default)]
struct Table {
    rows: Vec<Vec<Line>>,
    row: Vec<Line>,
}

pub(super) struct Writer {
    width: usize,
    lines: Vec<Line>,
    current: Line,
    style: (Tone, Emphasis),
    styles: Vec<(Tone, Emphasis)>,
    lists: Vec<List>,
    quotes: usize,
    code: Option<(String, String)>,
    table: Option<Table>,
    links: Vec<String>,
}

impl Writer {
    fn text(&mut self, text: impl Into<String>) {
        content::push(
            &mut self.current,
            Span {
                text: text.into(),
                tone: self.style.0,
                emphasis: self.style.1,
                hyperlink: None,
            },
        );
    }

    fn prefix(&self, first: bool) -> Line {
        let mut prefix = vec![Span::new("│ ".repeat(self.quotes.min(8)), Tone::Border)];
        for (index, list) in self.lists.iter().take(12).enumerate() {
            let marker = if index + 1 == self.lists.len() && list.first && first {
                list.marker.clone()
            } else {
                " ".repeat(list.marker.width())
            };
            prefix.push(Span::new(marker, Tone::Accent));
        }
        if prefix.iter().map(|span| span.text.width()).sum::<usize>() > self.width / 2 {
            vec![Span::new("… ", Tone::Border)]
        } else {
            prefix
        }
    }

    fn flush(&mut self) {
        if self.current.is_empty() || self.table.is_some() {
            return;
        }
        let content = std::mem::take(&mut self.current);
        let prefix_width: usize = self
            .prefix(false)
            .iter()
            .map(|span| span.text.width())
            .sum();
        let rows = content::wrap(
            &[content],
            self.width.saturating_sub(prefix_width).max(1),
            true,
        );
        for (index, row) in rows.into_iter().enumerate() {
            let mut line = self.prefix(index == 0);
            line.extend(row);
            self.lines.push(line);
        }
        if let Some(list) = self.lists.last_mut() {
            list.first = false;
        }
    }

    fn blank(&mut self) {
        self.flush();
        if self.lists.is_empty() && self.lines.last().is_some_and(|line| !line.is_empty()) {
            self.lines.push(Vec::new());
        }
    }

    fn save_style(&mut self) {
        self.styles.push(self.style);
    }
    fn restore_style(&mut self) {
        self.style = self.styles.pop().unwrap_or_default();
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Text(text) if self.code.is_some() => {
                if let Some((_, code)) = &mut self.code {
                    code.push_str(&text);
                }
            }
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                self.text(text.as_ref())
            }
            Event::Code(text) => {
                content::push(
                    &mut self.current,
                    Span {
                        text: text.into_string(),
                        tone: Tone::Code,
                        emphasis: self.style.1,
                        hyperlink: None,
                    },
                );
            }
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.flush(),
            Event::TaskListMarker(done) => self.text(if done { "[x] " } else { "[ ] " }),
            Event::Rule => {
                self.blank();
                self.lines.push(vec![Span::new(
                    "─".repeat(self.width.min(48)),
                    Tone::Border,
                )]);
                self.blank();
            }
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.flush(),
            Tag::Heading { .. } => {
                self.blank();
                self.save_style();
                self.style.0 = Tone::Heading;
                self.style.1.bold = true;
            }
            Tag::Strong => {
                self.save_style();
                self.style.1.bold = true;
            }
            Tag::Emphasis => {
                self.save_style();
                self.style.1.italic = true;
            }
            Tag::Strikethrough => {
                self.save_style();
                self.style.1.strike = true;
            }
            Tag::BlockQuote(_) => {
                self.blank();
                self.quotes += 1;
            }
            Tag::List(next) => {
                self.blank();
                self.lists.push(List {
                    next,
                    marker: String::new(),
                    first: true,
                });
            }
            Tag::Item => {
                self.flush();
                if let Some(list) = self.lists.last_mut() {
                    list.marker = list
                        .next
                        .map_or_else(|| "• ".into(), |next| format!("{next}. "));
                    list.next = list.next.map(|next| next.saturating_add(1));
                    list.first = true;
                }
            }
            Tag::CodeBlock(kind) => {
                self.blank();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_owned()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((language, String::new()));
            }
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                self.save_style();
                self.style.0 = Tone::Link;
                self.style.1.underline = true;
                self.links.push(dest_url.into_string());
            }
            Tag::Table(_) => {
                self.blank();
                self.table = Some(Table::default());
            }
            Tag::TableHead => {
                self.save_style();
                self.style.1.bold = true;
                self.style.0 = Tone::Heading;
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush();
                self.blank();
            }
            TagEnd::Heading(_) => {
                self.flush();
                self.restore_style();
                self.blank();
            }
            TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough => self.restore_style(),
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.quotes = self.quotes.saturating_sub(1);
                self.blank();
            }
            TagEnd::Item => self.flush(),
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                self.blank();
            }
            TagEnd::CodeBlock => self.finish_code(),
            TagEnd::Link | TagEnd::Image => {
                if let Some(url) = self.links.pop() {
                    self.text(format!(" ({url})"));
                }
                self.restore_style();
            }
            TagEnd::TableCell => {
                if let Some(table) = &mut self.table {
                    table.row.push(std::mem::take(&mut self.current));
                }
            }
            TagEnd::TableRow | TagEnd::TableHead => {
                if let Some(table) = &mut self.table {
                    table.rows.push(std::mem::take(&mut table.row));
                }
                if tag == TagEnd::TableHead {
                    self.restore_style();
                }
            }
            TagEnd::Table => self.finish_table(),
            _ => {}
        }
    }
}

pub(super) fn render(source: &str, width: usize) -> Vec<Line> {
    let source = crate::render::sanitize(&crate::session::bounded(source, 65_536));
    let mut writer = Writer {
        width,
        lines: Vec::new(),
        current: Vec::new(),
        style: (Tone::Text, Emphasis::default()),
        styles: Vec::new(),
        lists: Vec::new(),
        quotes: 0,
        code: None,
        table: None,
        links: Vec::new(),
    };
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(&source, options) {
        writer.event(event);
        if writer.lines.len() > 4096 {
            writer.lines.truncate(4096);
            writer
                .lines
                .push(vec![Span::new("… message view limited", Tone::Muted)]);
            break;
        }
    }
    writer.flush();
    while writer.lines.last().is_some_and(Vec::is_empty) {
        writer.lines.pop();
    }
    writer.lines
}
