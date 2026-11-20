//! Text an agent or a host sends, laid out in rows with its web addresses
//! linked: each address carries its whole destination on every row it wraps
//! onto, so that a terminal opens all of it from any of them.

use super::{Line, Span, Tone, link_text, sanitize, span};

/// The most bytes and spans of a line [`crate::content::wrap`] lays out.
const PIECE_BYTES: usize = 8192;
const PIECE_SPANS: usize = 1024;

/// `text` in rows of `width` columns, in `tone`, wrapped at words or, for
/// output, hard; its line breaks start new rows. A line longer than
/// [`crate::content::wrap`] lays out whole is laid out in pieces, never cut:
/// a person reads all of what they approve.
pub(super) fn rows(text: &str, width: usize, tone: Tone, words: bool) -> Vec<Line> {
    sanitize(text)
        .split('\n')
        .flat_map(|physical| pieces(link_text(vec![span(physical, tone)])))
        .flat_map(|piece| crate::content::wrap(&[piece], width, words))
        .collect()
}

/// `line` in pieces of at most [`PIECE_BYTES`] and [`PIECE_SPANS`], each
/// fragment keeping its style and link.
fn pieces(line: Line) -> Vec<Line> {
    let mut pieces = Vec::new();
    let mut piece = Line::new();
    let mut used = 0;
    for part in line {
        let mut text = part.text.as_str();
        while !text.is_empty() {
            let cut = text.floor_char_boundary((PIECE_BYTES - used).min(text.len()));
            if cut == 0 || piece.len() == PIECE_SPANS {
                pieces.push(std::mem::take(&mut piece));
                used = 0;
                continue;
            }
            piece.push(Span {
                text: text[..cut].to_owned(),
                ..part.clone()
            });
            used += cut;
            text = &text[cut..];
        }
    }
    pieces.push(piece);
    pieces
}
