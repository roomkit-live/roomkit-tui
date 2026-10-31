//! Text measurement and clipping in terminal display columns. Every string
//! that reaches the screen passes through [`sanitize`].

use std::time::Duration;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::{Line, Tone, span};

/// Remove terminal controls and bidirectional overrides from untrusted text,
/// preserving line breaks. Bidirectional controls could make the text a person
/// approves differ from the text they read.
pub fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|c| (!c.is_control() || matches!(c, '\n' | '\t')) && !is_bidi_control(*c))
        .collect::<String>()
        .replace('\t', "    ")
}

fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

pub(super) fn fit(text: &str, width: usize) -> String {
    // Only visit the visible prefix; clipping a huge argument does not allocate
    // every wrapped line. Newlines terminate this single-line field.
    let mut result = String::new();
    let mut used = 0;
    for grapheme in text.graphemes(true) {
        if grapheme.contains('\n') {
            break;
        }
        let clean = sanitize(grapheme);
        let size = clean.width();
        if used + size > width {
            break;
        }
        result.push_str(&clean);
        used += size;
    }
    result
}

/// Wrap on grapheme boundaries, respecting terminal display columns.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let text = sanitize(text);
    let mut rows = Vec::new();
    for physical in text.split('\n') {
        let mut line = String::new();
        let mut used = 0;
        for grapheme in physical.graphemes(true) {
            let size = grapheme.width();
            if used + size > width && !line.is_empty() {
                rows.push(std::mem::take(&mut line));
                used = 0;
            }
            if size > width {
                line.push('?');
                used += 1;
            } else {
                line.push_str(grapheme);
                used += size;
            }
        }
        rows.push(line);
    }
    rows
}

// Prose wraps at word boundaries; the editor deliberately keeps hard wrapping
// so its caret position remains a direct projection of the underlying text.
pub(super) fn wrap_words(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let text = sanitize(text);
    let mut lines = Vec::new();
    for physical in text.split('\n') {
        let mut rest = physical;
        loop {
            let mut columns = 0;
            let mut cut = rest.len();
            let mut space = None;
            for (index, grapheme) in rest.grapheme_indices(true) {
                if columns + grapheme.width() > width {
                    cut = index;
                    break;
                }
                if grapheme.chars().all(char::is_whitespace) && index > 0 {
                    space = Some(index);
                }
                columns += grapheme.width();
            }
            if cut == rest.len() {
                lines.push(rest.into());
                break;
            }
            if cut == 0 {
                let size = rest.graphemes(true).next().map_or(0, str::len);
                lines.push("?".into());
                rest = &rest[size..];
            } else if let Some(space) = space.filter(|index| !rest[..*index].trim().is_empty()) {
                lines.push(rest[..space].trim_end().into());
                rest = rest[space..].trim_start();
            } else {
                lines.push(rest[..cut].into());
                rest = &rest[cut..];
            }
        }
    }
    lines
}

pub(super) fn format_duration(duration: Duration, finished: bool) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        format!(
            "{}h {:02}m {:02}s",
            seconds / 3600,
            (seconds / 60) % 60,
            seconds % 60
        )
    } else if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else if finished {
        format!("{seconds}.{}s", duration.subsec_millis() / 100)
    } else {
        format!("{seconds}s")
    }
}

pub(super) fn panel(text: &str, cols: usize, tone: Tone, border: Tone) -> Line {
    vec![
        span("│ ", border),
        span(pad(text, cols - 4), tone),
        span(" │", border),
    ]
}

pub(super) fn rule(label: &str, width: usize, left: char, right: char) -> String {
    let label = fit(label, width.saturating_sub(2));
    format!(
        "{left}{label}{}{right}",
        "─".repeat(width.saturating_sub(2 + label.width()))
    )
}

pub(super) fn pad(text: &str, width: usize) -> String {
    let text = fit(text, width);
    format!("{text}{}", " ".repeat(width.saturating_sub(text.width())))
}

pub(super) fn input_gutter(prompt: &str) -> String {
    let prompt = fit(prompt, 4);
    if prompt.is_empty() {
        String::new()
    } else {
        format!("{prompt} ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_and_bidirectional_overrides_are_removed() {
        let clean = sanitize("rm -rf /tmp/\u{202E}txt.exe\u{2066}\u{1b}[2J\tend\n");
        assert_eq!(clean, "rm -rf /tmp/txt.exe[2J    end\n");
        assert_eq!(fit("a\u{200F}b", 10), "ab");
    }
}
