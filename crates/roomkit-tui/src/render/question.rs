//! The questions an agent asks: the question shown, its options, the
//! person's own words and the keys that answer. The card is shown whole or
//! not at all: a person never answers part of a question.

use unicode_width::UnicodeWidthStr;

use super::card::{card_screen, enlarge, filled_row, surface_rows};
use super::footer::quit_hint;
use super::text::{pad, wrap_words};
use super::{Frame, Line, Tone, View, frame, line, span};
use crate::QuestionCard;

pub(super) fn screen(view: &View<'_>, cols: usize, rows: usize) -> Frame {
    let session = view.session;
    let Some(card) = session.question() else {
        return frame(Vec::new(), cols, None);
    };
    let armed = session.question_armed(view.elapsed);
    let (mut lines, own_row) = card_lines(view, card, cols);
    let hint = quit_hint(session, view.elapsed).unwrap_or_else(|| hint(card, armed));
    for text in wrap_words(&hint, cols.saturating_sub(4)) {
        lines.push(filled(&text, cols, Tone::Muted));
    }
    lines.push(filled("", cols, Tone::Input));
    let caret = (card.typing() && !card.sent).then(|| {
        let column = own_prefix(card).width() + card.own_words().width();
        (column.min(cols.saturating_sub(1)), own_row)
    });
    let Some(placed) = card_screen(view, cols, rows, lines, caret) else {
        return enlarge("Enlarge terminal to read these questions", cols);
    };
    let mut result = placed.frame;
    result.question = Some(card.id().clone());
    result
}

/// The keys that answer, once the card could be read.
fn hint(card: &QuestionCard, armed: bool) -> String {
    if card.sent {
        return "Sending your answer…".to_owned();
    }
    if !armed {
        return "Read the question before answering".to_owned();
    }
    if card.typing() {
        return "Type your answer · Enter confirm · Esc back to the options".to_owned();
    }
    let keys = card.item().options.len() + 1;
    let space = if card.item().multi_select {
        "Space toggle"
    } else {
        "Space select"
    };
    let next = if card.current() + 1 < card.question().items.len() {
        " · Tab next"
    } else {
        ""
    };
    format!("↑↓ choose · 1-{keys} pick · {space} · Enter confirm{next} · Esc don't answer")
}

/// The card above its hint: who asks, the question, its options and the
/// own-words row, whose index it returns.
fn card_lines(view: &View<'_>, card: &QuestionCard, cols: usize) -> (Vec<Line>, usize) {
    let mut lines = heading(view, card, cols);
    lines.extend(options(card, cols));
    let own_row = lines.len();
    let own = card.own_words();
    let shown = if own.is_empty() && !card.typing() {
        "Other…".to_owned()
    } else {
        own.to_owned()
    };
    let text = format!("{}{shown}", own_prefix(card));
    let on_own = card.cursor() == card.item().options.len();
    lines.push(selectable(&text, cols, on_own && !card.typing()));
    lines.push(filled("", cols, Tone::Input));
    (lines, own_row)
}

/// Who asks, which question of how many, and the question itself.
fn heading(view: &View<'_>, card: &QuestionCard, cols: usize) -> Vec<Line> {
    let item = card.item();
    let count = card.question().items.len();
    let asker = view.session.actor_name(&card.question().actor);
    let position = if count > 1 {
        format!(" · Question {}/{count}", card.current() + 1)
    } else {
        String::new()
    };
    let header = if item.header.trim().is_empty() {
        String::new()
    } else {
        format!(" · {}", item.header.trim())
    };
    let mut lines = vec![filled("", cols, Tone::Input)];
    let mut heading = filled(
        &format!("{asker} asks{position}{header}"),
        cols,
        Tone::Input,
    );
    heading[0].emphasis.bold = true;
    lines.push(heading);
    for row in surface_rows(&item.text, cols.saturating_sub(4), Tone::Input) {
        let mut row = filled_row(row, 2, cols, Tone::Input);
        row.iter_mut().for_each(|part| part.emphasis.bold = true);
        lines.push(row);
    }
    lines.push(filled("", cols, Tone::Input));
    lines
}

/// The options, each with its pointer, its mark and its number, its label
/// wrapped whole beside them, since the label is what is sent, and its
/// description below.
fn options(card: &QuestionCard, cols: usize) -> Vec<Line> {
    let mut lines = Vec::new();
    for (index, option) in card.item().options.iter().enumerate() {
        let mark = choice_mark(card, index);
        let prefix = format!("{} {mark}{}. ", pointer(card, index), index + 1);
        let selected = card.cursor() == index && !card.typing();
        let width = cols.saturating_sub(prefix.width() + 2);
        let label = surface_rows(&option.label, width, choice_tone(selected));
        lines.extend(hanging(&prefix, label, cols, selected));
        let description = surface_rows(&option.description, cols.saturating_sub(10), Tone::Muted);
        lines.extend(
            description
                .into_iter()
                .map(|row| filled_row(row, 9, cols, Tone::Muted)),
        );
    }
    lines
}

/// `rows` beside `prefix`, the rows after the first indented as far, on the
/// surface of a choice, selected or not.
fn hanging(prefix: &str, rows: Vec<Line>, cols: usize, selected: bool) -> Vec<Line> {
    let tone = choice_tone(selected);
    let indent = " ".repeat(prefix.width());
    rows.into_iter()
        .enumerate()
        .map(|(index, row)| {
            let lead = if index == 0 { prefix } else { &indent };
            let mut row = filled_row([vec![span(lead, tone)], row].concat(), 0, cols, tone);
            row.iter_mut()
                .for_each(|part| part.emphasis.bold = selected);
            row
        })
        .collect()
}

fn choice_tone(selected: bool) -> Tone {
    if selected {
        Tone::Selected
    } else {
        Tone::Input
    }
}

fn pointer(card: &QuestionCard, index: usize) -> &'static str {
    if card.cursor() == index { "›" } else { " " }
}

/// What precedes an option's number: a box on a question taking several.
fn choice_mark(card: &QuestionCard, index: usize) -> &'static str {
    match (card.item().multi_select, card.is_picked(index)) {
        (true, true) => "[x] ",
        (true, false) => "[ ] ",
        (false, true) => "● ",
        (false, false) => "",
    }
}

/// The own-words row's marker and number, aligned with the options' boxes.
fn own_prefix(card: &QuestionCard) -> String {
    let row = card.item().options.len();
    let align = if card.item().multi_select { "    " } else { "" };
    format!("{} {align}{}. ", pointer(card, row), row + 1)
}

fn filled(text: &str, cols: usize, tone: Tone) -> Line {
    line(pad(&format!("  {text}"), cols), tone)
}

fn selectable(text: &str, cols: usize, selected: bool) -> Line {
    let tone = choice_tone(selected);
    let mut row = line(pad(text, cols), tone);
    row[0].emphasis.bold = selected;
    row
}
