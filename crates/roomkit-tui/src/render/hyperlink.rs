//! Validated HTTP links rendered by the terminal, without launching host programs.

use std::sync::Arc;

use super::{Line, Span, Tone};

mod detect;
pub(crate) use detect::{annotate, literal};

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
}
