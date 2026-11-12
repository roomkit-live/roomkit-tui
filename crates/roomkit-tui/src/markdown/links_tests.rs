//! Link destinations belong to the visible text, including after layout.

use super::writer::render;
use crate::render::{Line, Tone};

fn linked_text(lines: &[Line], url: &str) -> String {
    lines
        .iter()
        .flatten()
        .filter(|span| {
            span.hyperlink
                .as_ref()
                .is_some_and(|link| link.url() == url)
        })
        .map(|span| span.text.as_str())
        .collect()
}

#[test]
fn markdown_links_keep_their_destinations_through_styles_and_wrapping() {
    let url = "https://example.com/docs?topic=links&lang=en";
    for width in [16, 40, 100] {
        let rows = render(&format!("Before [**Open** `guide`]({url}) after."), width);
        let linked = linked_text(&rows, url);
        assert!(
            linked.contains("Open"),
            "The label must carry its destination"
        );
        assert!(linked.contains("guide"), "Code labels are still links");
        assert!(
            rows.iter()
                .flatten()
                .filter(|span| span.text.contains("Before") || span.text.contains("after"))
                .all(|span| span.hyperlink.is_none())
        );
    }
}

#[test]
fn bare_links_keep_the_entire_url_and_exclude_sentence_punctuation() {
    for url in [
        "https://example.com/docs",
        "https://example.com/~person/guide_(terminal)?tab=one&lang=en#links",
    ] {
        let rows = render(&format!("See ({url}). Then continue."), 17);
        assert_eq!(linked_text(&rows, url), url);
        assert!(
            rows.iter()
                .flatten()
                .filter(|span| span.hyperlink.is_some())
                .all(|span| span.emphasis.underline)
        );
    }
}

#[test]
fn question_mark_after_a_parenthesized_url_is_prose_but_a_query_is_preserved() {
    for (source, url) in [
        (
            "Open (https://example.com/docs)?",
            "https://example.com/docs",
        ),
        (
            "Open https://example.com/docs?",
            "https://example.com/docs?",
        ),
        (
            "Open https://example.com/guide_(terminal)?",
            "https://example.com/guide_(terminal)?",
        ),
    ] {
        assert_eq!(linked_text(&render(source, 80), url), url);
        assert_eq!(linked_text(&crate::content::plain(source), url), url);
    }
}

#[test]
fn links_in_tables_keep_their_destination() {
    let url = "https://example.com/guide";
    for width in [16, 48, 100] {
        let rows = render(
            &format!("| Resource | Status |\n| --- | --- |\n| [Guide]({url}) | Ready |"),
            width,
        );
        assert!(linked_text(&rows, url).contains("Guide"));
    }
}

/// A wrapped address in code must carry its destination on every row: the
/// terminal's own detection only sees the row clicked, so it opens a cut URL.
#[test]
fn urls_in_code_link_their_entire_destination_on_every_wrapped_row() {
    let url = "https://example.com/authorize?state=abcdefghijklmnopqrstuvwxyz&scope=one+two";
    for source in [
        format!("Run `{url}` now."),
        format!("```text\n{url}\n```"),
        // Shell highlighting splits the address at `&` and `=` into several spans.
        format!("```sh\ncurl {url} | jq .\n```"),
        format!("```json\n{{\"url\": \"{url}\", \"ok\": true}}\n```"),
    ] {
        for width in [24, 40, 120] {
            let rows = render(&source, width);
            assert_eq!(linked_text(&rows, url), url, "{source} at width {width}");
            let linked_rows = rows
                .iter()
                .filter(|row| row.iter().any(|span| span.hyperlink.is_some()))
                .count();
            if width < url.len() {
                assert!(linked_rows > 1, "{source} must wrap at width {width}");
            }
            for span in rows
                .iter()
                .flatten()
                .filter(|span| span.hyperlink.is_some())
            {
                assert_ne!(span.tone, Tone::Link, "Code keeps its own colors");
                assert!(!span.emphasis.underline, "Code keeps its own emphasis");
            }
            let visible: String = rows
                .iter()
                .flatten()
                .map(|span| span.text.as_str())
                .collect();
            assert!(
                !visible.contains(&format!("({url})")),
                "Code text is unchanged"
            );
        }
    }
}

#[test]
fn non_web_markdown_destinations_are_never_terminal_links() {
    for destination in [
        "file:///tmp/example",
        "javascript:alert",
        "relative.md",
        "mailto:example@example.com",
    ] {
        let rows = render(&format!("[https://example.com]({destination})"), 80);
        assert!(rows.iter().flatten().all(|span| span.hyperlink.is_none()));
    }
}
