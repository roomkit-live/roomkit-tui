//! Recognize visible web addresses before wrapping can split their destination.

use super::{Hyperlink, Line, Span, Tone};

pub(crate) fn annotate(line: Line) -> Line {
    let mut result = Vec::new();
    for span in line {
        if span.hyperlink.is_some() || matches!(span.tone, Tone::Code | Tone::Link) {
            crate::content::push(&mut result, span);
        } else {
            annotate_span(span, &mut result);
        }
    }
    result
}

fn annotate_span(span: Span, result: &mut Line) {
    let mut consumed = 0;
    let mut search = 0;
    for token in span.text.split_whitespace() {
        let Some(offset) = span.text[search..].find(token) else {
            continue;
        };
        let start = search + offset;
        search = start + token.len();
        let trimmed = token.trim_start_matches(['(', '[', '{', '<', '\'', '"']);
        let first = start + token.len() - trimmed.len();
        let last = first + url_end(trimmed);
        let Some(link) = Hyperlink::new(&span.text[first..last]) else {
            continue;
        };
        crate::content::push(result, fragment(&span, &span.text[consumed..first]));
        let mut linked = fragment(&span, &span.text[first..last]);
        linked.hyperlink = Some(link);
        linked.tone = Tone::Link;
        linked.emphasis.underline = true;
        crate::content::push(result, linked);
        consumed = last;
    }
    crate::content::push(
        result,
        Span {
            text: span.text[consumed..].into(),
            ..span
        },
    );
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
