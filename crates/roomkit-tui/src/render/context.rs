//! The sidebar's context section: the displayed measurement, its gauge or
//! its split into host-estimated parts, and the turn's consumption.

use roomkit_tui_protocol::{ContextUsage, ConversationCost, MeterSegment, TokenUsage};

use super::meter::{self, Fill, Part};
use super::text::wrap_words;
use super::{Line, Tone, line, span};
use crate::Session;

/// Latest occupancy with a single-tone gauge, then the turn's consumption.
pub(super) fn context(session: &Session, inner: usize) -> Vec<Line> {
    let mut lines = measurement(session);
    if let Some(percent) = session
        .displayed_context()
        .and_then(|(usage, _)| usage.percent())
    {
        lines.extend(gauge(percent, inner));
    }
    lines.extend(turn_usage(session));
    lines.extend(conversation_cost(session));
    lines
}

/// The measurement split into the host's parts and the remainder they leave,
/// with a stacked bar in place of the gauge. Without a measurement, only the
/// parts are listed and the remainder reads `—`.
pub(super) fn breakdown(
    session: &Session,
    parts: &[MeterSegment],
    remainder: &str,
    inner: usize,
    monochrome: bool,
) -> Vec<Line> {
    let usage = session.displayed_context().map(|(usage, _)| usage);
    let rows = breakdown_parts(parts, remainder, usage);
    let mut lines = measurement(session);
    match usage {
        None => lines.extend(meter::legend(&rows, None, inner, monochrome)),
        Some(usage) => lines.extend(measured(usage, &rows, inner, monochrome)),
    }
    lines.extend(turn_usage(session));
    lines.extend(conversation_cost(session));
    lines
}

/// The stacked bar, occupancy and legend of a measured breakdown, noting when
/// the host's estimates exceed the measurement.
fn measured(usage: &ContextUsage, rows: &[Part<'_>], inner: usize, monochrome: bool) -> Vec<Line> {
    let scale = meter::scale(rows, usage.capacity);
    let mut lines = Vec::new();
    // Like the gauge, no bar without a capacity: a full one would read as a full context.
    if let Some(percent) = usage.percent() {
        lines.extend(meter::bar(rows, scale, inner, monochrome));
        lines.push(used_line(percent));
    }
    lines.extend(meter::legend(rows, Some(scale), inner, monochrome));
    // The remainder is at least zero, so the rows outgrow the measurement
    // only when the host's estimates do.
    if meter::sum(rows) > usage.tokens {
        let note = wrap_words("Estimates exceed the measurement", inner);
        lines.extend(note.into_iter().map(|part| line(part, Tone::Muted)));
    }
    lines
}

/// The host's parts, then the remainder of the measurement, at least zero;
/// it is an estimate as soon as a part or the measurement is one.
fn breakdown_parts<'a>(
    parts: &'a [MeterSegment],
    remainder: &'a str,
    usage: Option<&ContextUsage>,
) -> Vec<Part<'a>> {
    let mut rows: Vec<Part<'a>> = parts.iter().map(Part::segment).collect();
    let fixed = meter::sum(&rows);
    rows.push(Part {
        label: remainder,
        value: usage.map(|usage| usage.tokens.saturating_sub(fixed)),
        fill: Fill::Remainder,
        estimated: usage.is_some_and(|usage| usage.estimated) || rows.iter().any(|p| p.estimated),
    });
    rows
}

/// Freshness and `tokens / capacity` of the displayed measurement, after a
/// refusal past the context window when the latest request had one: that
/// request has no measurement to wait for.
fn measurement(session: &Session) -> Vec<Line> {
    let mut lines = Vec::new();
    if session.context_exceeded() {
        lines.push(line("Refused: over capacity", Tone::Warning));
    }
    let Some((usage, previous)) = session.displayed_context() else {
        if lines.is_empty() {
            lines.push(line("Awaiting usage report", Tone::Muted));
        }
        return lines;
    };
    if previous {
        lines.push(line("Last report", Tone::Muted));
    }
    let estimate = if usage.estimated { "~" } else { "" };
    let capacity = usage
        .capacity
        .filter(|c| *c > 0)
        .map_or_else(|| "?".into(), |c| c.to_string());
    let tokens = format!("{estimate}{} / {capacity} tokens", usage.tokens);
    lines.push(line(tokens, Tone::Text));
    lines
}

fn gauge(percent: f64, inner: usize) -> Vec<Line> {
    let filled = ((percent / 100.0).clamp(0.0, 1.0) * inner as f64).round() as usize;
    vec![
        vec![
            span("━".repeat(filled), used_tone(percent)),
            span("─".repeat(inner - filled), Tone::Border),
        ],
        used_line(percent),
    ]
}

/// Occupancy warns from 90% of the capacity.
fn used_tone(percent: f64) -> Tone {
    if percent >= 90.0 {
        Tone::Warning
    } else {
        Tone::Accent
    }
}

fn used_line(percent: f64) -> Line {
    let left = (100.0 - percent).max(0.0);
    line(
        format!("{percent:.1}% used · {left:.1}% left"),
        used_tone(percent),
    )
}

/// Input and output of the latest turn, and whether its report is partial.
fn turn_usage(session: &Session) -> Vec<Line> {
    let Some(usage) = session.usage() else {
        return Vec::new();
    };
    let count = |n: Option<u64>| n.map_or_else(|| "?".into(), |n| n.to_string());
    let (input, output) = (count(usage.input_tokens), count(usage.output_tokens));
    let mut lines = vec![line(
        format!("Turn: {input} in / {output} out"),
        Tone::Muted,
    )];
    match (usage.cached_input_tokens, usage.cache_write_input_tokens) {
        (0, 0) => {}
        (read, 0) => lines.push(line(format!("Cache: {read} read"), Tone::Muted)),
        (0, written) => lines.push(line(format!("Cache: {written} written"), Tone::Muted)),
        (read, written) => lines.push(line(
            format!("Cache: {read} read, {written} written"),
            Tone::Muted,
        )),
    }
    if !usage.complete {
        lines.push(line("Partial usage report", Tone::Warning));
    }
    lines
}

/// The conversation's cost at its model's list price, when the host states
/// one, and whether a turn of it has none.
fn conversation_cost(session: &Session) -> Vec<Line> {
    let Some(cost) = session.cost() else {
        return Vec::new();
    };
    let mut lines = vec![line(format!("Conversation: {}", amount(cost)), Tone::Muted)];
    if !cost.complete {
        lines.push(line("Some turns have no price", Tone::Warning));
    }
    lines
}

/// ` (14785 cached)`, ` (14785 cached, 300 written)`: what the provider read
/// from its prompt cache and wrote to it, when it reported either.
pub(super) fn cache_note(usage: &TokenUsage) -> String {
    match (usage.cached_input_tokens, usage.cache_write_input_tokens) {
        (0, 0) => String::new(),
        (read, 0) => format!(" ({read} cached)"),
        (0, written) => format!(" ({written} written)"),
        (read, written) => format!(" ({read} cached, {written} written)"),
    }
}

/// `≈ $0.42`, a list price's estimate, never an invoice; below a cent, four
/// decimals, so a cheap turn does not read as free.
pub(super) fn amount(cost: &ConversationCost) -> String {
    let value = cost.micros as f64 / 1_000_000.0;
    let digits = if cost.micros == 0 || cost.micros >= 10_000 {
        2
    } else {
        4
    };
    let value = format!("{value:.digits$}");
    match cost.currency.as_str() {
        "USD" => format!("≈ ${value}"),
        currency => format!("≈ {value} {currency}"),
    }
}
