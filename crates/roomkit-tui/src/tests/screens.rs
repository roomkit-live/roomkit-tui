#![allow(
    clippy::unwrap_used,
    reason = "Fixtures and assertions fail the test on invalid state"
)]

use roomkit_tui_protocol::*;
use unicode_width::UnicodeWidthStr;

use super::*;
use crate::Session;
use crate::render::{self, Renderers};

#[test]
fn background_panel_is_scoped_preserves_draft_and_rejects_stale_actions() {
    let mut s = session();
    s.editor.insert("unfinished draft");
    s.apply(Event::BackgroundChanged {
        conversation: "room-b".into(),
        status: Some(BackgroundStatus::new("Jobs", "summarize", "open")),
    });
    assert!(s.background.is_none());
    s.apply(Event::BackgroundChanged {
        conversation: "room-a".into(),
        status: Some(BackgroundStatus::new("2 running", "summarize", "open")),
    });
    let options = crate::UiOptions {
        selection: crate::SelectionMode::CopyOnSelect,
        ..Default::default()
    };
    let frame = render::screen_with_options(
        &s,
        100,
        25,
        &Renderers::default(),
        &options,
        std::time::Duration::ZERO,
    );
    let badge = frame.action_targets.first().unwrap();
    assert!(s.action_command(badge).is_some());
    let panel = PanelSpec::new("panel-1", "summarize", "Background work", "first\nsecond")
        .with_actions(vec![
            Choice::new("stop:1", "Stop").with_description("Job one"),
        ])
        .with_dismiss("dismiss:1");
    s.apply(Event::Panel {
        conversation: "room-a".into(),
        panel: panel.clone(),
    });
    s.panel.as_mut().unwrap().scroll = 10_000;
    let frame = render::screen(&s, 80, 24, &Renderers::default());
    assert_eq!(frame.panel_scroll, Some(("panel-1".into(), 0)));
    assert!(frame.lines.join("\n").contains("Background work"));
    assert_eq!(frame.action_targets.len(), 1);
    assert!(s.action_command(&frame.action_targets[0]).is_some());
    assert!(s.action_command(badge).is_none());
    assert_eq!(s.editor.text(), "unfinished draft");
    s.apply(Event::OperationStarted {
        conversation: "other-room".into(),
        operation: "foreign".into(),
    });
    assert!(s.panel.is_some());
    s.apply(Event::OperationStarted {
        conversation: "room-a".into(),
        operation: "panel-action".into(),
    });
    assert!(s.panel.is_none());
    assert_eq!(s.editor.text(), "unfinished draft");
    s.apply(Event::PanelUpdated {
        conversation: "room-a".into(),
        panel,
    });
    assert!(s.panel.is_none());
    assert!(s.action_command(&frame.action_targets[0]).is_none());
    s.apply(Event::Snapshot(snapshot()));
    assert!(s.background.is_none());
}

#[test]
fn background_and_return_to_bottom_share_a_bounded_activity_row() {
    let mut s = session();
    s.apply(Event::Item {
        conversation: "room-a".into(),
        item: Item::Message(Message::new("long", "agent", "history line\n\n".repeat(60))),
    });
    s.scroll = 12;
    s.apply(Event::BackgroundChanged {
        conversation: "room-a".into(),
        status: Some(BackgroundStatus::new(
            "Processes: 2 running",
            "summarize",
            "open",
        )),
    });
    let options = crate::UiOptions {
        selection: crate::SelectionMode::CopyOnSelect,
        ..Default::default()
    };
    for width in [12, 20, 36, 80, 160] {
        let frame = render::screen_with_options(
            &s,
            width,
            24,
            &Renderers::default(),
            &options,
            std::time::Duration::ZERO,
        );
        assert!(frame.lines.iter().all(|l| l.width() <= usize::from(width)));
        let bottom = frame.back_to_bottom.as_ref().unwrap();
        for action in &frame.action_targets {
            assert_eq!(action.row, bottom.row);
            assert!(action.end <= bottom.start);
        }
    }
}

