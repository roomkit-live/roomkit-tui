//! Image attachments, transferred as bytes and never as paths.

use serde::{Deserialize, Serialize};

/// Image bytes transferred from a client, never a path on either machine.
/// Hosts validate the bytes and persist authorized opaque references themselves.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ImageUpload {
    /// Display filename, without a directory.
    pub name: String,
    /// MIME type; this first transport supports normalized PNG clipboard images.
    pub mime_type: String,
    /// Base64-encoded file bytes. Debug deliberately omits their contents.
    pub data: String,
    /// Display width in pixels, verified by the host decoder.
    pub width: u32,
    /// Display height in pixels, verified by the host decoder.
    pub height: u32,
}

impl ImageUpload {
    /// A base64-encoded PNG image and its claimed dimensions.
    pub fn png(name: impl Into<String>, data: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            name: name.into(),
            mime_type: "image/png".into(),
            data: data.into(),
            width,
            height,
        }
    }
}

impl std::fmt::Debug for ImageUpload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageUpload")
            .field("name", &self.name)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// An author message with normalized image uploads, validated by the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ImageMessage {
    /// User text accompanying the images; may be empty.
    pub text: String,
    /// At most four PNG images; never filesystem paths or host URLs.
    pub images: Vec<ImageUpload>,
}

impl ImageMessage {
    /// Text and its attached images.
    pub fn new(text: impl Into<String>, images: Vec<ImageUpload>) -> Self {
        Self {
            text: text.into(),
            images,
        }
    }
}
