//! A stacked bar and its legend: parts of one scale, each in a categorical
//! color, or with a distinct glyph when colors are disabled.

use std::cmp::Reverse;

use roomkit_tui_protocol::{MeterColor, MeterSegment};

use super::text::{columns, ellipsize};
use super::{Line, Tone, span};

/// Narrower meters show only their legend.
const MIN_BAR_WIDTH: usize = 10;
/// Glyphs of the eight series slots when colors are disabled.
const SERIES_GLYPHS: [&str; 8] = ["█", "▓", "▒", "░", "▚", "▞", "▖", "▗"];
/// Columns of a right-aligned share, `100.0%`, and its gap.
const SHARE_WIDTH: usize = 8;

/// How a part is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fill {
    /// A categorical slot; indexes wrap modulo eight.
    Series(u8),
    /// A secondary part.
    Muted,
    /// The measured remainder of a context breakdown.
    Remainder,
}

impl Fill {
    fn tone(self) -> Tone {
        match self {
            Self::Series(slot) => Tone::Series(slot),
            Self::Muted => Tone::Muted,
            Self::Remainder => Tone::Accent,
        }
    }

    /// A bar cell: a block in the part's color, or its glyph without colors.
    fn cell(self, monochrome: bool) -> &'static str {
        if monochrome { self.glyph() } else { "█" }
    }

    /// A legend marker, drawn like the part's cells.
    fn marker(self, monochrome: bool) -> &'static str {
        if monochrome { self.glyph() } else { "■" }
    }

    /// What tells the part apart when colors are disabled.
    fn glyph(self) -> &'static str {
        match self {
            Self::Series(slot) => SERIES_GLYPHS[usize::from(slot) % SERIES_GLYPHS.len()],
            Self::Muted => "·",
            Self::Remainder => "▄",
        }
    }
}

/// One legend row and its share of the bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Part<'a> {
    pub label: &'a str,
    /// Unknown until measured; shown as `—` and drawn as nothing.
    pub value: Option<u64>,
    pub fill: Fill,
    /// Shown with `≈`.
    pub estimated: bool,
}

impl<'a> Part<'a> {
    pub fn segment(segment: &'a MeterSegment) -> Self {
        Self {
            label: &segment.label,
            value: Some(segment.value),
            fill: match segment.color {
                MeterColor::Series(slot) => Fill::Series(slot),
                _ => Fill::Muted,
            },
            estimated: segment.estimated,
        }
    }
}

/// A host meter: the bar when there is room, then the legend.
pub(super) fn meter(
    segments: &[MeterSegment],
    capacity: Option<u64>,
    inner: usize,
    monochrome: bool,
) -> Vec<Line> {
    let parts: Vec<Part<'_>> = segments.iter().map(Part::segment).collect();
    let scale = scale(&parts, capacity);
    bar(&parts, scale, inner, monochrome)
        .into_iter()
        .chain(legend(&parts, Some(scale), inner, monochrome))
        .collect()
}

