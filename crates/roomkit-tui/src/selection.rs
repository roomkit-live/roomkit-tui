use std::ops::Range;

use base64::{Engine, engine::general_purpose::STANDARD};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::render::{ActionTarget, BackToBottomTarget, Frame, Span, Tone, ToolTarget};

/// A selection refers to a frozen visible frame, never a changing live stream.
pub(crate) struct TextSelection {
    frame: Frame,
    anchor: (u16, u16),
    head: (u16, u16),
    dragged: bool,
}

impl TextSelection {
    pub(crate) fn new(frame: Frame, column: u16, row: u16) -> Self {
        Self {
            frame,
            anchor: (column, row),
            head: (column, row),
            dragged: false,
        }
    }

    pub(crate) fn extend(&mut self, column: u16, row: u16) {
        let column = if let Some(sidebar) = &self.frame.sidebar {
            if usize::from(self.anchor.0) < sidebar.start {
                column.min(sidebar.start.saturating_sub(1) as u16)
            } else {
                column.max(sidebar.start as u16 + 2)
            }
        } else {
            column
        };
        self.head = (column, row);
        self.dragged |= self.head != self.anchor;
    }

    pub(crate) fn permission_click(&self) -> Option<&crate::render::PermissionTarget> {
        self.frame.permission_targets.iter().find(|target| {
            !self.dragged
                && target.row == usize::from(self.head.1)
                && target.start <= usize::from(self.head.0)
                && usize::from(self.head.0) < target.end
        })
    }

    pub(crate) fn action_click(&self) -> Option<&ActionTarget> {
        self.frame.action_targets.iter().find(|target| {
            !self.dragged
                && target.row == usize::from(self.head.1)
                && target.start <= usize::from(self.head.0)
                && usize::from(self.head.0) < target.end
        })
    }

    pub(crate) fn tool_click(&self) -> Option<&ToolTarget> {
        if self.dragged {
            return None;
        }
        self.frame.tool_targets.iter().find(|target| {
            target.row == usize::from(self.head.1)
                && target.start <= usize::from(self.head.0)
                && usize::from(self.head.0) < target.end
        })
    }

    pub(crate) fn back_to_bottom_click(&self) -> Option<&BackToBottomTarget> {
        self.frame.back_to_bottom.as_ref().filter(|target| {
            !self.dragged
                && target.row == usize::from(self.head.1)
                && target.start <= usize::from(self.head.0)
                && usize::from(self.head.0) < target.end
        })
    }

    fn range(&self, row: usize) -> Range<usize> {
        let (a, b) = if (self.anchor.1, self.anchor.0) <= (self.head.1, self.head.0) {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        };
        if row < usize::from(a.1) || row > usize::from(b.1) {
            return 0..0;
        }
        let start = if row == usize::from(a.1) {
            usize::from(a.0)
        } else {
            0
        };
        let end = if row == usize::from(b.1) {
            usize::from(b.0) + 1
        } else {
            usize::MAX
        };
        if let Some(sidebar) = &self.frame.sidebar {
            if usize::from(self.anchor.0) < sidebar.start {
                return start..end.min(sidebar.start);
            }
            return start.max(sidebar.start + 2)..end;
        }
        start..end
    }

    pub(crate) fn highlighted(&self) -> Frame {
        let mut frame = self.frame.clone();
        frame.cursor = None;
        if self.anchor == self.head {
            return frame;
        }
        for (row, spans) in frame.spans.iter_mut().enumerate() {
            let range = self.range(row);
            let mut column = 0;
            let mut highlighted: Vec<Span> = Vec::new();
            for span in spans.iter() {
                for grapheme in span.text.graphemes(true) {
                    let end = column + grapheme.width();
                    let tone = if column < range.end && end > range.start {
                        Tone::Selected
                    } else {
                        span.tone
                    };
                    if let Some(last) = highlighted
                        .last_mut()
                        .filter(|last| last.tone == tone && last.emphasis == span.emphasis)
                    {
                        last.text.push_str(grapheme);
                    } else {
                        highlighted.push(Span {
                            text: grapheme.into(),
                            tone,
                            emphasis: span.emphasis,
                        });
                    }
                    column = end;
                }
            }
            *spans = highlighted;
        }
        frame
    }

    pub(crate) fn text(&self) -> Result<Option<String>, &'static str> {
        if self.anchor == self.head {
            return Ok(None);
        }
        let mut lines = Vec::new();
        let mut bytes = 0;
        for (row, line) in self.frame.lines.iter().enumerate() {
            let range = self.range(row);
            if range.is_empty() {
                continue;
            }
            let mut column = 0;
            let mut selected = String::new();
            for grapheme in line.graphemes(true) {
                let end = column + grapheme.width();
                if column < range.end && end > range.start {
                    selected.push_str(grapheme);
                }
                column = end;
            }
            let selected = selected.trim_end().to_string();
            bytes += selected.len() + 1;
            if bytes > 65_536 {
                return Err("Selection exceeds the 64 KiB clipboard limit");
            }
            lines.push(selected);
        }
        let text = lines.join("\n");
        Ok((!text.trim().is_empty()).then_some(text))
    }
}

