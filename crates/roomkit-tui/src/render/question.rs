//! The questions an agent asks: the question shown, its options, the
//! person's own words and the keys that answer. The card is shown whole or
//! not at all: a person never answers part of a question.

use unicode_width::UnicodeWidthStr;

use super::footer::quit_hint;
use super::text::{pad, wrap, wrap_words};
use super::timeline::timeline;
use super::{Frame, Line, Tone, View, frame, line};
use crate::QuestionCard;

pub(super) fn screen(view: &View<'_>, cols: usize, rows: usize) -> Frame {
    let session = view.session;
    let Some(card) = session.question() else {
        return frame(Vec::new(), cols, None);
    };
    let armed = session.question_armed(view.elapsed);
    let (mut lines, own_row) = card_lines(view, card, cols);
    let hint = if card.sent {
        "Sending your answer…".to_owned()
    } else if !armed {
        "Read the question before answering".to_owned()
    } else if card.typing() {
        "Type your answer · Enter confirm · Esc back to the options".to_owned()
    } else {
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
    };
    let hint = quit_hint(session, view.elapsed).unwrap_or(hint);
    for text in wrap_words(&hint, cols.saturating_sub(4)) {
        lines.push(filled(&text, cols, Tone::Muted));
    }
    lines.push(filled("", cols, Tone::Input));
    let notice = usize::from(!session.notice.is_empty());
    if lines.len() + notice > rows {
        let text = wrap("Enlarge terminal to read these questions", cols);
        return frame(
            text.into_iter().map(|t| line(t, Tone::Muted)).collect(),
            cols,
            None,
        );
    }
    let top = rows - lines.len() - notice;
    let cursor = (card.typing() && !card.sent).then(|| {
        let prefix = own_prefix(card).width();
        let words = card.own_words().width();
        let column = (prefix + words).min(cols.saturating_sub(1));
        (column as u16, (top + own_row) as u16)
    });
    let history = timeline(view, cols, top.saturating_sub(1), 0).lines;
    let mut screen: Vec<Line> = history
        .into_iter()
        .rev()
        .take(top.saturating_sub(1))
        .collect();
    screen.reverse();
    screen.resize_with(top, Vec::new);
    screen.extend(lines);
    if notice > 0 {
        screen.push(line(format!(" {}", session.notice), Tone::Warning));
    }
    let mut result = frame(screen, cols, cursor);
    result.question = Some(card.id().clone());
    result
}

/// The card above its hint: who asks, the question, its options and the
/// own-words row, whose index it returns.
fn card_lines(view: &View<'_>, card: &QuestionCard, cols: usize) -> (Vec<Line>, usize) {
    let session = view.session;
    let item = card.item();
    let count = card.question().items.len();
    let asker = session.actor_name(&card.question().actor);
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
    for text in wrap_words(&item.text, cols.saturating_sub(4)) {
        let mut row = filled(&text, cols, Tone::Input);
        row[0].emphasis.bold = true;
        lines.push(row);
    }
    lines.push(filled("", cols, Tone::Input));
    for (index, option) in item.options.iter().enumerate() {
        let mark = choice_mark(card, index);
        let label = format!(
            "{} {mark}{}. {}",
            pointer(card, index),
            index + 1,
            option.label
        );
        lines.push(selectable(
            &label,
            cols,
            card.cursor() == index && !card.typing(),
        ));
        for text in wrap_words(&option.description, cols.saturating_sub(10)) {
            lines.push(filled(&format!("       {text}"), cols, Tone::Muted));
        }
    }
    let own_row = lines.len();
    let own = card.own_words();
    let shown = if own.is_empty() && !card.typing() {
        "Other…".to_owned()
    } else {
        own.to_owned()
    };
    let text = format!("{}{shown}", own_prefix(card));
    let on_own = card.cursor() == item.options.len();
    lines.push(selectable(&text, cols, on_own && !card.typing()));
    lines.push(filled("", cols, Tone::Input));
    (lines, own_row)
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
    let tone = if selected {
        Tone::Selected
    } else {
        Tone::Input
    };
    let mut row = line(pad(text, cols), tone);
    row[0].emphasis.bold = selected;
    row
}
