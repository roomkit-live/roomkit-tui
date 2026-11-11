mod provider;

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
    /// HTTP failure status read from a complete provider error envelope, if present.
    pub http_status: Option<u16>,
}

/// Failures known by fragments: any listed fragment of the lowercase error
/// selects the entry.
type Known = &'static [(&'static [&'static str], &'static str, &'static str)];

/// Known failures read before a context refusal, most specific first.
const KNOWN: Known = &[
    (
        &["model returned no visible answer"],
        "provider_empty_response",
        "The model returned no visible answer. Retry, or adjust the output limit and reasoning effort.",
    ),
    (
        &[
            "credit balance is too low",
            "insufficient credit",
            "credit balance is insufficient",
        ],
        provider::PAYMENT.0,
        provider::PAYMENT.1,
    ),
    (
        &[
            "rate limit exceeded",
            "rate_limit_error",
            "too many requests",
        ],
        provider::QUOTA.0,
        provider::QUOTA.1,
    ),
    (
        &[
            "overloaded_error",
            "provider is overloaded",
            "api is overloaded",
        ],
        provider::OVERLOADED.0,
        provider::OVERLOADED.1,
    ),
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
        let http_status = provider::status(&lower);
        let known = |table: Known| {
            table
                .iter()
                .find(|(fragments, ..)| fragments.iter().any(|f| lower.contains(f)))
                .map(|(_, code, summary)| (*code, *summary))
        };
        let (code, summary) = provider::usage_limit(&lower, http_status)
            .or_else(|| provider::priority(http_status))
            .or_else(|| known(KNOWN))
            .or_else(|| provider::request(&lower, http_status))
            .or_else(|| reads_as_context_overflow(&lower).then_some(CONTEXT_LIMIT))
            .or_else(|| provider::fallback(http_status))
            .or_else(|| known(AFTER_CONTEXT))
            .unwrap_or((
                "engine_failed",
                "Engine execution failed. Check the host diagnostics; completed tool actions may remain.",
            ));
        Self {
            code,
            summary,
            http_status,
        }
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
mod tests;
