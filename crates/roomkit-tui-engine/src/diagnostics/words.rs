//! The words a provider refused a request with, as a view shows them.
//!
//! A refusal (HTTP 4xx) often says what no summary can: OpenCode Go refuses
//! a model the organization's region setting excludes, and says where to
//! change it. The adapters pass the provider's message on one line, within
//! 500 characters, their API key removed; a view gets at most
//! [`WORDS_MAX`] characters of it, any other key it holds masked.

/// How many characters of the provider's words a view shows.
const WORDS_MAX: usize = 300;

/// How the OpenAI adapter words a failure before the provider's message.
const OPENAI_STATUS: &str = "the provider answered HTTP ";
const OPENAI_WORDED: [&str; 2] = [
    "the provider's stream failed: ",
    "the provider answered with an error: ",
];

/// The providers the kit's adapters name in a `name (status): ` envelope.
const ADAPTERS: [&str; 2] = ["anthropic", "openai"];

/// The shortest part of a key, between `-` and `_`, read as random: a
/// model's ID has shorter ones (`claude-3-5-sonnet-20241022`).
const KEY_PART: usize = 16;

/// The words the provider refused the request with, when `status` is a
/// refusal (4xx) and `error` carries them.
pub(super) fn says(error: &str, status: Option<u16>) -> Option<String> {
    let status = status.filter(|status| (400..500).contains(status))?;
    let start = words_start(error, status)?;
    shown(&error[start..])
}

/// Where the provider's words start in `error`: after the OpenAI adapter's
/// wording, else after the `name (status): ` envelope an adapter of the kit
/// leads them with. Another failure worded alike is not a provider's.
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
        ADAPTERS.iter().find_map(|name| {
            let envelope = format!("{name} ({status}): ");
            error.find(&envelope).map(|at| at + envelope.len())
        })
    })
}

/// `words` on one line, any key they hold masked, cut at
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

/// `token` with each key it holds as `[redacted]`. A link's path stays as
/// it is, since it says where to go; its query may hold a key.
fn masked(token: &str) -> String {
    let (path, rest) = match token.find("://") {
        Some(_) => token.split_at(token.find('?').unwrap_or(token.len())),
        None => ("", token),
    };
    let mut shown = String::from(path);
    let mut run = String::new();
    for c in rest.chars() {
        if is_key_char(c) {
            run.push(c);
            continue;
        }
        shown.push_str(run_shown(&run));
        run.clear();
        shown.push(c);
    }
    shown.push_str(run_shown(&run));
    shown
}

/// A run of letters, digits, `-` and `_`, or `[redacted]` when one of its
/// parts between `-` and `_` reads as random: at least [`KEY_PART`]
/// characters, with a letter and a digit.
fn run_shown(run: &str) -> &str {
    let random = run.split(['-', '_']).any(|part| {
        part.len() >= KEY_PART
            && part.chars().any(|c| c.is_ascii_digit())
            && part.chars().any(|c| c.is_ascii_alphabetic())
    });
    if random { "[redacted]" } else { run }
}

fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}