/// The full scale: the capacity, or the sum of the parts when larger or absent.
pub(super) fn scale(parts: &[Part<'_>], capacity: Option<u64>) -> u64 {
    capacity.unwrap_or(0).max(sum(parts))
}

/// The known values added up.
pub(super) fn sum(parts: &[Part<'_>]) -> u64 {
    parts
        .iter()
        .filter_map(|part| part.value)
        .fold(0, u64::saturating_add)
}

/// One row of `inner` cells, the unused scale drawn as a border-toned track.
pub(super) fn bar(parts: &[Part<'_>], scale: u64, inner: usize, monochrome: bool) -> Option<Line> {
    if inner < MIN_BAR_WIDTH {
        return None;
    }
    let values: Vec<u64> = parts.iter().map(|part| part.value.unwrap_or(0)).collect();
    let cells = cells(&values, scale, inner);
    let mut line: Line = parts
        .iter()
        .zip(&cells)
        .filter(|(_, count)| **count > 0)
        .map(|(part, count)| span(part.fill.cell(monochrome).repeat(*count), part.fill.tone()))
        .collect();
    let free = inner - cells.iter().sum::<usize>();
    if free > 0 {
        line.push(span("─".repeat(free), Tone::Border));
    }
    Some(line)
}

/// Cells of each value on a `width`-cell bar of `scale`, by largest remainder
/// so they add up exactly. Each non-zero value keeps a cell while there is room.
fn cells(values: &[u64], scale: u64, width: usize) -> Vec<usize> {
    // Exact arithmetic: even sixteen maximal values cannot overflow.
    let mut weights: Vec<u128> = values.iter().map(|value| u128::from(*value)).collect();
    let used: u128 = weights.iter().sum();
    // The unused scale competes for cells as a last pseudo-part.
    weights.push(u128::from(scale).saturating_sub(used));
    let total: u128 = weights.iter().sum();
    if total == 0 {
        return vec![0; values.len()];
    }
    let shares: Vec<u128> = weights.iter().map(|w| w * width as u128).collect();
    let mut cells: Vec<usize> = shares
        .iter()
        .map(|share| (share / total) as usize)
        .collect();
    let mut order: Vec<usize> = (0..shares.len()).collect();
    order.sort_by_key(|&i| Reverse(shares[i] % total));
    let missing = width - cells.iter().sum::<usize>();
    for &i in order.iter().take(missing) {
        cells[i] += 1;
    }
    keep_visible(&mut cells, &weights);
    cells.pop();
    cells
}

/// Give each non-zero part a cell, taken from the largest holder that can
/// spare one; the unused scale (last) need not stay visible.
fn keep_visible(cells: &mut [usize], weights: &[u128]) {
    let parts = weights.len() - 1;
    if weights[..parts].iter().filter(|w| **w > 0).count() > cells.iter().sum::<usize>() {
        return;
    }
    for starved in 0..parts {
        if weights[starved] == 0 || cells[starved] > 0 {
            continue;
        }
        let spare = |i: &usize| cells[*i] > usize::from(*i < parts);
        if let Some(donor) = (0..cells.len()).filter(spare).max_by_key(|i| cells[*i]) {
            cells[donor] -= 1;
            cells[starved] = 1;
        }
    }
}

/// One row per part: marker, label clipped with `…`, then its value and its
/// share of `scale`, right-aligned. Without a scale, shares are omitted.
pub(super) fn legend(
    parts: &[Part<'_>],
    scale: Option<u64>,
    inner: usize,
    monochrome: bool,
) -> Vec<Line> {
    let scale = scale.filter(|scale| *scale > 0);
    let values: Vec<String> = parts.iter().map(value).collect();
    let value_width = values.iter().map(|v| columns(v)).max().unwrap_or(0);
    let share_width = if scale.is_some() { SHARE_WIDTH } else { 0 };
    let label_width = inner.saturating_sub(3 + value_width + share_width);
    parts
        .iter()
        .zip(values)
        .map(|(part, value)| {
            let label = ellipsize(part.label, label_width);
            let gap = label_width - columns(&label) + 1 + value_width - columns(&value);
            let mut row = vec![
                span(part.fill.marker(monochrome), part.fill.tone()),
                span(format!(" {label}{}", " ".repeat(gap)), Tone::Text),
                span(value, Tone::Text),
            ];
            if let (Some(scale), Some(amount)) = (scale, part.value) {
                let share = format!("{:.1}%", amount as f64 * 100.0 / scale as f64);
                row.push(span(format!("{share:>SHARE_WIDTH$}"), Tone::Muted));
            }
            row
        })
        .collect()
}

fn value(part: &Part<'_>) -> String {
    match part.value {
        None => "—".into(),
        Some(amount) if part.estimated => format!("≈{}", compact(amount)),
        Some(amount) => compact(amount),
    }
}

/// `921`, `9.9k`, `300.4k`, `1.2M`: one decimal from a thousand, up to the
/// `E` that `u64::MAX` (`18.4E`) needs.
pub(super) fn compact(value: u64) -> String {
    if value < 1_000 {
        return value.to_string();
    }
    let mut scaled = value as f64 / 1_000.0;
    for unit in ["k", "M", "G", "T", "P"] {
        // Promote before rounding could print `1000.0k`.
        if scaled < 999.95 {
            return format!("{scaled:.1}{unit}");
        }
        scaled /= 1_000.0;
    }
    format!("{scaled:.1}E")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_compact_with_one_decimal() {
        let cases = [
            (0, "0"),
            (921, "921"),
            (999, "999"),
            (1_000, "1.0k"),
            (9_900, "9.9k"),
            (300_400, "300.4k"),
            (999_949, "999.9k"),
            (999_950, "1.0M"),
            (1_200_000, "1.2M"),
            (4_000_000_000, "4.0G"),
            (2_500_000_000_000, "2.5T"),
            (u64::MAX, "18.4E"),
        ];
        for (value, expected) in cases {
            assert_eq!(compact(value), expected, "{value}");
        }
    }

    #[test]
    fn cells_fill_the_width_exactly_and_keep_small_parts_visible() {
        // 131k context: one huge part, two tiny ones and some free space.
        let split = cells(&[122_000, 2_000, 1_000], 131_072, 30);
        assert_eq!(split, [27, 1, 1]);
        for (values, scale, width) in [
            (vec![1, 1, 1], 3, 10),
            (vec![5, 0, 7, 3], 100, 24),
            (vec![u64::MAX, u64::MAX], 0, 33),
            (vec![10, 20, 30], 0, 17),
        ] {
            let cells = cells(&values, scale, width);
            assert!(cells.iter().sum::<usize>() <= width);
            if scale <= values.iter().copied().fold(0, u64::saturating_add) {
                assert_eq!(cells.iter().sum::<usize>(), width, "{values:?}");
            }
            for (value, count) in values.iter().zip(&cells) {
                assert_eq!(*value > 0, *count > 0, "{values:?} → {cells:?}");
            }
        }
        assert_eq!(cells(&[1, 1, 1], 0, 2).iter().sum::<usize>(), 2);
        assert_eq!(cells(&[0, 0], 0, 12), [0, 0]);
    }

    #[test]
    fn without_capacity_the_parts_share_their_sum() {
        let segments = [
            MeterSegment::new("Images", 750, MeterColor::Series(0)),
            MeterSegment::new("Notes", 250, MeterColor::Series(1)),
        ];
        let lines = meter(&segments, None, 24, false);
        let text = |line: &Line| line.iter().map(|s| s.text.as_str()).collect::<String>();
        assert_eq!(
            text(&lines[0]),
            format!("{}{}", "█".repeat(18), "█".repeat(6))
        );
        assert_eq!(text(&lines[1]), "■ Images     750   75.0%");
        assert_eq!(text(&lines[2]), "■ Notes      250   25.0%");
        let narrow = meter(&segments, Some(2_000), MIN_BAR_WIDTH - 1, true);
        assert_eq!(narrow.len(), 2, "A narrow meter keeps only its legend");
    }
}
