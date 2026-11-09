//! The status and hint rows under the composer.

use std::time::Duration;

use roomkit_tui_protocol::{ConversationCost, SidebarContent, Snapshot};

use super::text::sanitize;
use super::{Line, Session, Span, Tone, line, span};
use unicode_width::UnicodeWidthStr;

pub(super) fn footer(
    session: &Session,
    cols: usize,
    sidebar: bool,
    elapsed: Duration,
) -> Vec<Line> {
    let mut lines = status(session, cols, sidebar);
    // A short mode indicator shares the hint row while the sidebar carries
    // session details. Keep full-width hints if a special input mode needs them.
    if sidebar && let Some(last) = lines.last_mut() {
        let used: usize = last.iter().map(|part| sanitize(&part.text).width()).sum();
        let available = cols.saturating_sub(used + 2);
        let hints = hints(session, available, sidebar, elapsed);
        if available >= 40 && sanitize(&hints).width() <= available {
            last.push(span(" ·", Tone::Muted));
            last.push(span(hints, Tone::Muted));
            return lines;
        }
    }
    lines.push(line(hints(session, cols, sidebar, elapsed), Tone::Muted));
    lines
}

pub(super) fn hints(session: &Session, cols: usize, sidebar: bool, elapsed: Duration) -> String {
    let action_hint = session.input_action().map(|action| {
        format!(
            " {} {} · Enter run · Alt+Enter newline · Ctrl+C clear",
            action.prefix, action.description
        )
    });
    let quit = quit_hint(session, elapsed).map(|hint| format!(" {hint}"));
    let hints = if let Some(hint) = quit.as_deref() {
        hint
    } else if session.picker.is_some() {
        " Type to filter · ↑↓ choose · Enter confirm · Esc cancel"
    } else if session.menu.visible {
        " ↑↓ choose · Enter/Tab insert · Esc close · Enter again to send"
    } else if let Some(hint) = action_hint.as_deref() {
        hint
    } else if session.answers_after_interruption() {
        " Answers once the work stops · Esc cancel"
    } else if session.sends_after_interruption() {
        " Sends once the work stops · Esc keep in composer"
    } else if session
        .snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.active_run.is_some() && snapshot.can_interrupt)
        && !session.interrupting()
    {
        if session.draft().is_empty() {
            " Esc interrupt · Enter send · Alt+Enter newline · F2 details"
        } else if cols >= 80 {
            " Esc interrupt · Enter send · Ctrl+S send now · Alt+Enter newline · F2 details"
        } else {
            " Esc interrupt · Enter send · Ctrl+S send now"
        }
    } else if session
        .snapshot
        .as_ref()
        .is_none_or(|snapshot| snapshot.commands.is_empty())
    {
        if cols >= 80 {
            " Enter send · Alt+Enter newline · F2 details · Alt+↑↓ tools · Ctrl+D detach"
        } else {
            " Enter send · Alt+Enter newline · F2 details"
        }
    } else if session
        .snapshot
        .as_ref()
        .is_some_and(|s| s.can_attach_images)
        && cols >= 80
    {
        " Enter send · Ctrl+V paste image · Alt+Enter newline · Tab commands · F2 details"
    } else if cols >= 100 {
        " Enter send · Alt+Enter newline · Tab commands · F2 details · Alt+↑↓ tools · PgUp/PgDn scroll"
    } else if cols >= 60 {
        " Enter send · Alt+Enter newline · Tab commands · F2 details"
    } else {
        " Enter send · Tab menu · F2 details"
    };
    if session.sidebar.is_some() && !sidebar {
        format!(" F7 sidebar ·{hints}")
    } else {
        hints.to_string()
    }
}

/// The armed exit's confirmation, naming what the host says exiting stops.
/// Ctrl+D arms the exit whichever screen has focus, so every screen shows it.
pub(super) fn quit_hint(session: &Session, elapsed: Duration) -> Option<String> {
    if !session
        .quit_until
        .is_some_and(|deadline| elapsed < deadline)
    {
        return None;
    }
    Some(match &session.exit_warning {
        Some(warning) => format!("Press {} again to exit · {warning}", session.quit_key),
        None => format!("Press {} again to exit", session.quit_key),
    })
}

