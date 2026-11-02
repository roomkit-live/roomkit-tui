//! Message presentation using the host's identity for this view.

use roomkit_tui_protocol::Message;

use unicode_width::UnicodeWidthStr;

use super::text::input_gutter;
use super::{Line, Tone, View, line, span};
use crate::Steering;

pub(super) fn render(view: &View<'_>, message: &Message, cols: usize) -> Vec<Line> {
    let (session, options) = (view.session, view.options);
    let Some(snapshot) = &session.snapshot else {
        return Vec::new();
    };
    let own =
        options.highlight_own_messages && snapshot.local_actor.as_ref() == Some(&message.actor);
    let gutter = if own {
        input_gutter(&options.input_prompt)
    } else {
        String::new()
    };
    let mut lines = Vec::new();
    if own {
        lines.push(line(" ".repeat(cols), Tone::Input));
    } else {
        lines.push(line(
            format!(" ● {}", session.actor_name(&message.actor)),
            if message.actor == snapshot.selected_agent {
                Tone::Primary
            } else {
                Tone::Accent
            },
        ));
    }
    let key = format!("{}\u{0}{}", message.actor, message.id);
    let width = cols.saturating_sub(4 + gutter.width());
    let mut content = view
        .renderers
        .markdown(&key, &message.text, width, options.markdown);
    if own && content.is_empty() {
        content.push(Vec::new());
    }
    for (index, mut text) in content.into_iter().enumerate() {
        if own {
            // Reuse the composer's foreground/background pair, including its
            // monochrome contrast. Preserve Markdown emphasis and visible text.
            for part in &mut text {
                part.tone = Tone::Input;
            }
            text.insert(
                0,
                span(
                    if index == 0 {
                        gutter.clone()
                    } else {
                        " ".repeat(gutter.width())
                    },
                    Tone::InputPrompt,
                ),
            );
            text.insert(0, span("  ", Tone::Input));
            let used = text.iter().map(|part| part.text.width()).sum::<usize>();
            text.push(span(" ".repeat(cols.saturating_sub(used)), Tone::Input));
        } else {
            text.insert(0, span("   ", Tone::Text));
        }
        lines.push(text);
    }
    if own {
        lines.push(line(" ".repeat(cols), Tone::Input));
    }
    if let Some(steering) = session.steering(&message.id) {
        lines.push(line(
            format!("   ↳ {}", steering_note(steering)),
            Tone::Muted,
        ));
    }
    lines
}

/// Where a message sent during a turn went (RMK-1161).
fn steering_note(steering: Steering) -> &'static str {
    match steering {
        Steering::Handed => "sent to the running turn",
        Steering::OwnTurn => "answered in its own turn",
    }
}
