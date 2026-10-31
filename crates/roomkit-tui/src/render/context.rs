//! The sidebar's context section: the displayed measurement, its gauge and
//! the turn's consumption.

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
    lines
}

/// Freshness and `tokens / capacity` of the displayed measurement.
fn measurement(session: &Session) -> Vec<Line> {
    let Some((usage, previous)) = session.displayed_context() else {
        return vec![line("Awaiting usage report", Tone::Muted)];
    };
    let mut lines = Vec::new();
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
    if !usage.complete {
        lines.push(line("Partial usage report", Tone::Warning));
    }
    lines
}
