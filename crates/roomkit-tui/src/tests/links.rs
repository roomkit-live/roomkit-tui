//! A web address in any text an agent or a host sends carries its whole
//! destination on every row it is shown on.

use crate::content::plain;
use crate::render::Line;

/// The text of the fragments of `rows` that link `url`, in order.
fn linked(rows: &[Line], url: &str) -> String {
    rows.iter()
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
fn an_address_in_unicode_quotes_or_before_unicode_punctuation_is_linked() {
    let url = "https://example.com/guide";
    for text in [
        format!("See “{url}” first."),
        format!("See {url}。"),
        format!("«{url}», then go on"),
        format!("「{url}」、"),
        format!("（{url}）"),
        format!("Read {url}…"),
    ] {
        assert_eq!(linked(&plain(&text), url), url, "{text}");
    }
}
