use super::*;
use crate::render::{self, Line, Renderers, Span, Tone, ToolRenderer};
use unicode_width::UnicodeWidthStr;

struct Preview;
impl ToolRenderer for Preview {
    fn render(&self, call: &ToolCall) -> Vec<String> {
        call.text.lines().map(str::to_owned).collect()
    }

    fn render_styled(&self, call: &ToolCall, _: bool) -> Vec<Line> {
        self.render(call)
            .into_iter()
            .map(|text| vec![Span::new(text, Tone::Code)])
            .collect()
    }
}

fn result(id: &str, text: &str, state: ToolState) -> Event {
    let mut event = call(id, Some(id), state);
    if let Event::Item {
        item: Item::Tool { call, .. },
        ..
    } = &mut event
    {
        call.text = text.into();
    }
    event
}

#[test]
fn activity_reuses_styled_previews_and_replaces_the_previous_body() {
    let mut session = session();
    started(&mut session, "run");
    let mut renderers = Renderers::default();
    renderers.register("unknown-domain-tool", Preview);
    let options = UiOptions {
        tool_activity: true,
        tool_preview_lines: 2,
        ..Default::default()
    };
    let draw = |session: &Session| {
        render::screen_with_options(session, 48, 40, &renderers, &options, Duration::ZERO)
    };
    session.apply(result("first", "Earlier result", ToolState::Succeeded));
    session.apply(result("second", "", ToolState::Running));
    assert!(!draw(&session).lines.join("\n").contains("Earlier result"));
    session.apply(result(
        "second",
        "Current 文档\n\x1b[31mColored source\nMore details",
        ToolState::Running,
    ));
    let frame = draw(&session);
    let text = frame.lines.join("\n");
    assert!(text.contains("Current 文档") && text.contains("Colored source"));
    assert!(!text.contains("Earlier result") && !text.contains("More details"));
    assert!(
        frame
            .lines
            .iter()
            .all(|line| line.width() <= 48 && !line.contains('\x1b'))
    );
    assert!(
        frame
            .spans
            .iter()
            .flatten()
            .any(|span| span.tone == Tone::Code)
    );
    assert_eq!(frame.tool_targets.len(), 1);
    let target = &frame.tool_targets[0];
    assert_eq!(target.call.as_str(), "second");
    assert!(frame.lines[target.row].contains("second"));

    session.apply(result(
        "second",
        "Final result\nSecond line\nMore details",
        ToolState::Succeeded,
    ));
    let text = draw(&session).lines.join("\n");
    assert!(text.contains("Final result"));
    assert!(!text.contains("Current 文档"));
    session.expanded = true;
    let text = draw(&session).lines.join("\n");
    assert!(text.contains("Earlier result") && text.contains("More details"));
    session.expanded = false;
    message(&mut session, "explanation", "Findings stay visible.");
    session.apply(result("third", "Next result", ToolState::Succeeded));
    let frame = draw(&session);
    assert_eq!(frame.tool_targets.len(), 2);
    assert!(frame.lines.join("\n").contains("Final result"));
    assert!(frame.lines.join("\n").contains("Next result"));
}
