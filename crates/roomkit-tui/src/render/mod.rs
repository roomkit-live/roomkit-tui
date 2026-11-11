//! Pure, styled terminal layout and optional tool presentations.

mod activity;
mod approval;
pub(crate) mod cache;
mod chrome;
mod composer;
mod context;
mod footer;
mod form;
mod frame;
mod hyperlink;
mod menus;
mod message;
mod meter;
mod panel;
mod presenters;
mod question;
mod sidebar;
mod text;
mod thinking;
mod timeline;
mod tool_cache;
mod welcome;

pub(crate) use frame::TimelineRows;
pub use frame::{ActionTarget, BackToBottomTarget, Frame, PermissionTarget, ToolTarget};
pub use hyperlink::Hyperlink;
pub use presenters::{Renderers, ToolRenderer};
pub use sidebar::SidebarArea;
pub use text::{sanitize, wrap};

use std::time::Duration;

use roomkit_tui_protocol::ToolCall;
use unicode_width::UnicodeWidthStr;

use crate::{ColorMode, Session, UiOptions};
use text::fit;

/// Semantic emphasis resolved through the application's palette at draw time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Tone {
    /// Ordinary content.
    #[default]
    Text,
    /// Branding and agent headings.
    Primary,
    /// A piece of a logo, in the color its application gave it.
    Mark(crate::Color),
    /// Activity and secondary accents.
    Accent,
    /// Secondary information.
    Muted,
    /// Unfocused borders.
    Border,
    /// Successful operation.
    Success,
    /// Pending or refused operation.
    Warning,
    /// Failed operation.
    Error,
    /// Highlighted menu option.
    Selected,
    /// Composer text and background.
    Input,
    /// Small prompt marker on the input surface.
    InputPrompt,
    /// Empty composer hint on the input surface.
    Placeholder,
    /// Markdown headings.
    Heading,
    /// Literal code and file paths.
    Code,
    /// Visible link labels and destinations.
    Link,
    /// Programming-language keywords.
    Keyword,
    /// String literals.
    String,
    /// Numeric literals.
    Number,
    /// Code comments.
    Comment,
    /// A categorical meter part: the theme's series color, wrapping modulo eight.
    Series(u8),
}

/// Independent text attributes, preserved even when colors are disabled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Emphasis {
    /// Strong emphasis.
    pub bold: bool,
    /// Italic emphasis.
    pub italic: bool,
    /// Underlined text.
    pub underline: bool,
    /// Deleted text.
    pub strike: bool,
}

/// A sanitized piece of a line, with semantic styling and no terminal controls.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Span {
    /// Text occupying terminal display columns.
    pub text: String,
    /// Application-defined color role.
    pub tone: Tone,
    /// Text attributes independent of the semantic color.
    pub emphasis: Emphasis,
    /// Optional native terminal hyperlink, independent of the visible label.
    pub hyperlink: Option<Hyperlink>,
}

/// A line of semantic spans, without terminal escape sequences.
pub type Line = Vec<Span>;

impl Span {
    /// Construct an unformatted semantic span; terminal layout sanitizes text.
    pub fn new(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
            emphasis: Emphasis::default(),
            hyperlink: None,
        }
    }

    /// Use strong emphasis.
    #[must_use]
    pub fn bold(mut self) -> Self {
        self.emphasis.bold = true;
        self
    }

    /// Use italic emphasis.
    #[must_use]
    pub fn italic(mut self) -> Self {
        self.emphasis.italic = true;
        self
    }
}

fn span(text: impl Into<String>, tone: Tone) -> Span {
    Span::new(text, tone)
}

fn line(text: impl Into<String>, tone: Tone) -> Line {
    vec![span(text, tone)]
}

/// Lay out a deterministic frame with default appearance and no elapsed time.
pub fn screen(session: &Session, width: u16, height: u16, renderers: &Renderers) -> Frame {
    screen_with_options(
        session,
        width,
        height,
        renderers,
        &UiOptions::default(),
        Duration::ZERO,
    )
}