/// Only an explicit mouse release can call this path; it never reads a clipboard.
pub(crate) fn clipboard_sequence(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", STANDARD.encode(text.as_bytes()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "Invalid fixtures must fail the test")]
mod tests {
    use super::*;

    fn frame() -> Frame {
        let lines = vec!["a界e\u{301}z".to_string(), "second line   ".to_string()];
        let spans = lines
            .iter()
            .map(|text| {
                vec![Span {
                    text: text.clone(),
                    tone: Tone::Text,
                    emphasis: Default::default(),
                }]
            })
            .collect();
        Frame {
            permission_targets: Vec::new(),
            lines,
            spans,
            cursor: Some((0, 0)),
            tool_targets: Vec::new(),
            back_to_bottom: None,
            action_targets: Vec::new(),
            panel_scroll: None,
            sidebar: None,
            scroll: None,
            permission_scroll: None,
            approval: None,
        }
    }

    #[test]
    fn multiline_copy_does_not_include_the_other_pane() {
        let mut frame = frame();
        frame.spans = vec![
            vec![Span::new("hello   │ side", Tone::Text)],
            vec![Span::new("world   │ panel", Tone::Text)],
        ];
        frame.lines = vec!["hello   │ side".into(), "world   │ panel".into()];
        frame.sidebar = Some(crate::render::SidebarArea {
            start: 8,
            height: 2,
            scroll: 0,
        });
        let mut left = TextSelection::new(frame.clone(), 0, 0);
        left.extend(7, 1);
        assert_eq!(left.text().unwrap().unwrap(), "hello\nworld");
        let mut right = TextSelection::new(frame, 10, 0);
        right.extend(14, 1);
        assert_eq!(right.text().unwrap().unwrap(), "side\npanel");
    }

    #[test]
    fn reversed_selection_preserves_clusters_and_trims_padding() {
        let mut selection = TextSelection::new(frame(), 5, 1);
        selection.extend(2, 0); // Inside the wide glyph, selecting it whole.
        assert_eq!(
            selection.text().unwrap().as_deref(),
            Some("界e\u{301}z\nsecond")
        );
        assert_eq!(selection.highlighted().spans[0][1].text, "界e\u{301}z");
        assert!(selection.highlighted().cursor.is_none());
        let mut selection = TextSelection::new(frame(), 0, 0);
        assert!(selection.text().unwrap().is_none()); // A click never copies.
        selection.extend(300, 300); // Outside the visible frame is clamped.
        assert_eq!(
            selection.text().unwrap().as_deref(),
            Some("a界e\u{301}z\nsecond line")
        );
    }

    #[test]
    fn clipboard_payload_is_encoded_and_has_no_executable_terminal_controls() {
        let sequence = clipboard_sequence("text\x1b]52;c;attack\x07");
        let encoded = sequence
            .strip_prefix("\x1b]52;c;")
            .unwrap()
            .strip_suffix('\x07')
            .unwrap();
        assert!(!encoded.chars().any(char::is_control));
        assert_eq!(
            STANDARD.decode(encoded).unwrap(),
            b"text\x1b]52;c;attack\x07"
        );
    }

    #[test]
    fn a_disclosure_click_is_scoped_and_dragging_back_never_triggers_it() {
        let mut frame = frame();
        frame.tool_targets.push(ToolTarget {
            conversation: "room".into(),
            row: 0,
            start: 1,
            end: 4,
            actor: "agent".into(),
            run: "run".into(),
            call: "call".into(),
        });
        let mut click = TextSelection::new(frame.clone(), 2, 0);
        click.extend(2, 0);
        assert_eq!(click.tool_click().unwrap().call.as_str(), "call");
        assert!(click.text().unwrap().is_none());
        click.extend(3, 0);
        click.extend(2, 0);
        assert!(click.tool_click().is_none());
        let outside = TextSelection::new(frame, 4, 0);
        assert!(outside.tool_click().is_none());
    }

    #[test]
    fn application_action_requires_a_click_and_never_a_drag() {
        let mut frame = frame();
        frame.action_targets.push(ActionTarget {
            conversation: "room".into(),
            command: "jobs".into(),
            arguments: "stop:opaque".into(),
            panel: Some("panel".into()),
            sidebar: false,
            row: 1,
            start: 2,
            end: 8,
        });
        let mut selection = TextSelection::new(frame.clone(), 3, 1);
        assert_eq!(selection.action_click().unwrap().arguments, "stop:opaque");
        selection.extend(4, 1);
        selection.extend(3, 1);
        assert!(selection.action_click().is_none());
        assert!(TextSelection::new(frame, 8, 1).action_click().is_none());
    }

    #[test]
    fn return_to_bottom_requires_a_click_not_a_text_selection() {
        let mut frame = frame();
        frame.back_to_bottom = Some(BackToBottomTarget {
            conversation: "room".into(),
            row: 1,
            start: 2,
            end: 8,
        });
        let mut click = TextSelection::new(frame.clone(), 2, 1);
        assert_eq!(
            click.back_to_bottom_click().unwrap().conversation.as_str(),
            "room"
        );
        assert!(click.text().unwrap().is_none());
        click.extend(3, 1);
        click.extend(2, 1);
        assert!(click.back_to_bottom_click().is_none());
        assert!(
            TextSelection::new(frame.clone(), 8, 1)
                .back_to_bottom_click()
                .is_none()
        );
        assert!(
            TextSelection::new(frame, 2, 0)
                .back_to_bottom_click()
                .is_none()
        );
    }
}
