//! Explicit native clipboard reads, isolated from the terminal event loop.
use crate::Session;
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

#[derive(Default)]
pub(crate) struct Clipboard {
    pending: Option<PendingPaste>,
    suppress_until: Duration,
}
impl Clipboard {
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
