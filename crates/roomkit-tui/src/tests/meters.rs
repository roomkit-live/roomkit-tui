#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]

use roomkit_tui_protocol::*;
use unicode_width::UnicodeWidthStr;

use super::*;
use crate::render::{self, Frame, Renderers, Span, Tone};
use crate::{ColorMode, UiOptions};

const CAPACITY: u64 = 131_072;

fn sidebar(content: SidebarContent) -> Event {
    Event::SidebarChanged {
        conversation: "room-a".into(),
        sidebar: Some(SidebarSpec::new(
            "Session",
            vec![SidebarSection::new("Context", content)],
        )),
    }
}

fn breakdown(parts: Vec<MeterSegment>) -> SidebarContent {
    SidebarContent::ContextBreakdown {
        parts,
        remainder: "Messages".into(),
    }
}

fn measure(run: &str, tokens: u64) -> Event {
    Event::ContextUpdated {
        conversation: "room-a".into(),
        run: run.into(),
        provider: "local".into(),
        model: "model-a".into(),
        usage: ContextUsage::new(tokens).with_capacity(CAPACITY),
    }
}

fn draw(session: &Session, color: ColorMode) -> Frame {
    let options = UiOptions {
        color,
        ..UiOptions::default()
    };
    render::screen_with_options(
        session,
        140,
        30,
        &Renderers::default(),
        &options,
        std::time::Duration::ZERO,
    )
}

/// Spans of each sidebar row, after the separator the layout inserts.
fn side_spans(frame: &Frame) -> Vec<Vec<Span>> {
    frame
        .spans
        .iter()
        .map(|row| {
            let start = row
                .iter()
                .rposition(|s| s.text == "│ " && s.tone == Tone::Border);
            start.map_or_else(Vec::new, |i| row[i + 1..].to_vec())
        })
        .collect()
}

fn side(session: &Session) -> Vec<String> {
    side_spans(&draw(session, ColorMode::Auto))
        .iter()
        .map(|row| row.iter().map(|s| s.text.as_str()).collect())
        .collect()
}

fn row<'a>(rows: &'a [String], label: &str) -> &'a str {
    rows.iter()
        .find(|row| row.chars().skip(2).collect::<String>().starts_with(label))
        .unwrap_or_else(|| unreachable!("No legend row for {label}: {rows:#?}"))
}

fn fixed_parts() -> Vec<MeterSegment> {
    vec![
        MeterSegment::new("System prompt", 2_000, MeterColor::Series(0)).estimated(),
        MeterSegment::new("MCP github", 80_000, MeterColor::Series(1)).estimated(),
    ]
}

#[test]
fn a_context_breakdown_follows_each_measurement_without_a_new_sidebar() {
    let mut s = session();
    s.apply(sidebar(breakdown(fixed_parts())));
    let rows = side(&s);
    assert!(rows.iter().any(|r| r == "Awaiting usage report"));
    assert!(row(&rows, "System prompt").ends_with("≈2.0k"));
    assert!(
        row(&rows, "Messages").ends_with('—'),
        "Unmeasured remainder"
    );
    started(&mut s, "run-1");
    s.apply(measure("run-1", 100_000));
    let rows = side(&s);
    assert!(
        rows.iter().any(|r| r == "100000 / 131072 tokens"),
        "{rows:#?}"
    );
    assert!(rows.iter().any(|r| r == "76.3% used · 23.7% left"));
    assert!(row(&rows, "MCP github").ends_with("≈80.0k   61.0%"));
    assert!(row(&rows, "Messages").ends_with("≈18.0k   13.7%"));
    for legend in ["System prompt", "MCP github", "Messages"] {
        assert_eq!(row(&rows, legend).width(), 30, "{legend} fills the sidebar");
    }
    s.apply(measure("run-1", 120_000));
    assert!(row(&side(&s), "Messages").ends_with("≈38.0k   29.0%"));
    started(&mut s, "run-2");
    let rows = side(&s);
    assert!(rows.iter().any(|r| r == "Last report"));
    assert!(row(&rows, "Messages").ends_with("≈38.0k   29.0%"));
}

#[test]
fn estimates_beyond_the_measurement_leave_no_negative_remainder() {
    let mut s = session();
    let parts = vec![MeterSegment::new("Tool schemas", 150_000, MeterColor::Series(2)).estimated()];
    s.apply(sidebar(breakdown(parts)));
    started(&mut s, "run-1");
    s.apply(measure("run-1", 100_000));
    let rows = side(&s);
    assert!(row(&rows, "Messages").ends_with("≈0    0.0%"));
    assert!(row(&rows, "Tool schemas").ends_with("≈150.0k  100.0%"));
    let note = rows
        .iter()
        .position(|r| r == "Estimates exceed the")
        .unwrap();
    assert_eq!(rows[note + 1], "measurement", "Wrapped between words");
}

