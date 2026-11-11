//! Bounded, semantic text presentation without terminal I/O or domain schemas.

use std::sync::OnceLock;

use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::render::{Line, Span, Tone, sanitize};

/// Display literal text, retaining indentation and line breaks, without controls.
pub fn plain(text: &str) -> Vec<Line> {
    sanitize(&crate::session::bounded(text, 65_536))
        .split('\n')
        .map(|line| vec![Span::new(line, Tone::Text)])
        .collect()
}

/// Highlight source using bundled syntaxes and application-defined semantic
/// colors. Unknown languages remain literal code. No parser executes code or
/// reads files. Input is limited to 64 KiB; very long lines skip syntax parsing.
pub fn code(text: &str, language: &str) -> Vec<Line> {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    let text = sanitize(&crate::session::bounded(text, 65_536));
    let syntaxes = SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines);
    let syntax = syntaxes
        .find_syntax_by_token(language)
        .or_else(|| syntaxes.find_syntax_by_extension(language))
        .unwrap_or_else(|| syntaxes.find_syntax_plain_text());
    let mut parser = ParseState::new(syntax);
    let mut scopes = ScopeStack::new();
    let mut rows = Vec::new();
    for source in text.split_inclusive('\n') {
        let visible = source.trim_end_matches('\n');
        let mut row = Vec::new();
        if source.len() > 2048 {
            rows.push(vec![Span::new(visible, Tone::Code)]);
            parser = ParseState::new(syntax);
            scopes = ScopeStack::new();
            continue;
        }
        match parser.parse_line(source, syntaxes) {
            Ok(operations) => {
                let mut previous = 0;
                for (offset, operation) in operations {
                    let end = offset.min(visible.len());
                    if let Some(part) = visible.get(previous..end) {
                        push(&mut row, Span::new(part, syntax_tone(&scopes)));
                    }
                    let _ = scopes.apply(&operation);
                    previous = end;
                }
                if let Some(part) = visible.get(previous..) {
                    push(&mut row, Span::new(part, syntax_tone(&scopes)));
                }
            }
            Err(_) => row.push(Span::new(visible, Tone::Code)),
        }
        rows.push(row);
    }
    rows
}

fn syntax_tone(scopes: &ScopeStack) -> Tone {
    for scope in scopes.as_slice().iter().rev() {
        let name = scope.build_string();
        if name.starts_with("comment") {
            return Tone::Comment;
        }
        if name.starts_with("string") {
            return Tone::String;
        }
        if name.starts_with("constant.numeric") {
            return Tone::Number;
        }
        if name.starts_with("keyword")
            || name.starts_with("storage")
            || name.starts_with("constant.language")
        {
            return Tone::Keyword;
        }
    }
    Tone::Code
}

/// Pretty-print a JSON result for display only, or preserve literal output.
/// Parsing never changes protocol data, permissions or execution state.
pub fn json_or_plain(text: &str) -> Vec<Line> {
    let bounded = crate::session::bounded(text, 65_536);
    match serde_json::from_str::<serde_json::Value>(&bounded) {
        Ok(data) => match serde_json::to_string_pretty(&data) {
            Ok(pretty) => code(&pretty, "json"),
            Err(_) => plain(&bounded),
        },
        Err(_) => plain(&bounded),
    }
}

/// Wrap styled lines without splitting graphemes or dropping their attributes.
/// Word wrapping is for prose; hard wrapping preserves source indentation.
/// Each supplied line is bounded to 8 KiB and 1024 spans before layout.
pub fn wrap(lines: &[Line], width: usize, words: bool) -> Vec<Line> {
    if width == 0 {
        return Vec::new();
    }
    let mut output = Vec::new();
    for line in lines {
        let mut physical = Vec::new();
        let mut remaining = 8192;
        for span in line.iter().take(1024) {
            let clean = sanitize(&crate::session::bounded(&span.text, remaining));
            remaining = remaining.saturating_sub(clean.len());
            for grapheme in clean.graphemes(true) {
                if grapheme == "\n" {
                    wrap_physical(&physical, width, words, &mut output);
                    physical.clear();
                } else {
                    physical.push(Span {
                        text: grapheme.into(),
                        ..span.clone()
                    });
                }
            }
            if remaining == 0 {
                break;
            }
        }
        wrap_physical(&physical, width, words, &mut output);
    }
    output
}

fn wrap_physical(glyphs: &[Span], width: usize, words: bool, output: &mut Vec<Line>) {
    if glyphs.is_empty() {
        output.push(Vec::new());
        return;
    }
    let mut start = 0;
    while start < glyphs.len() {
        let mut end = start;
        let mut used = 0;
        let mut space = None;
        while let Some(glyph) = glyphs.get(end) {
            let size = glyph.text.width();
            if used + size > width {
                break;
            }
            if glyph.text.trim().is_empty() && end > start {
                space = Some(end);
            }
            used += size;
            end += 1;
        }
        if end == start {
            output.push(vec![Span {
                text: "?".into(),
                ..glyphs[start].clone()
            }]);
            start += 1;
            continue;
        }
        let cut = if words && end < glyphs.len() {
            space.unwrap_or(end)
        } else {
            end
        };
        let mut row = Vec::new();
        for glyph in &glyphs[start..cut] {
            push(&mut row, glyph.clone());
        }
        output.push(row);
        start = cut;
        if words && end < glyphs.len() {
            while glyphs
                .get(start)
                .is_some_and(|glyph| glyph.text.trim().is_empty())
            {
                start += 1;
            }
        }
    }
}

pub(crate) fn push(line: &mut Line, span: Span) {
    if span.text.is_empty() {
        return;
    }
    if let Some(last) = line.last_mut().filter(|last| {
        last.tone == span.tone && last.emphasis == span.emphasis && last.hyperlink == span.hyperlink
    }) {
        last.text.push_str(&span.text);
    } else {
        line.push(span);
    }
}