pub(super) fn settings(
    session: &Session,
    snapshot: &Snapshot,
    include_agent: bool,
    compact: bool,
) -> Line {
    let mut parts = Vec::new();
    if let Some(mode) = &snapshot.mode
        && let Some(choice) = mode
            .choices
            .iter()
            .find(|choice| choice.value == mode.selected)
    {
        parts.push(span(format!(" [{}]", choice.label), Tone::Warning));
    }
    if include_agent {
        if !parts.is_empty() {
            parts.push(span(" ·", Tone::Muted));
        }
        parts.push(span(
            format!(" {}", session.actor_name(&snapshot.selected_agent)),
            Tone::Primary,
        ));
        if let Some(model) = &snapshot.model {
            parts.push(span(
                if compact {
                    format!(" · {}", model.model)
                } else {
                    format!(" · {}/{}", model.provider, model.model)
                },
                Tone::Accent,
            ));
            parts.push(span(
                format!(" · effort {}", model.effort.as_deref().unwrap_or("n/a")),
                Tone::Muted,
            ));
        }
    }
    parts
}

fn shows_agent(content: &SidebarContent) -> bool {
    matches!(content, SidebarContent::Agent)
}

/// A context section, plain or broken down, makes the footer's redundant.
fn shows_context(content: &SidebarContent) -> bool {
    matches!(
        content,
        SidebarContent::Context | SidebarContent::ContextBreakdown { .. }
    )
}

fn status(session: &Session, width: usize, sidebar: bool) -> Vec<Line> {
    let Some(snapshot) = &session.snapshot else {
        return vec![line(" Waiting for the host", Tone::Muted)];
    };
    let in_sidebar = |shown: fn(&SidebarContent) -> bool| {
        sidebar
            && session.sidebar.as_ref().is_some_and(|s| {
                s.spec
                    .sections
                    .iter()
                    .any(|section| shown(&section.content))
            })
    };
    let mut parts = settings(session, snapshot, !in_sidebar(shows_agent), false);
    if in_sidebar(shows_context) {
        return if parts.is_empty() {
            Vec::new()
        } else {
            vec![parts]
        };
    }
    let mut settings = std::mem::take(&mut parts);
    let refused = session.context_exceeded();
    let context = session.displayed_context().map_or_else(
        || {
            if refused {
                "context refused"
            } else {
                "context ?"
            }
            .into()
        },
        |(usage, previous)| {
            let estimate = if usage.estimated { "~" } else { "" };
            // Put freshness before the numbers so clipping a narrow footer
            // cannot make an earlier report look like a current measurement,
            // nor hide that the latest request was refused. A refused request
            // was never measured: whatever is shown predates it.
            let label = match (refused, previous) {
                (true, _) => "context refused (last report)",
                (false, true) => "context (last report)",
                (false, false) => "context",
            };
            match usage.percent() {
                Some(percent) => format!(
                    "{label} {estimate}{}/{} · {:.1}% left · {percent:.1}% used",
                    usage.tokens,
                    usage.capacity.unwrap_or(0),
                    (100.0 - percent).max(0.0)
                ),
                None => format!("{label} {estimate}{} tokens / ?", usage.tokens),
            }
        },
    );
    parts.push(span(format!(" {context}"), Tone::Accent));
    if let Some(usage) = session.usage() {
        let input = usage
            .input_tokens
            .map_or_else(|| "?".into(), |n| n.to_string());
        let output = usage
            .output_tokens
            .map_or_else(|| "?".into(), |n| n.to_string());
        let partial = if usage.complete { "" } else { " (partial)" };
        let cache = super::context::cache_note(usage);
        parts.push(span(
            format!(" · turn {input} in{cache} / {output} out{partial}"),
            Tone::Muted,
        ));
    }
    parts.extend(costs(session));
    let combined_width: usize = settings
        .iter()
        .chain(&parts)
        .map(|part| sanitize(&part.text).width())
        .sum();
    if settings.is_empty() || combined_width.saturating_add(2) <= width {
        if !settings.is_empty() {
            settings.push(span(" ·", Tone::Muted));
        }
        settings.extend(parts);
        vec![settings]
    } else {
        vec![settings, parts]
    }
}

/// The conversation's own cost and its total with the work related to it,
/// each saying when it is partial.
fn costs(session: &Session) -> Vec<Span> {
    let part = |prefix: &str, cost: &ConversationCost| {
        let partial = if cost.complete { "" } else { " (partial)" };
        let amount = super::context::amount(cost);
        span(format!(" · {prefix}{amount}{partial}"), Tone::Muted)
    };
    let own = session.cost().map(|cost| part("", cost));
    let total = session.total_cost().map(|cost| part("total ", &cost));
    own.into_iter().chain(total).collect()
}
