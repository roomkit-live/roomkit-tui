//! OSC 8 is handled by the user's terminal, including when the app runs over SSH.

use std::io::{self, Write};

use crate::render::Span;

pub(super) fn print(writer: &mut impl Write, span: &Span) -> io::Result<()> {
    if let Some(link) = &span.hyperlink {
        // Use the same BEL-terminated OSC 8 form as Codex's terminal output.
        write!(writer, "\x1b]8;;{}\x07", link.url())?;
        writer.write_all(span.text.as_bytes())?;
        writer.write_all(b"\x1b]8;;\x07")
    } else {
        writer.write_all(span.text.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "Invalid fixtures must fail the test")]
    use super::*;
    use crate::render::{Hyperlink, Tone};

    #[test]
    fn link_output_is_closed_before_following_text_and_keeps_the_exact_url() {
        let url = "https://example.com/authorize?state=example&challenge=unchanged";
        let mut linked = Span::new("Open sign-in", Tone::Link);
        linked.hyperlink = Hyperlink::new(url);
        let mut bytes = Vec::new();
        print(&mut bytes, &linked).unwrap();
        print(&mut bytes, &Span::new("Enter code", Tone::Text)).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            format!("\x1b]8;;{url}\x07Open sign-in\x1b]8;;\x07Enter code")
        );
    }
}