/// Lay out a frame without I/O. The caller supplies time for optional animation.
/// Idle frames do not depend on time; animations never delay model output.
pub fn screen_with_options(
    session: &Session,
    width: u16,
    height: u16,
    renderers: &Renderers,
    options: &UiOptions,
    elapsed: Duration,
) -> Frame {
    let margin = options.bottom_margin.min(3).min(height.saturating_sub(18));
    let view = View {
        session,
        renderers,
        options,
        elapsed,
    };
    let viewport_height = height.saturating_sub(margin);
    let mut result = welcome::screen(&view, width, viewport_height)
        .unwrap_or_else(|| conversation_screen(&view, width, viewport_height));
    if margin > 0 {
        result.lines.resize(usize::from(height), String::new());
        result.spans.resize_with(usize::from(height), Vec::new);
    }
    result
}

/// The plain lines of the conversation segments whose first item is within
/// `items`, oldest first, laid out at `cols` columns as the screen shows them.
pub(crate) fn timeline_text<'a>(
    session: &'a Session,
    renderers: &'a Renderers,
    options: &'a UiOptions,
    elapsed: Duration,
    cols: usize,
    items: std::ops::RangeInclusive<usize>,
) -> impl Iterator<Item = (usize, Vec<String>)> + 'a {
    let view = View {
        session,
        renderers,
        options,
        elapsed,
    };
    timeline::blocks(view, cols, items)
}

/// Everything a layout reads: state, presentations, settings and the clock.
#[derive(Clone, Copy)]
struct View<'a> {
    session: &'a Session,
    renderers: &'a Renderers,
    options: &'a UiOptions,
    elapsed: Duration,
}

fn conversation_screen(view: &View<'_>, width: u16, height: u16) -> Frame {
    let side_width = sidebar::width(view.session, width, height, view.options);
    let main = composer::screen(
        view,
        width - side_width.unwrap_or(0),
        height,
        composer::Layout::Conversation {
            sidebar: side_width.is_some(),
        },
    );
    match side_width {
        Some(side) => sidebar::compose(
            main,
            view.session,
            usize::from(width),
            usize::from(height),
            usize::from(side),
            view.options.color == ColorMode::Never,
        ),
        None => main,
    }
}

fn tool_detail_lines(details: &[Line], width: usize, limit: usize) -> (Vec<Line>, bool) {
    let mut lines = Vec::new();
    for detail in details.iter().take(limit + 1) {
        for line in crate::content::wrap(std::slice::from_ref(detail), width, false) {
            if lines.len() == limit {
                return (lines, true);
            }
            lines.push(line);
        }
    }
    (lines, false)
}

fn generic_tool(call: &ToolCall, expanded: bool) -> Vec<Line> {
    let mut lines = vec![line(format!("args: {}", call.arguments), Tone::Muted)];
    if !call.text.is_empty() {
        lines.extend(crate::content::json_or_plain(&call.text));
    }
    if let Some(data) = &call.data
        && (expanded || call.text.is_empty())
    {
        lines.extend(crate::content::json_or_plain(&data.to_string()));
    }
    lines
}

fn spinner(options: &UiOptions, elapsed: Duration) -> &'static str {
    if !options.animations {
        return "◌";
    }
    const FRAMES: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
    FRAMES[((elapsed.as_millis() / 100) % FRAMES.len() as u128) as usize]
}

/// Clip every span to `width` columns and derive the plain text of each line.
fn frame(lines: Vec<Line>, width: usize, cursor: Option<(u16, u16)>) -> Frame {
    let spans: Vec<Vec<Span>> = lines
        .into_iter()
        .map(|line| {
            let mut remaining = width;
            line.into_iter()
                .filter_map(|part| {
                    if remaining == 0 {
                        return None;
                    }
                    let text = fit(&part.text, remaining);
                    remaining = remaining.saturating_sub(text.width());
                    Some(Span { text, ..part })
                })
                .collect()
        })
        .collect();
    let lines = spans
        .iter()
        .map(|line| line.iter().map(|span| span.text.as_str()).collect())
        .collect();
    Frame {
        lines,
        spans,
        cursor,
        ..Frame::default()
    }
}
