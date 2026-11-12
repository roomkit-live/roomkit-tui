//! The words a provider refused a request with, as a view shows them.
//!
//! A refusal (HTTP 4xx) often says what no summary can: OpenCode Go refuses
//! a model the organization's region setting excludes, and says where to
//! change it. The adapters pass the provider's message on one line, within
//! 500 characters, their API key removed; a view gets at most
//! [`WORDS_MAX`] characters of it, any other token that reads as a key
//! masked.

/// How many characters of the provider's words a view shows.
const WORDS_MAX: usize = 300;

/// How the OpenAI adapter words a failure before the provider's message.
const OPENAI_STATUS: &str = "the provider answered HTTP ";
const OPENAI_WORDED: [&str; 2] = [
    "the provider's stream failed: ",
    "the provider answered with an error: ",
];

/// The shortest run of key characters masked as a key.
const KEY_MIN: usize = 24;

/// The words the provider refused the request with, when `status` is a
/// refusal (4xx) and `error` carries them.
pub(super) fn says(error: &str, status: Option<u16>) -> Option<String> {
    let status = status.filter(|status| (400..500).contains(status))?;
    let start = words_start(error, status)?;
    shown(&error[start..])
}

/// Where the provider's words start in `error`: after the OpenAI adapter's
/// wording, else after the `name (status): ` envelope the other adapters
/// lead them with.
fn words_start(error: &str, status: u16) -> Option<usize> {
    if let Some(at) = error.find(OPENAI_STATUS) {
        // `HTTP 400 (type, code): ` names no colon before its own.
        return error[at..].find(": ").map(|colon| at + colon + 2);
    }
    let worded = OPENAI_WORDED
        .iter()
        .filter_map(|wording| error.find(wording).map(|at| at + wording.len()))
        .min();
    worded.or_else(|| {
        let envelope = format!("({status}): ");
        error.find(&envelope).map(|at| at + envelope.len())
    })
}

/// `words` on one line, a token that reads as a key masked, cut at
/// [`WORDS_MAX`] characters; nothing for blank words.
fn shown(words: &str) -> Option<String> {
    let text = words
        .split_whitespace()
        .map(masked)
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        return None;
    }
    if text.chars().count() <= WORDS_MAX {
        return Some(text);
    }
    let kept: String = text.chars().take(WORDS_MAX - 1).collect();
    Some(format!("{}…", kept.trim_end()))
}

/// `token`, or `[redacted]` in place of a key it holds: a run of at least
/// [`KEY_MIN`] letters, digits, `-` and `_`, with a letter and a digit. The
/// punctuation around it stays; a URL, which holds `/`, is no key.
fn masked(token: &str) -> String {
    let core = token.trim_matches(|c: char| !is_key_char(c));
    let key = core.len() >= KEY_MIN
        && core.chars().all(is_key_char)
        && core.chars().any(|c| c.is_ascii_digit())
        && core.chars().any(|c| c.is_ascii_alphabetic());
    if key {
        token.replacen(core, "[redacted]", 1)
    } else {
        token.to_owned()
    }
}

fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}