#[test]
fn the_stacked_bar_spans_the_sidebar_and_keeps_small_parts_visible() {
    let mut s = session();
    let parts = vec![
        MeterSegment::new("MCP", 122_000, MeterColor::Series(0)),
        MeterSegment::new("Skills", 2_000, MeterColor::Series(1)),
    ];
    s.apply(sidebar(breakdown(parts)));
    started(&mut s, "run-1");
    s.apply(measure("run-1", 125_000));
    let rows = side_spans(&draw(&s, ColorMode::Auto));
    let bar = rows
        .iter()
        .find(|row| row.iter().any(|s| s.text.starts_with('█')))
        .unwrap();
    let cells = |tone: Tone| -> usize {
        bar.iter()
            .filter(|s| s.tone == tone)
            .map(|s| s.text.width())
            .sum()
    };
    assert_eq!(bar.iter().map(|s| s.text.width()).sum::<usize>(), 30);
    assert_eq!(cells(Tone::Series(1)), 1, "2k of 131k still shows");
    assert_eq!(cells(Tone::Accent), 1, "the 1k remainder still shows");
    assert!(cells(Tone::Border) > 0, "free capacity closes the bar");
}

#[test]
fn without_colors_each_part_has_its_own_glyph() {
    let mut s = session();
    let parts = vec![
        MeterSegment::new("Prompt", 40_000, MeterColor::Series(0)),
        MeterSegment::new("Tools", 40_000, MeterColor::Series(9)),
        MeterSegment::new("Spare", 10_000, MeterColor::Muted),
    ];
    s.apply(sidebar(breakdown(parts)));
    started(&mut s, "run-1");
    s.apply(measure("run-1", 120_000));
    let rows: Vec<String> = side_spans(&draw(&s, ColorMode::Never))
        .iter()
        .map(|row| row.iter().map(|s| s.text.as_str()).collect())
        .collect();
    let bar = rows.iter().find(|r| r.starts_with('█')).unwrap();
    for glyph in ['█', '▓', '·', '▄', '─'] {
        assert!(bar.contains(glyph), "{glyph} missing from {bar}");
    }
    assert!(row(&rows, "Prompt").starts_with("█ "));
    assert!(
        row(&rows, "Tools").starts_with("▓ "),
        "Slot 9 wraps to slot 1"
    );
    assert!(row(&rows, "Spare").starts_with("· "));
    assert!(row(&rows, "Messages").starts_with("▄ "));
    assert!(rows.iter().all(|r| !r.contains('■')));
    let colored = side_spans(&draw(&s, ColorMode::Auto));
    assert!(colored.iter().flatten().any(|s| s.tone == Tone::Series(9)));
}

#[test]
fn a_host_meter_shows_any_proportion_of_its_capacity() {
    let mut s = session();
    let segments = vec![
        MeterSegment::new("Images", 750, MeterColor::Series(3)),
        MeterSegment::new("Notes", 250, MeterColor::Series(4)),
    ];
    s.apply(sidebar(SidebarContent::Meter {
        segments,
        capacity: Some(4_000),
    }));
    let rows = side(&s);
    assert!(row(&rows, "Images").ends_with("750   18.8%"));
    assert!(
        row(&rows, "Notes").ends_with("250    6.2%"),
        "6.25 rounds half to even"
    );
    let bar = rows.iter().find(|r| r.starts_with('█')).unwrap();
    assert_eq!(bar.width(), 30);
    assert!(bar.ends_with('─'), "{bar}");
}

#[test]
fn without_a_capacity_the_breakdown_draws_no_bar_like_the_gauge() {
    let mut s = session();
    s.apply(sidebar(breakdown(fixed_parts())));
    started(&mut s, "run-1");
    s.apply(Event::ContextUpdated {
        conversation: "room-a".into(),
        run: "run-1".into(),
        provider: "local".into(),
        model: "model-a".into(),
        usage: ContextUsage::new(100_000),
    });
    let rows = side(&s);
    assert!(rows.iter().any(|r| r == "100000 / ? tokens"), "{rows:#?}");
    assert!(
        !rows.iter().any(|r| r.contains('█')),
        "A full bar would read as a full context: {rows:#?}"
    );
    assert!(row(&rows, "Messages").ends_with("≈18.0k   18.0%"));
}

#[test]
fn a_legend_row_fits_however_its_label_is_shaped() {
    let mut s = session();
    let segments = vec![MeterSegment::new("لا لا", 2_500, MeterColor::Series(0))];
    s.apply(sidebar(SidebarContent::Meter {
        segments,
        capacity: Some(10_000),
    }));
    let rows = side(&s);
    let legend = rows.iter().find(|r| r.starts_with('■')).unwrap();
    assert!(legend.ends_with("2.5k   25.0%"), "{rows:#?}");
    // Narrower by `str::width` than by grapheme: clipping must count the same way.
    let segments = vec![MeterSegment::new(
        "لا".repeat(10),
        2_500,
        MeterColor::Series(0),
    )];
    s.apply(sidebar(SidebarContent::Meter {
        segments,
        capacity: Some(10_000),
    }));
    let rows = side(&s);
    let legend = rows.iter().find(|r| r.starts_with('■')).unwrap();
    assert!(
        legend.contains('…') && legend.ends_with("2.5k   25.0%"),
        "{rows:#?}"
    );
}

