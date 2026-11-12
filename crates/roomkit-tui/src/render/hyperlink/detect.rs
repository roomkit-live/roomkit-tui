//! Recognize visible web addresses before wrapping can split their destination.

use std::ops::Range;

use super::{Hyperlink, Line, Span, Tone};

/// Link the web addresses of prose, which then looks like a link, and of code,
/// which keeps its text and style: only the terminal's native link is added.
pub(crate) fn annotate(line: Line) -> Line {
    let mut result = Vec::new();
    for span in line {
        if span.hyperlink.is_some() || span.tone == Tone::Link {
            crate::content::push(&mut result, span);
        } else if span.tone == Tone::Code {
            for part in literal(vec![span]) {
                crate::content::push(&mut result, part);
            }
        } else {
            annotate_span(span, &mut result);
        }
    }
    result
}

/// Link the web addresses of a highlighted code row without changing its text
/// or style. An address is found in the row's whole text, so syntax colors that
/// split it into several spans still give every fragment the entire URL.
pub(crate) fn literal(line: Line) -> Line {
    let text: String = line.iter().map(|span| span.text.as_str()).collect();
    let links = find(&text);
    if links.is_empty() {
        return line;
    }
    let mut result = Vec::new();
    let mut offset = 0;
    for span in line {
        let end = offset + span.text.len();
        let mut cut = offset;
        for (range, link) in links
            .iter()
            .filter(|(range, _)| range.start < end && range.end > offset)
        {
            let start = range.start.max(offset);
            let stop = range.end.min(end);
            crate::content::push(&mut result, fragment(&span, &text[cut..start]));
            let mut linked = fragment(&span, &text[start..stop]);
            linked.hyperlink = Some(link.clone());
            crate::content::push(&mut result, linked);
            cut = stop;
        }
        crate::content::push(&mut result, fragment(&span, &text[cut..end]));
        offset = end;
    }
    result
}

fn annotate_span(span: Span, result: &mut Line) {
    let mut consumed = 0;
    for (range, link) in find(&span.text) {
        crate::content::push(result, fragment(&span, &span.text[consumed..range.start]));
        let mut linked = fragment(&span, &span.text[range.clone()]);
        linked.hyperlink = Some(link);
        linked.tone = Tone::Link;
        linked.emphasis.underline = true;
        crate::content::push(result, linked);
        consumed = range.end;
    }
    crate::content::push(
        result,
        Span {
            text: span.text[consumed..].into(),
            ..span
        },
    );
}

/// The web addresses among the whitespace-separated words of `text`, without
/// the prose punctuation around them, in order and without overlap.
fn find(text: &str) -> Vec<(Range<usize>, Hyperlink)> {
    let mut links = Vec::new();
    let mut search = 0;
    for token in text.split_whitespace() {
        let Some(offset) = text[search..].find(token) else {
            continue;
        };
        let start = search + offset;
        search = start + token.len();
        let trimmed = token.trim_start_matches(['(', '[', '{', '<', '\'', '"']);
        let first = start + token.len() - trimmed.len();
        let last = first + url_end(trimmed);
        if let Some(link) = Hyperlink::new(&text[first..last]) {
            links.push((first..last, link));
        }
    }
    links
}

fn fragment(span: &Span, text: &str) -> Span {
    Span {
        text: text.into(),
        tone: span.tone,
        emphasis: span.emphasis,
        hyperlink: span.hyperlink.clone(),
    }
}

/// Retain balanced path parentheses, but omit surrounding prose punctuation.
fn url_end(text: &str) -> usize {
    let mut balance = [0_isize; 4];
    for ch in text.chars() {
        match ch {
            '(' => balance[0] += 1,
            ')' => balance[0] -= 1,
            '[' => balance[1] += 1,
            ']' => balance[1] -= 1,
            '{' => balance[2] += 1,
            '}' => balance[2] -= 1,
            '<' => balance[3] += 1,
            '>' => balance[3] -= 1,
            _ => {}
        }
    }
    let mut end = text.len();
    for ch in text.chars().rev() {
        let closing = match ch {
            ')' => Some(0),
            ']' => Some(1),
            '}' => Some(2),
            '>' => Some(3),
            _ => None,
        };
        let trim = if let Some(index) = closing {
            let unmatched = balance[index] < 0;
            balance[index] += 1;
            unmatched
        } else {
            matches!(ch, ',' | '.' | ';' | '!' | '\'' | '"')
                || (ch == '?' && unmatched_closer_before_question(&text[..end], &balance))
        };
        if !trim {
            break;
        }
        end -= ch.len_utf8();
    }
    end
}

fn unmatched_closer_before_question(text: &str, balance: &[isize; 4]) -> bool {
    match text.chars().rev().nth(1) {
        Some(')') => balance[0] < 0,
        Some(']') => balance[1] < 0,
        Some('}') => balance[2] < 0,
        Some('>') => balance[3] < 0,
        _ => false,
    }
}