#[test]
fn centered_welcome_reuses_the_composer_and_restores_conversation_layout() {
    use crate::{UiOptions, WelcomeScreen};
    let mut s = session();
    s.apply(Event::SidebarChanged {
        conversation: "room-a".into(),
        sidebar: Some(sidebar_spec()),
    });
    let options = UiOptions {
        welcome: Some(WelcomeScreen {
            logo: vec!["╭─╮".into(), "│▦│".into(), "╰─╯".into()],
            logo_tiles: Vec::new(),
            wordmark: vec!["CUSTOM MARK\x1b[2J".into()],
            width: 80,
        }),
        bottom_margin: 1,
        ..UiOptions::default()
    };
    let renderers = Renderers::default();
    let draw = |s: &Session, w, h| {
        render::screen_with_options(s, w, h, &renderers, &options, std::time::Duration::ZERO)
    };
    s.editor.insert("An unsent draft");
    let home = draw(&s, 140, 40);
    assert!(home.sidebar.is_none());
    assert!(home.lines.join("\n").contains("CUSTOM MARK"));
    assert!(!home.lines.join("\n").contains('\x1b'));
    let (x, y) = home.cursor.unwrap();
    assert!(x > 30 && (15..30).contains(&y));
    assert!(home.lines[39].trim().is_empty());
    assert_eq!(s.editor.text(), "An unsent draft");
    s.snapshot.as_mut().unwrap().can_attach_images = true;
    s.add_image(image_fixture()).unwrap();
    assert!(draw(&s, 140, 40).lines.join("\n").contains("Image 1"));
    assert_eq!(s.editor.text(), "An unsent draft");
    s.images.clear();
    s.editor.set("/".into());
    s.refresh_menu();
    assert!(draw(&s, 140, 40).lines.join("\n").contains("summarize"));
    assert_eq!(s.editor.text(), "/");
    s.menu.visible = false;
    s.editor.set("An unsent draft".into());
    s.apply(Event::Item {
        conversation: "room-a".into(),
        item: Item::Message(Message::new("first", "agent", "First reply")),
    });
    let conversation = draw(&s, 140, 40);
    assert!(conversation.sidebar.is_some());
    assert!(!conversation.lines.join("\n").contains("CUSTOM MARK"));
    assert!(conversation.lines.join("\n").contains("First reply"));
    assert!(conversation.cursor.unwrap().1 >= 35);
    assert!(conversation.lines[39].is_empty());
    s.apply(Event::Snapshot(snapshot()));
    assert!(draw(&s, 140, 40).lines.join("\n").contains("CUSTOM MARK"));
    s.apply(Event::Notice {
        conversation: "room-a".into(),
        text: "Please check this setting".into(),
    });
    // A notice at the start, such as a setup step, keeps the welcome layout:
    // it shows above the composer instead of replacing the whole screen.
    let noticed = draw(&s, 140, 40);
    let text = noticed.lines.join("\n");
    assert!(text.contains("Please check this setting"), "{text}");
    assert!(
        text.contains("CUSTOM MARK"),
        "The notice hid the welcome screen"
    );
    assert!(noticed.sidebar.is_none());
    let notice_row = noticed
        .lines
        .iter()
        .position(|row| row.contains("Please check this setting"))
        .unwrap();
    assert!(usize::from(noticed.cursor.unwrap().1) > notice_row);
    // A long notice is cut to a few rows, and says so.
    s.notice = "word ".repeat(200);
    let long = draw(&s, 140, 40);
    let rows: Vec<_> = long
        .lines
        .iter()
        .filter(|row| row.contains("word"))
        .collect();
    assert_eq!(rows.len(), 3);
    assert!(rows[2].trim_end().ends_with('…'));
    assert!(long.lines.join("\n").contains("CUSTOM MARK"));
    s.notice.clear();
    for width in [0, 11, 40, 60, 80, 140] {
        for height in [0, 7, 18, 20, 24, 40] {
            let frame = draw(&s, width, height);
            assert!(frame.lines.len() <= usize::from(height));
            assert!(
                frame
                    .lines
                    .iter()
                    .all(|row| row.width() <= usize::from(width))
            );
            if let Some((x, y)) = frame.cursor {
                assert!(x < width && y < height);
            }
        }
    }
}

#[test]
fn application_caption_is_scoped_bounded_and_preserves_the_welcome_composer() {
    let mut s = session();
    s.editor.insert("Draft");
    let options = crate::UiOptions {
        welcome: Some(crate::WelcomeScreen::default()),
        ..Default::default()
    };
    s.apply(Event::ApplicationInfo {
        conversation: "room-a".into(),
        text: Some("Version 1.0 · /update".into()),
    });
    let frame = render::screen_with_options(
        &s,
        100,
        40,
        &Renderers::default(),
        &options,
        std::time::Duration::ZERO,
    );
    assert!(frame.lines.iter().any(|l| l.contains("Version 1.0")));
    assert!(frame.lines.iter().any(|l| l.contains("Draft")));
    s.apply(Event::ApplicationInfo {
        conversation: "other".into(),
        text: None,
    });
    s.apply(Event::ApplicationInfo {
        conversation: "room-a".into(),
        text: Some("x".repeat(257)),
    });
    assert_eq!(s.application_info.as_deref(), Some("Version 1.0 · /update"));
    s.apply(Event::Snapshot(snapshot()));
    assert!(s.application_info.is_none());
}

fn title_changed(conversation: &str, title: impl Into<String>) -> Event {
    Event::TitleChanged {
        conversation: conversation.into(),
        title: title.into(),
    }
}

fn header(s: &mut Session) -> String {
    let frame = render::screen(s, 80, 24, &Renderers::default());
    s.sync(&frame, std::time::Duration::ZERO);
    frame.lines[..frame.timeline.as_ref().unwrap().top].join("\n")
}

