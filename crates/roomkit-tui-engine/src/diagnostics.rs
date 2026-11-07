/// Classify known engine/provider failures without forwarding arbitrary error
/// bodies, URLs, prompts or credentials to a client.
/// A bounded, safe explanation that never contains the provider's raw payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Diagnostic {
    /// Stable classification for logs and support.
    pub code: &'static str,
    /// Human-readable explanation and recovery guidance.
    pub summary: &'static str,
}

/// Failures known by fragments: any listed fragment of the lowercase error
/// selects the entry.
type Known = &'static [(&'static [&'static str], &'static str, &'static str)];

/// Known failures read before a context refusal, most specific first.
const KNOWN: Known = &[
    (
        &["responses api only", "takes no function tools"],
        "provider_model_refused",
        "This model cannot serve this request on the configured API. Choose another model or use a connection that speaks its API.",
    ),
    (
        &["terminal text differs"],
        "responses_text_mismatch",
        "The Responses stream and its final text disagree. Check the provider adapter and stream diagnostics.",
    ),
    (
        &["without a terminal response"],
        "responses_missing_terminal",
        "The Responses stream ended before its completion event. Retry or check provider connectivity.",
    ),
    (
        &["unexpected namespace"],
        "responses_namespace_mismatch",
        "The Responses tool call names an unexpected namespace. Check the provider adapter and tool declarations.",
    ),
    (
        &["requires an encrypted reasoning item"],
        "responses_missing_reasoning",
        "The Responses provider omitted encrypted reasoning required to continue the tool conversation.",
    ),
    (
        &[
            "unfinished output items",
            "missing items",
            "completed items conflict",
        ],
        "responses_inconsistent_items",
        "The Responses stream contains incomplete or conflicting output items. No pending tool calls were executed.",
    ),
    (
        &["provider's answer is malformed"],
        "provider_malformed",
        "The provider returned an invalid response. Check protocol compatibility and host diagnostics.",
    ),
    (
        &["account must sign in", "not authorized model usage"],
        "account_sign_in_required",
        "Sign in to the provider again and authorize model usage.",
    ),
    (
        &["provider credentials are unavailable"],
        "credentials_unavailable",
        "Provider credentials could not be refreshed. Check the connection and account.",
    ),
    (
        &["process_timeout"],
        "engine_timeout",
        "The engine processing deadline expired. Check host limits and whether another operation is still running.",
    ),
    (
        &["timed out", "timeout"],
        "provider_timeout",
        "Model request timed out. Increase the provider timeout in your configuration, then retry.",
    ),
    (
        &["http 401", "http 403"],
        "provider_access_denied",
        "Provider access denied. Check the API key and endpoint permissions.",
    ),
    (
        &["http 429"],
        "provider_quota",
        "Provider rate limit or quota reached. Check your quota and retry later.",
    ),
];

/// A refusal past the model's context window: read after access and quota
/// failures, and before the generic rejection it usually comes with. Tools
/// often fill the window more than the conversation, so a new conversation
/// is not the only remedy.
const CONTEXT_LIMIT: (&str, &str) = (
    "context_limit",
    "The request exceeds the model's context limit. Disconnect tools, compact the conversation or start a new one, or use a model with more capacity.",
);

/// Known failures read after a context refusal, most specific first.
const AFTER_CONTEXT: Known = &[
    (
        &["http 400", "http 422"],
        "provider_request_rejected",
        "Provider rejected the request format or parameters. Check model and tool-calling support.",
    ),
    (
        &["http 404"],
        "provider_not_found",
        "Provider endpoint or model not found. Check the base URL and model identifier.",
    ),
    (
        &["http 5"],
        "provider_server_error",
        "Provider server failed. Check its health and logs, then retry.",
    ),
    (
        &["connect", "dns"],
        "provider_unreachable",
        "Cannot reach the model server. Check its address, network and availability.",
    ),
    (
        &["not a chat completion", "stream failed"],
        "provider_stream_error",
        "Provider returned an invalid or interrupted response. Check server logs and protocol compatibility.",
    ),
    (
        &["cancel", "kit_closed"],
        "cancelled",
        "Execution was cancelled or the engine shut down. Completed tool actions are retained.",
    ),
];

impl Diagnostic {
    /// Classify an engine error without copying arbitrary upstream text.
    pub fn classify(error: &str) -> Self {
        let lower = error.to_ascii_lowercase();
        let known = |table: Known| {
            table
                .iter()
                .find(|(fragments, ..)| fragments.iter().any(|f| lower.contains(f)))
                .map(|(_, code, summary)| (*code, *summary))
        };
        let (code, summary) = known(KNOWN)
            .or_else(|| reads_as_context_overflow(&lower).then_some(CONTEXT_LIMIT))
            .or_else(|| known(AFTER_CONTEXT))
            .unwrap_or((
                "engine_failed",
                "Engine execution failed. Check the host diagnostics; completed tool actions may remain.",
            ));
        Self { code, summary }
    }

    /// Whether the provider refused the request as past the model's context
    /// window.
    pub(crate) fn exceeds_context(&self) -> bool {
        self.code == CONTEXT_LIMIT.0
    }
}

/// Whether `lower` is a refusal past the model's context window as the kit
/// reads one, or the kit's own failure when its compaction cannot make room
/// ("Context too large but …").
fn reads_as_context_overflow(lower: &str) -> bool {
    lower.contains("context too large") || roomkit_ai::is_context_overflow_message(lower)
}

pub(crate) fn explain(error: &str) -> &'static str {
    Diagnostic::classify(error).summary
}