#[test]
fn an_out_of_bounds_meter_is_ignored_like_out_of_bounds_entries() {
    let segments = |count: usize, label: &str| -> Vec<MeterSegment> {
        (0..count)
            .map(|i| MeterSegment::new(label, 1, MeterColor::Series(i as u8)))
            .collect()
    };
    let meter = |segments| SidebarContent::Meter {
        segments,
        capacity: None,
    };
    let mut s = session();
    let current = |s: &Session| s.sidebar.as_ref().unwrap().spec.sections[0].content.clone();
    let mut longest_remainder = breakdown(segments(16, &"x".repeat(256)));
    if let SidebarContent::ContextBreakdown { remainder, .. } = &mut longest_remainder {
        *remainder = "x".repeat(256);
    }
    s.apply(sidebar(longest_remainder.clone()));
    assert_eq!(current(&s), longest_remainder);
    let accepted = meter(segments(16, &"x".repeat(256)));
    s.apply(sidebar(accepted.clone()));
    assert_eq!(current(&s), accepted);
    let mut long_remainder = breakdown(Vec::new());
    if let SidebarContent::ContextBreakdown { remainder, .. } = &mut long_remainder {
        *remainder = "x".repeat(257);
    }
    for rejected in [
        meter(segments(17, "x")),
        meter(segments(1, &"x".repeat(257))),
        breakdown(segments(17, "x")),
        breakdown(segments(1, &"x".repeat(257))),
        long_remainder,
    ] {
        s.apply(sidebar(rejected));
        assert_eq!(current(&s), accepted, "The previous sidebar stays");
    }
}

#[test]
fn a_breakdown_in_the_sidebar_replaces_the_footer_context() {
    let mut s = session();
    s.apply(sidebar(breakdown(fixed_parts())));
    started(&mut s, "run-1");
    s.apply(measure("run-1", 100_000));
    // The conversation side of every row: the footer, never the sidebar.
    let footer = |s: &Session| {
        let frame = draw(s, ColorMode::Auto);
        frame
            .spans
            .iter()
            .map(|row| {
                let end = row
                    .iter()
                    .rposition(|s| s.text == "│ " && s.tone == Tone::Border);
                row[..end.unwrap_or(row.len())]
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(!footer(&s).contains("context"), "{}", footer(&s));
    s.sidebar_visible = Some(false);
    assert!(
        footer(&s).contains("context 100000/131072"),
        "{}",
        footer(&s)
    );
}

fn exceeded(run: &str) -> Event {
    Event::ContextExceeded {
        conversation: "room-a".into(),
        run: run.into(),
    }
}

/// A request refused past the window: the sidebar says so above the last
/// measurement, which it shows as an earlier report, until the next run.
#[test]
fn a_refused_request_marks_the_last_measurement_until_the_next_run() {
    let mut s = session();
    s.apply(sidebar(breakdown(fixed_parts())));
    started(&mut s, "run-1");
    s.apply(measure("run-1", 12_544));
    started(&mut s, "run-2");
    s.apply(exceeded("run-2"));
    let rows = side(&s);
    let refused = rows
        .iter()
        .position(|r| r == "Refused: over capacity")
        .unwrap_or_else(|| unreachable!("No refusal: {rows:#?}"));
    assert_eq!(
        rows.get(refused + 1).map(String::as_str),
        Some("Last report")
    );
    assert!(rows.iter().any(|r| r == "12544 / 131072 tokens"));
    started(&mut s, "run-3");
    assert!(!side(&s).iter().any(|r| r == "Refused: over capacity"));
    s.apply(exceeded("run-2"));
    assert!(
        !side(&s).iter().any(|r| r == "Refused: over capacity"),
        "an earlier run's refusal is not the latest run's"
    );
}

/// The very first request refused: no measurement will come to wait for.
#[test]
fn a_refusal_before_any_measurement_replaces_the_wait() {
    let mut s = session();
    s.apply(sidebar(breakdown(fixed_parts())));
    started(&mut s, "run-1");
    s.apply(exceeded("run-1"));
    let rows = side(&s);
    assert!(rows.iter().any(|r| r == "Refused: over capacity"));
    assert!(
        !rows.iter().any(|r| r == "Awaiting usage report"),
        "{rows:#?}"
    );
}

/// A refusal after an earlier step of the same run measured: that step's
/// measure is the last report, not the refused request's.
#[test]
fn a_refusal_after_an_earlier_step_shows_that_step_as_the_last_report() {
    let mut s = session();
    s.apply(sidebar(breakdown(fixed_parts())));
    started(&mut s, "run-1");
    s.apply(measure("run-1", 5_000));
    s.apply(exceeded("run-1"));
    let rows = side(&s);
    let refused = rows
        .iter()
        .position(|r| r == "Refused: over capacity")
        .unwrap_or_else(|| unreachable!("No refusal: {rows:#?}"));
    let next: Vec<&str> = rows[refused + 1..refused + 3]
        .iter()
        .map(String::as_str)
        .collect();
    assert_eq!(next, ["Last report", "5000 / 131072 tokens"]);
}
