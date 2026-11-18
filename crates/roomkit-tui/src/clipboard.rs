//! Explicit native clipboard reads, and copies, isolated from the terminal
//! event loop.
use crate::Session;
use crate::selection::clipboard_sequence;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use roomkit_tui_protocol::{ConversationId, ImageUpload, MAX_IMAGE_BYTES, MAX_IMAGE_PIXELS};
use std::time::Duration;
use std::{
    io::{self, Write},
    sync::mpsc::{self, Receiver, TryRecvError},
};

pub(crate) enum Paste {
    Image(ImageUpload),
    Text(String),
}

type PendingPaste = (Receiver<Result<Paste, String>>, Option<ConversationId>, u64);

/// Writes copied text to the clipboard of the computer the application runs
/// on.
type Writer = fn(&str) -> Result<(), String>;

#[derive(Default)]
pub(crate) struct Clipboard {
    pending: Option<PendingPaste>,
    suppress_until: Duration,
    /// Where a copy also writes the computer's own clipboard.
    writer: Option<Writer>,
    /// The answer of that write, while it runs.
    written: Option<Receiver<Result<(), String>>>,
}
impl Clipboard {
    /// A clipboard copying as the computer running the application allows.
    pub fn native() -> Self {
        Self {
            writer: native_writer(),
            ..Self::default()
        }
    }
    /// Copy `text`: the terminal is asked through OSC 52, and the computer's
    /// own clipboard written where a writer applies.
    pub fn copy(
        &mut self,
        session: &mut Session,
        out: &mut impl Write,
        text: &str,
    ) -> io::Result<()> {
        out.write_all(clipboard_sequence(text).as_bytes())?;
        out.flush()?;
        let Some(writer) = self.writer else {
            session.confirm_copy(false);
            return Ok(());
        };
        let (tx, rx) = mpsc::sync_channel(1);
        let text = text.to_owned();
        let spawn = std::thread::Builder::new()
            .name("clipboard-write".into())
            .spawn(move || {
                let _ = tx.send(writer(&text));
            });
        match spawn {
            Ok(_) => self.written = Some(rx),
            Err(_) => session.confirm_copy(false),
        }
        Ok(())
    }
    /// Tell a finished copy's write; true when it answered.
    fn poll_written(&mut self, session: &mut Session) -> bool {
        let Some(rx) = &self.written else {
            return false;
        };
        let copied = match rx.try_recv() {
            Ok(result) => result.is_ok(),
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => false,
        };
        self.written = None;
        session.confirm_copy(copied);
        true
    }
    pub fn reset_repeat(&mut self) {
        self.suppress_until = Duration::ZERO;
    }
    fn accept_paste(&mut self, now: Duration) -> bool {
        let suppressed = now < self.suppress_until;
        self.suppress_until = now.saturating_add(Duration::from_millis(250));
        !suppressed && self.pending.is_none()
    }
    pub fn start(&mut self, session: &mut Session, now: Duration) {
        if !self.accept_paste(now) {
            return;
        }
        let (tx, rx) = mpsc::sync_channel(1);
        let spawn = std::thread::Builder::new()
            .name("clipboard-read".into())
            .spawn(move || {
                let _ = tx.send(read());
            });
        if spawn.is_err() {
            session.notice = "Cannot start clipboard reader".into();
            return;
        }
        self.pending = Some((
            rx,
            session.snapshot.as_ref().map(|s| s.conversation.clone()),
            session.draft_epoch,
        ));
        session.notice = "Reading clipboard…".into();
    }
    pub fn poll(&mut self, session: &mut Session, now: Duration) -> bool {
        let written = self.poll_written(session);
        self.poll_paste(session, now) || written
    }
    fn poll_paste(&mut self, session: &mut Session, now: Duration) -> bool {
        let Some((rx, conversation, epoch)) = &self.pending else {
            return false;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Err("Clipboard reader stopped".into()),
        };
        let current = *epoch == session.draft_epoch
            && *conversation == session.snapshot.as_ref().map(|s| s.conversation.clone())
            && !session.images_pending();
        self.pending = None;
        self.suppress_until = now.saturating_add(Duration::from_millis(250));
        if current {
            session.notice = match result {
                Ok(Paste::Image(image)) => match session.add_image(image) {
                    Ok(()) => "Image attached · Alt+Backspace removes it".into(),
                    Err(error) => error.to_string(),
                },
                Ok(Paste::Text(text))
                    if session.editor.expanded_len().saturating_add(text.len()) <= 65_536 =>
                {
                    session.paste(&text);
                    String::new()
                }
                Ok(Paste::Text(_)) => "Input exceeds the 64 KiB composer limit".into(),
                Err(error) => error,
            };
        } else if session.notice == "Reading clipboard…" {
            session.notice.clear();
        }
        true
    }
}

/// macOS's Terminal ignores OSC 52 and iTerm2 refuses it by default, so a
/// copy there also writes the Mac's own clipboard, unless the application
/// runs on it over SSH, where that clipboard is not the person's. Elsewhere
/// the terminal alone copies.
fn native_writer() -> Option<Writer> {
    let over_ssh = ["SSH_CONNECTION", "SSH_TTY"]
        .into_iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
    (cfg!(target_os = "macos") && !over_ssh).then_some(write as Writer)
}

fn write(text: &str) -> Result<(), String> {
    arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.set_text(text))
        .map_err(|error| error.to_string())
}