#[test]
fn a_title_change_renames_the_header_in_place_during_a_run() {
    let mut s = session();
    s.apply(Event::Item {
        conversation: "room-a".into(),
        item: Item::Message(Message::new("long", "agent", "history line\n\n".repeat(60))),
    });
    started(&mut s, "run-1");
    s.editor.insert("unfinished draft");
    s.scroll_back(7);
    assert!(header(&mut s).contains("Research"));
    let scroll = s.scroll();

    s.apply(title_changed("room-a", "Quarterly review"));
    let shown = header(&mut s);
    assert!(shown.contains("Quarterly review") && !shown.contains("Research"));
    assert!(s.is_executing(), "Renaming never settles the run");
    assert_eq!(s.editor.text(), "unfinished draft");
    assert_eq!(s.scroll(), scroll);
    assert_eq!(s.snapshot().unwrap().items.len(), 1);

    s.apply(title_changed("room-b", "Another view"));
    s.apply(title_changed("room-a", "x".repeat(257)));
    assert_eq!(s.snapshot().unwrap().title, "Quarterly review");
    s.apply(title_changed("room-a", "x".repeat(256)));
    assert_eq!(s.snapshot().unwrap().title.len(), 256);

    s.apply(title_changed("room-a", "Plain\u{1b}[2J\u{7}\u{202E}title"));
    let shown = header(&mut s);
    assert!(shown.contains("Plain[2Jtitle"), "{shown}");
    assert!(!shown.contains(['\u{1b}', '\u{7}', '\u{202E}']));
}

/// A logo of colored pieces is drawn piece by piece, in its own colors, in
/// the header and on the welcome screen; a mark set as plain text clears it.
#[test]
fn a_logo_of_colored_pieces_keeps_its_colors() {
    use crate::render::Tone;
    use crate::{Branding, Color, LogoTile, UiOptions, WelcomeScreen};
    let (first, second) = (Color::rgb(99, 102, 241), Color::rgb(55, 58, 130));
    let rows = vec![
        vec![
            LogoTile::colored("██", first),
            LogoTile::gap(" "),
            LogoTile::colored("██", second),
        ],
        vec![LogoTile::colored("\x1b[2J██", second)],
    ];
    let branding = Branding::new("Code").with_logo_tiles(rows.clone());
    assert_eq!(branding.logo, ["██ ██", "\x1b[2J██"]);
    let options = UiOptions {
        branding,
        welcome: Some(WelcomeScreen::default().with_logo_tiles(rows.clone())),
        ..UiOptions::default()
    };
    let renderers = Renderers::default();
    let tones = |frame: &crate::render::Frame| -> Vec<Tone> {
        frame.spans.iter().flatten().map(|span| span.tone).collect()
    };
    let session_on = session();
    let welcome = render::screen_with_options(
        &session_on,
        140,
        40,
        &renderers,
        &options,
        std::time::Duration::ZERO,
    );
    assert!(tones(&welcome).contains(&Tone::Mark(first)));
    assert!(tones(&welcome).contains(&Tone::Mark(second)));

    let options = UiOptions {
        welcome: None,
        ..options
    };
    let header = render::screen_with_options(
        &session_on,
        140,
        40,
        &renderers,
        &options,
        std::time::Duration::ZERO,
    );
    assert!(tones(&header).contains(&Tone::Mark(first)));
    assert!(header.lines[0].contains("██ ██"), "{:?}", header.lines[0]);
    assert!(!header.lines.join("\n").contains('\x1b'));

    let plain = Branding::new("Code")
        .with_logo_tiles(rows)
        .with_logo(vec!["AB".into()]);
    assert!(plain.logo_tiles.is_empty());
}

/// A frame's first row, with `branding` in its header.
fn header_row(branding: crate::Branding) -> String {
    let options = crate::UiOptions {
        branding,
        welcome: None,
        ..crate::UiOptions::default()
    };
    let session_on = session();
    let frame = render::screen_with_options(
        &session_on,
        140,
        40,
        &Renderers::default(),
        &options,
        std::time::Duration::ZERO,
    );
    frame.lines[0].clone()
}

#[test]
fn colored_pieces_stay_within_the_width_a_plain_logo_gets() {
    use crate::{Branding, LogoTile};
    let color = crate::Color::rgb(1, 2, 3);
    let (left, right) = ("A".repeat(10), "B".repeat(10));
    let tiles = Branding::new("Code").with_logo_tiles(vec![vec![
        LogoTile::colored(left.clone(), color),
        LogoTile::gap("  "),
        LogoTile::colored(right.clone(), color),
    ]]);
    let plain = Branding::new("Code").with_logo(vec![format!("{left}  {right}")]);

    assert_eq!(header_row(tiles), header_row(plain));
}

#[test]
fn a_logo_written_after_its_pieces_is_the_one_drawn() {
    use crate::{Branding, LogoTile};
    let color = crate::Color::rgb(1, 2, 3);
    let mut branding = Branding::new("Code").with_logo_tiles(vec![vec![
        LogoTile::colored("AA", color),
        LogoTile::gap(" "),
        LogoTile::colored("BB", color),
    ]]);
    branding.logo = vec!["XYZWV".into()];

    let row = header_row(branding);

    assert!(row.contains("XYZWV"), "{row:?}");
    assert!(!row.contains("AA"), "{row:?}");
}
