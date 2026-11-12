//! Long pasted text as one block of the composer.
//!
//! A paste of several lines, or a long one, filled the composer with text the
//! person could not read back there. It goes in as a label, such as
//! `[Pasted text #1 · 13 lines]`, which the composer moves over and deletes
//! as a whole; the message sent carries the pasted text in its place.

use std::ops::Range;

/// A paste of this many lines or more goes in as a block.
const BLOCK_LINES: usize = 3;
/// A paste longer than this many characters goes in as a block.
const BLOCK_CHARS: usize = 500;

/// The blocks of one draft, each a label in its text and the paste it
/// stands for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Pasted {
    blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Block {
    label: String,
    text: String,
}

impl Pasted {
    /// The label `text` goes in as, when it is long enough to be a block.
    pub fn block(&mut self, text: &str) -> Option<String> {
        let lines = text.lines().count();
        let chars = text.chars().count();
        if lines < BLOCK_LINES && chars <= BLOCK_CHARS {
            return None;
        }
        let size = if lines > 1 {
            format!("{lines} lines")
        } else {
            format!("{chars} characters")
        };
        let label = format!("[Pasted text #{} · {size}]", self.blocks.len() + 1);
        self.blocks.push(Block {
            label: label.clone(),
            text: text.to_owned(),
        });
        Some(label)
    }

    /// The label in `text` that ends at byte `at`.
    pub fn ending_at(&self, text: &str, at: usize) -> Option<Range<usize>> {
        self.labels_in(text).find(|range| range.end == at)
    }

    /// The label in `text` that starts at byte `at`.
    pub fn starting_at(&self, text: &str, at: usize) -> Option<Range<usize>> {
        self.labels_in(text).find(|range| range.start == at)
    }

    /// The label in `text` that holds byte `at` strictly inside it.
    pub fn around(&self, text: &str, at: usize) -> Option<Range<usize>> {
        self.labels_in(text)
            .find(|range| range.start < at && at < range.end)
    }

    /// `text` with each label replaced by the paste it stands for.
    pub fn expand(&self, text: &str) -> String {
        if self.blocks.is_empty() {
            return text.to_owned();
        }
        let mut expanded = String::with_capacity(text.len());
        let mut rest = text;
        while let Some((at, block)) = self.first_label(rest) {
            expanded.push_str(&rest[..at]);
            expanded.push_str(&block.text);
            rest = &rest[at + block.label.len()..];
        }
        expanded.push_str(rest);
        expanded
    }

    /// How many bytes [`expand`](Self::expand) gives for `text`.
    pub fn expanded_len(&self, text: &str) -> usize {
        let mut len = text.len();
        let mut rest = text;
        while let Some((at, block)) = self.first_label(rest) {
            len = len - block.label.len() + block.text.len();
            rest = &rest[at + block.label.len()..];
        }
        len
    }

    /// The first label in `text`, and its block.
    fn first_label(&self, text: &str) -> Option<(usize, &Block)> {
        self.blocks
            .iter()
            .filter_map(|block| text.find(&block.label).map(|at| (at, block)))
            .min_by_key(|(at, _)| *at)
    }

    /// Where each label stands in `text`.
    fn labels_in<'a>(&'a self, text: &'a str) -> impl Iterator<Item = Range<usize>> + 'a {
        self.blocks.iter().flat_map(move |block| {
            text.match_indices(block.label.as_str())
                .map(|(at, label)| at..at + label.len())
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        reason = "Fixtures fail the test on invalid state"
    )]
    use super::Pasted;

    #[test]
    fn a_long_or_many_lined_paste_is_a_block_and_a_short_one_is_not() {
        let mut pasted = Pasted::default();
        assert_eq!(pasted.block("one\ntwo"), None);
        assert_eq!(pasted.block(&"x".repeat(500)), None);
        assert_eq!(
            pasted.block("one\ntwo\nthree").as_deref(),
            Some("[Pasted text #1 · 3 lines]")
        );
        assert_eq!(
            pasted.block(&"x".repeat(501)).as_deref(),
            Some("[Pasted text #2 · 501 characters]")
        );
    }

    #[test]
    fn labels_expand_to_their_paste_and_are_found_where_they_stand() {
        let mut pasted = Pasted::default();
        let one = pasted.block("a\nb\nc").unwrap();
        // A paste that holds another label's words stays as it was pasted.
        let two = pasted.block(&format!("{one}\n2\n3")).unwrap();
        let text = format!("x {one} y {two}");
        assert_eq!(pasted.expand(&text), format!("x a\nb\nc y {one}\n2\n3"));
        assert_eq!(pasted.expanded_len(&text), pasted.expand(&text).len());
        let start = 2;
        let end = start + one.len();
        assert_eq!(pasted.ending_at(&text, end), Some(start..end));
        assert_eq!(pasted.starting_at(&text, start), Some(start..end));
        assert_eq!(pasted.around(&text, start + 1), Some(start..end));
        assert_eq!(pasted.around(&text, start), None);
        assert_eq!(Pasted::default().expand(&text), text);
    }
}