fn read() -> Result<Paste, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(
        |_| "Native clipboard unavailable; use a local desktop terminal for image paste",
    )?;
    if let Ok(image) = clipboard.get_image() {
        return encode(image.width, image.height, &image.bytes).map(Paste::Image);
    }
    let text = clipboard
        .get_text()
        .map_err(|_| "The clipboard contains no readable image or text")?;
    if text.len() > 65_536 {
        return Err("Input exceeds the 64 KiB composer limit".into());
    }
    Ok(Paste::Text(text))
}

fn encode(width: usize, height: usize, bytes: &[u8]) -> Result<ImageUpload, String> {
    let width = u32::try_from(width).map_err(|_| "Image is too large")?;
    let height = u32::try_from(height).map_err(|_| "Image is too large")?;
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 || pixels > MAX_IMAGE_PIXELS || pixels * 4 != bytes.len() as u64 {
        return Err("Image exceeds 16 million pixels or has invalid dimensions".into());
    }
    let mut output = Bounded(Vec::new());
    {
        let mut encoder = png::Encoder::new(&mut output, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|_| "Cannot encode clipboard image")?;
        writer
            .write_image_data(bytes)
            .map_err(|_| "Cannot encode image within the 4 MiB limit")?;
        writer
            .finish()
            .map_err(|_| "Cannot finish encoding clipboard image")?;
    }
    Ok(ImageUpload::png(
        "clipboard.png",
        STANDARD.encode(output.0),
        width,
        height,
    ))
}
struct Bounded(Vec<u8>);
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_IMAGE_BYTES {
            return Err(io::Error::other("Image exceeds 4 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn buffered_paste_presses_do_not_duplicate_images() {
        let mut clipboard = Clipboard::default();
        assert!(clipboard.accept_paste(Duration::ZERO));
        assert!(!clipboard.accept_paste(Duration::from_millis(200)));
        assert!(!clipboard.accept_paste(Duration::from_millis(400)));
        clipboard.reset_repeat();
        assert!(clipboard.accept_paste(Duration::from_millis(410)));
        assert!(clipboard.accept_paste(Duration::from_millis(700)));
    }
    #[test]
    fn rgba_is_encoded_as_png_with_bounded_dimensions() -> Result<(), Box<dyn std::error::Error>> {
        let image = encode(1, 1, &[255, 0, 128, 255])?;
        let bytes = STANDARD.decode(&image.data)?;
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info()?;
        let mut pixels = vec![0; 4];
        reader.next_frame(&mut pixels)?;
        assert_eq!(pixels, [255, 0, 128, 255]);
        assert!(encode(16_000_001, 1, &[]).is_err());
        assert!(encode(1, 1, &[0]).is_err());
        assert!(!format!("{image:?}").contains(&image.data));
        Ok(())
    }
    /// Poll until the copy's own write of the clipboard has answered.
    fn written(clipboard: &mut Clipboard, session: &mut Session) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !clipboard.poll(session, Duration::ZERO) {
            assert!(
                std::time::Instant::now() < deadline,
                "no clipboard write answered"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// macOS's Terminal ignores OSC 52 and iTerm2 refuses it by default: a
    /// copy also writes the computer's own clipboard where it is the
    /// person's, and says so once that write succeeds. A failed write leaves
    /// the terminal's copy to tell; without a writer the terminal alone is
    /// asked.
    #[test]
    fn a_copy_writes_the_computer_s_clipboard_beside_asking_the_terminal() {
        static WRITTEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        fn writer(text: &str) -> Result<(), String> {
            WRITTEN.lock().map_err(|_| "poisoned")?.push(text.into());
            Ok(())
        }
        fn failing(_: &str) -> Result<(), String> {
            Err("No pasteboard".into())
        }
        let mut session = Session::default();
        let mut clipboard = Clipboard {
            writer: Some(writer),
            ..Clipboard::default()
        };
        let mut out = Vec::new();
        assert!(clipboard.copy(&mut session, &mut out, "selected").is_ok());
        assert_eq!(out, clipboard_sequence("selected").as_bytes());
        written(&mut clipboard, &mut session);
        assert!(WRITTEN.lock().is_ok_and(|texts| *texts == ["selected"]));
        assert_eq!(session.notice(), "Copied");

        session.notice.clear();
        clipboard.writer = Some(failing);
        assert!(
            clipboard
                .copy(&mut session, &mut Vec::new(), "other")
                .is_ok()
        );
        written(&mut clipboard, &mut session);
        assert!(
            session.notice().starts_with("Copy requested"),
            "{}",
            session.notice()
        );

        session.notice.clear();
        clipboard.writer = None;
        let mut out = Vec::new();
        assert!(clipboard.copy(&mut session, &mut out, "alone").is_ok());
        assert_eq!(out, clipboard_sequence("alone").as_bytes());
        assert!(!clipboard.poll(&mut session, Duration::ZERO));
        assert!(session.notice().starts_with("Copy requested"));
        assert!(WRITTEN.lock().is_ok_and(|texts| texts.len() == 1));
    }

    #[test]
    fn late_clipboard_result_cannot_resurrect_a_cleared_draft() {
        let mut session = Session::default();
        let (tx, rx) = mpsc::sync_channel(1);
        let mut clipboard = Clipboard {
            pending: Some((rx, None, session.draft_epoch)),
            ..Clipboard::default()
        };
        session.clear_composer();
        assert!(tx.send(Ok(Paste::Text("stale".into()))).is_ok());
        assert!(clipboard.poll(&mut session, Duration::ZERO));
        assert!(session.editor.text().is_empty());
    }
}
