#[test]
fn diagnostics_explain_failures_without_echoing_private_provider_text() {
    for (raw, expected) in [
        (
            "the completion request failed: operation timed out https://private.invalid/?key=secret",
            "timed out",
        ),
        ("the provider answered HTTP 503: secret", "server failed"),
        (
            "openai (500): the provider's stream failed: secret",
            "server failed",
        ),
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
    let limit = "the provider answered HTTP 429: Request too large for gpt-4o on tokens per min";
    assert_eq!(super::Diagnostic::classify(limit).code, "provider_quota");
}

#[test]
fn native_provider_statuses_have_safe_actionable_explanations() {
    for (status, code) in [
        (400, "provider_request_rejected"),
        (401, "provider_access_denied"),
        (402, "provider_payment_required"),
        (403, "provider_access_denied"),
        (404, "provider_not_found"),
        (408, "provider_timeout"),
        (413, "provider_request_too_large"),
        (422, "provider_request_rejected"),
        (429, "provider_quota"),
        (500, "provider_server_error"),
        (503, "provider_server_error"),
        (529, "provider_overloaded"),
    ] {
        for raw in [
            format!("provider failed: anthropic ({status}): secret-key https://private.invalid"),
            format!("openai ({status}): secret-key https://private.invalid"),
            format!("the provider answered HTTP {status}: secret-key https://private.invalid"),
        ] {
            let diagnostic = super::Diagnostic::classify(&raw);
            assert_eq!(diagnostic.code, code, "{raw}");
            assert_eq!(diagnostic.http_status, Some(status), "{raw}");
            assert!(!diagnostic.summary.contains("secret-key"));
            assert!(!diagnostic.summary.contains("private.invalid"));
        }
    }
}

#[test]
fn native_context_and_request_details_keep_specific_remedies() {
    for (raw, code) in [
        (
            "anthropic (400): prompt is too long: private details",
            "context_limit",
        ),
        (
            "anthropic (429): request too large for tokens per minute",
            "provider_quota",
        ),
        (
            "anthropic (400): max_tokens exceeds the model's output token limit",
            "provider_output_limit",
        ),
        (
            "anthropic (400): input length + max_tokens exceed context window",
            "context_limit",
        ),
        (
            "anthropic (400): max_tokens must be less than 64000",
            "provider_output_limit",
        ),
        (
            "anthropic (400): thinking.budget_tokens must be less than max_tokens",
            "provider_output_limit",
        ),
        (
            "anthropic (400): output_config.effort: unsupported value",
            "provider_reasoning_rejected",
        ),
        (
            "anthropic (400): Invalid signature in thinking block",
            "provider_reasoning_replay",
        ),
        (
            "Your credit balance is too low to access the Anthropic API",
            "provider_payment_required",
        ),
        ("Anthropic API rate limit exceeded", "provider_quota"),
    ] {
        let diagnostic = super::Diagnostic::classify(raw);
        assert_eq!(diagnostic.code, code, "{raw}");
        assert!(!diagnostic.summary.contains("private details"));
    }
}

#[test]
fn only_complete_status_envelopes_are_read() {
    for error in [
        "HTTP 4290: private",
        "HTTP 429words",
        "anthropic (4290): private",
        "anthropic (429) private",
        "count (123): private",
        "xhttp 429: private",
    ] {
        assert_eq!(
            super::Diagnostic::classify(error).http_status,
            None,
            "{error}"
        );
    }
    let diagnostic =
        super::Diagnostic::classify("provider failed: anthropic (401): private mentions HTTP 429");
    assert_eq!(diagnostic.http_status, Some(401));
    assert_eq!(diagnostic.code, "provider_access_denied");
}

#[test]
fn an_empty_answer_has_a_recovery_message() {
    let diagnostic = super::Diagnostic::classify("model returned no visible answer");
    assert_eq!(diagnostic.code, "provider_empty_response");
    assert!(!diagnostic.exceeds_context());
}

/// A Responses stream can report this quota as an invalid request (400),
/// even though its specific code names the ChatGPT plan usage limit.
#[test]
fn a_chatgpt_usage_limit_explains_the_plan_or_app_limit() {
    for envelope in ["openai (400): ", "HTTP 429: ", ""] {
        let raw =
            format!("{envelope}subscription_sharing_usage_limit_exceeded: private account details");
        let diagnostic = super::Diagnostic::classify(&raw);
        assert_eq!(diagnostic.code, "provider_quota", "{raw}");
        assert_eq!(
            diagnostic.summary,
            "ChatGPT plan usage limit reached. Check your plan or this app's limit in ChatGPT Settings > Usage, then retry when usage is available or choose an API-key connection."
        );
        assert!(!diagnostic.exceeds_context());
        assert!(!diagnostic.summary.contains("private account details"));
    }
}

/// The fallback for a 400 no rule knows.
const REJECTED: &str =
    "Provider rejected the request format or parameters. Check model and tool-calling support.";

/// A refusal (HTTP 4xx) shows the words the provider gave it below the
/// summary: OpenCode Go refuses a model the organization's region setting
/// excludes, which no summary names (2026-11-12).
#[test]
fn a_provider_refusal_shows_the_provider_s_words() {
    let words = "This OpenCode Go model is hosted outside Europe and the United States. \
        Select Global at https://opencode.ai/console/org_01ABC/settings/privacy#regions to use it.";
    for raw in [
        format!("openai (400): the provider answered HTTP 400 (invalid_request_error): {words}"),
        format!("the provider answered HTTP 400: {words}"),
        format!("openai (400): the provider's stream failed: {words}"),
        format!("openai (400): the provider answered with an error: {words}"),
        format!("provider failed: anthropic (400): {words}"),
    ] {
        assert_eq!(
            super::explain(&raw),
            format!("{REJECTED}\nProvider says: {words}"),
            "{raw}"
        );
    }
    let told = super::explain("openai (403): the provider answered HTTP 403: Model not allowed");
    assert!(
        told.starts_with("Provider access denied.")
            && told.ends_with("\nProvider says: Model not allowed"),
        "{told}"
    );
}

/// A server failure, a status without the provider's words, or a failure
/// with no status shows the summary alone.
#[test]
fn only_a_worded_refusal_shows_the_provider_s_words() {
    for raw in [
        "the provider answered HTTP 503: private",
        "openai (500): the provider answered HTTP 500: private",
        "HTTP 400 maximum context length private",
        "HTTP 429: private",
        "the provider's stream failed: private",
        "openai (400): the provider answered HTTP 400:  ",
        "private failure",
    ] {
        let diagnostic = super::Diagnostic::classify(raw);
        assert_eq!(super::explain(raw), diagnostic.summary, "{raw}");
    }
}

/// The provider's words stay within 300 characters, and a token that
/// reads as a key is masked, the punctuation around it kept.
#[test]
fn the_provider_s_words_are_bounded_and_keep_no_key() {
    let key = "sk-proj-abcdef0123456789abcdef0123456789";
    let raw = format!(
        "openai (401): the provider answered HTTP 401 (invalid_request_error, invalid_api_key): \
         Incorrect API key provided: {key}. See https://platform.example/account/api-keys."
    );
    let told = super::explain(&raw);
    assert!(
        told.ends_with(
            "\nProvider says: Incorrect API key provided: [redacted]. \
             See https://platform.example/account/api-keys."
        ),
        "{told}"
    );

    let long = "word ".repeat(200);
    let told = super::explain(&format!("anthropic (400): {long}"));
    let (_, words) = told.split_once("\nProvider says: ").unwrap_or_default();
    assert!(
        words.chars().count() <= 300 && words.ends_with('…'),
        "{words}"
    );
    assert!(words.starts_with("word word"));
}

#[test]
fn a_chatgpt_usage_code_does_not_override_access_or_server_failures() {
    for (status, code) in [
        (401, "provider_access_denied"),
        (503, "provider_server_error"),
    ] {
        let raw = format!("openai ({status}): subscription_sharing_usage_limit_exceeded");
        assert_eq!(super::Diagnostic::classify(&raw).code, code);
    }
}
