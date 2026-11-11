//! Preserve explicit destinations and detect bare URLs while text is still unwrapped.

use super::Writer;
use crate::render::{Hyperlink, Line, Tone};

impl Writer {
    pub(super) fn active_link(&self) -> Option<Hyperlink> {
        self.links.last().and_then(|url| Hyperlink::new(url))
    }

    pub(super) fn start_link(&mut self, destination: String) {
        self.save_style();
        self.style.0 = Tone::Link;
        self.style.1.underline = true;
        self.links.push(destination);
    }

    pub(super) fn end_link(&mut self) {
        if let Some(url) = self.links.last() {
            self.text(format!(" ({url})"));
        }
        self.links.pop();
        self.restore_style();
    }

    pub(super) fn take_inline(&mut self) -> Line {
        crate::render::link_text(std::mem::take(&mut self.current))
    }
}
