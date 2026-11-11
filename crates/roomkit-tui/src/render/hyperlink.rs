//! Validated HTTP links rendered by the terminal, without launching host programs.

use std::sync::Arc;

use super::{Line, Span, Tone};

mod detect;
pub(crate) use detect::annotate;

/// A bounded HTTP(S) destination safe to include in an OSC 8 control sequence.
/// Opening it remains the terminal emulator's explicit user action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hyperlink(Arc<str>);

impl Hyperlink {
    /// Validate an absolute HTTP(S) URL with a host and ASCII URI characters.
    /// Other schemes, whitespace, control characters and URLs over 8 KiB are refused.
    pub fn new(url: &str) -> Option<Self> {
        let rest = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))?;
        let host = rest.split(['/', '?', '#']).next()?;
        (url.len() <= 8192 && !host.is_empty() && url.bytes().all(|byte| byte.is_ascii_graphic()))
            .then(|| Self(Arc::from(url)))
    }

    /// The complete destination, unchanged by clipping or wrapping its label.
    pub fn url(&self) -> &str {
        &self.0
    }
}

/// Link standalone URLs in host guidance. Keep the full address visible as a
/// fallback for terminals without OSC 8 and use that same target on every row.
pub(super) fn guidance(text: &str, width: usize) -> Vec<Line> {
    let lines: Vec<_> = text
        .split('\n')
        .map(|text| {
            let hyperlink = Hyperlink::new(text.trim());
            let tone = if hyperlink.is_some() {
                Tone::Link
            } else {
                Tone::Text
            };
            let mut span = Span {
                hyperlink,
                ..Span::new(text, tone)
            };
            span.emphasis.underline = span.hyperlink.is_some();
            vec![span]
        })
        .collect();
    crate::content::wrap(&lines, width, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_reject_controls_whitespace_and_non_web_schemes() {
        for url in [
            "javascript:alert(1)",
            "file:///tmp/example",
            "https://",
            "https:///path",
            "https://example.com/\x1b\\attack",
            "https://example.com/\x07",
            "https://example.com/\n",
            "https://example.com/\u{009c}",
            "https://example.com/with space",
        ] {
            assert!(Hyperlink::new(url).is_none());
        }
        assert!(Hyperlink::new(&format!("https://example.com/{}", "x".repeat(8192))).is_none());
        assert!(
            Hyperlink::new("https://example.com/signin?state=example&scope=one%20two").is_some()
        );
    }

    #[test]
    fn wrapped_links_keep_the_entire_destination_on_every_fragment() {
        let url = "https://example.com/authorize?state=example&challenge=unchanged";
        let rows = guidance(url, 16);
        assert!(rows.len() > 1);
        assert_eq!(
            rows.iter()
                .flatten()
                .map(|span| span.text.as_str())
                .collect::<String>(),
            url
        );
        assert!(rows.iter().flatten().all(|span| {
            span.hyperlink
                .as_ref()
                .is_some_and(|link| link.url() == url)
        }));
        assert!(
            guidance("https://example.com/\x1b\\attack", 80)
                .iter()
                .flatten()
                .all(|span| span.hyperlink.is_none())
        );
    }
}