#[cfg(test)]
mod tests {
    #[test]
    fn diagnostics_explain_failures_without_echoing_private_provider_text() {
        for (raw, expected) in [
            (
                "the completion request failed: operation timed out https://private.invalid/?key=secret",
                "timed out",
            ),
            ("the provider answered HTTP 401: secret", "access denied"),
            ("the provider answered HTTP 429: secret", "quota"),
            ("HTTP 400 maximum context length secret", "context limit"),
            ("private failure secret", "Engine execution failed"),
            (
                "gpt-5.5-pro is served by OpenAI's Responses API only, not Chat Completions",
                "Choose another model",
            ),
            (
                "gpt-6.1-sol takes no function tools on Chat Completions",
                "Choose another model",
            ),
            (
                "Responses function call has an unexpected namespace secret",
                "unexpected namespace",
            ),
            (
                "stateless reasoning replay requires an encrypted reasoning item secret",
                "encrypted reasoning",
            ),
            (
                "Responses completed items conflict secret",
                "conflicting output items",
            ),
            (
                "the provider's answer is malformed: secret",
                "invalid response",
            ),
            (
                "Responses terminal text differs from its streamed text secret",
                "final text disagree",
            ),
            ("the provider account must sign in again secret", "Sign in"),
        ] {
            let rendered = super::explain(raw);
            assert!(rendered.contains(expected));
            assert!(!rendered.contains("secret") && !rendered.contains("private.invalid"));
        }
    }

    /// The kit's own failure when compaction cannot make room, and the
    /// wordings it reads as a context refusal, name the context limit, with
    /// the tools among the remedies.
    #[test]
    fn a_context_refusal_reads_as_the_kit_reads_it() {
        for raw in [
            "Context too large but cannot compact further (<=4 messages)",
            "Context too large but nothing left to compact before the turn's input",
            "the provider answered HTTP 400: prompt is too long: 210000 tokens > 200000 maximum",
            "the provider answered HTTP 400: This model's maximum context length is 131072 tokens",
        ] {
            let diagnostic = super::Diagnostic::classify(raw);
            assert!(diagnostic.exceeds_context(), "{raw}");
            assert!(diagnostic.summary.contains("Disconnect tools"));
            assert!(!diagnostic.summary.contains("Start a new session"));
        }
        // A rate limit on tokens per minute stays a quota, worth a later retry.
        let limit =
            "the provider answered HTTP 429: Request too large for gpt-4o on tokens per min";
        assert_eq!(super::Diagnostic::classify(limit).code, "provider_quota");
    }
}
