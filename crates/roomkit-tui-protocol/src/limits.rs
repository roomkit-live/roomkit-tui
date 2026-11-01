//! Bounds shared by terminals and hosts. A host may accept less; a terminal
//! rejects or truncates anything larger instead of growing without limit.

/// Maximum bytes of a submitted message, command arguments or streamed text.
pub const MAX_TEXT_BYTES: usize = 65_536;
/// Maximum bytes of an operation identity.
pub const MAX_OPERATION_ID_BYTES: usize = 256;
/// Maximum bytes of a permission title.
pub const MAX_PERMISSION_TITLE_BYTES: usize = 256;
/// Maximum bytes of the details a person approves. Larger requests are refused,
/// never shown truncated: a person must see everything they approve.
pub const MAX_PERMISSION_DETAIL_BYTES: usize = 65_536;
/// Maximum bytes of a permission's history summary.
pub const MAX_PERMISSION_SUMMARY_BYTES: usize = 512;
/// Maximum permissions waiting at once in one conversation.
pub const MAX_PENDING_PERMISSIONS: usize = 64;
/// Maximum bytes of a short label (a title, caption, status or argument value)
/// or of an identity a terminal tracks: a conversation, run or permission.
/// Terminals ignore a `RunStarted` or `PermissionRequested` whose identity is
/// longer.
pub const MAX_LABEL_BYTES: usize = 256;
/// Maximum advertised commands or actors in one snapshot.
pub const MAX_SNAPSHOT_ENTRIES: usize = 256;
/// Maximum bytes of a command's description.
pub const MAX_COMMAND_DESCRIPTION_BYTES: usize = 1_024;
/// Maximum composer actions in one snapshot.
pub const MAX_INPUT_ACTIONS: usize = 8;
/// Maximum bytes of a composer action's guidance.
pub const MAX_INPUT_ACTION_DESCRIPTION_BYTES: usize = 120;
/// Maximum choices in one selector or form field.
pub const MAX_CHOICES: usize = 1_024;
/// Maximum interaction modes a host offers at once.
pub const MAX_MODES: usize = 32;
/// Maximum bytes of an interaction mode's label.
pub const MAX_MODE_LABEL_BYTES: usize = 80;
/// Maximum bytes of an interaction mode's description.
pub const MAX_MODE_DESCRIPTION_BYTES: usize = 512;
/// Maximum fields in one form.
pub const MAX_FORM_FIELDS: usize = 32;
/// Maximum bytes of a submitted form value.
pub const MAX_FORM_VALUE_BYTES: usize = 8_192;
/// Maximum bytes of a form field name.
pub const MAX_FORM_NAME_BYTES: usize = 64;
/// Maximum number of images submitted in one message.
pub const MAX_IMAGES: usize = 4;
/// Maximum encoded file size per image before base64 transport (4 MiB).
pub const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
/// Maximum base64 text length of one image, derived from [`MAX_IMAGE_BYTES`].
pub const MAX_IMAGE_BASE64_BYTES: usize = MAX_IMAGE_BYTES.div_ceil(3) * 4;
/// Maximum decoded pixel count accepted from the clipboard or by a host.
pub const MAX_IMAGE_PIXELS: u64 = 16_000_000;
/// Maximum bytes of an image's display name.
pub const MAX_IMAGE_NAME_BYTES: usize = 128;
