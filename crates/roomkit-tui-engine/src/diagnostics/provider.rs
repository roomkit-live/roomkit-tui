//! Read status envelopes without exposing a provider's arbitrary error body.
type Description = (&'static str, &'static str);

/// Both the HTTP adapters and `ProviderError::Display` reach the view as text.
/// Read the first complete envelope, never a number embedded in a longer one.
pub(super) fn status(error: &str) -> Option<u16> {
    let http = error.match_indices("http ").filter_map(|(at, tail)| {
        if at > 0 && error.as_bytes()[at - 1].is_ascii_alphanumeric() {
            return None;
        }
        let value = error.get(at + tail.len()..)?;
        let (status, rest) = code(value)?;
        (rest.is_empty() || rest.starts_with(|c: char| c.is_ascii_whitespace() || c == ':'))
            .then_some((at, status))
    });
    let native = error.match_indices(" (").filter_map(|(at, _)| {
        let provider = error
            .get(..at)?
            .rsplit(|c: char| c.is_ascii_whitespace() || c == ':')
            .next()?;
        if !provider.starts_with(|c: char| c.is_ascii_alphabetic())
            || !provider
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return None;
        }
        let (status, rest) = code(error.get(at + 2..)?)?;
        rest.starts_with("):")
            .then_some((at - provider.len(), status))
    });
    http.chain(native)
        .min_by_key(|(at, _)| *at)
        .map(|(_, status)| status)
}

fn code(text: &str) -> Option<(u16, &str)> {
    let digits = text.get(..3)?;
    if !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let status = digits.parse().ok()?;
    (400..=599)
        .contains(&status)
        .then_some((status, text.get(3..)?))
}

/// Access, quota and service failures take precedence over request wording.
pub(super) fn priority(status: Option<u16>) -> Option<Description> {
    Some(match status? {
        401 | 403 => (
            "provider_access_denied",
            "Provider access denied. Check the API key and endpoint permissions.",
        ),
        402 => PAYMENT,
        408 | 504 => (
            "provider_timeout",
            "Model request timed out. Check provider availability and the configured timeout, then retry.",
        ),
        429 => QUOTA,
        529 => OVERLOADED,
        500..=599 => (
            "provider_server_error",
            "Provider server failed. Check its health and logs, then retry.",
        ),
        _ => return None,
    })
}

pub(super) const PAYMENT: Description = (
    "provider_payment_required",
    "Provider credit or billing is insufficient. Check the account balance and billing settings, then retry.",
);
pub(super) const QUOTA: Description = (
    "provider_quota",
    "Provider rate limit or quota reached. Check your quota and retry later.",
);
pub(super) const OVERLOADED: Description = (
    "provider_overloaded",
    "The provider is overloaded. Wait briefly, then retry; completed tool actions are retained.",
);

/// Specific parameter failures are useful even when the vendor's message
/// names itself and `ProviderError::Display` consequently omits its status.
pub(super) fn request(error: &str, status: Option<u16>) -> Option<Description> {
    if status.is_some_and(|s| !matches!(s, 400 | 422)) {
        return None;
    }
    if [
        "context window",
        "context length",
        "input length",
        "prompt is too long",
    ]
    .iter()
    .any(|fragment| error.contains(fragment))
    {
        return None;
    }
    if error.contains("max_tokens") || error.contains("max_completion_tokens") {
        return Some((
            "provider_output_limit",
            "Provider rejected the output token limit. Check the model's maximum output and whether the limit leaves room for reasoning.",
        ));
    }
    if error.contains("signature") && (error.contains("thinking") || error.contains("reasoning")) {
        return Some((
            "provider_reasoning_replay",
            "Provider rejected the saved reasoning data. Start a new conversation and report the failure if it repeats; existing work is retained.",
        ));
    }
    if error.contains("output_config.effort")
        || error.contains("thinking.budget_tokens")
        || error.contains("reasoning_effort")
    {
        return Some((
            "provider_reasoning_rejected",
            "Provider rejected the reasoning settings. Choose an effort supported by this model or use its default.",
        ));
    }
    None
}

pub(super) fn fallback(status: Option<u16>) -> Option<Description> {
    Some(match status? {
        404 => (
            "provider_not_found",
            "Provider endpoint or model not found. Check the base URL and model identifier.",
        ),
        413 => (
            "provider_request_too_large",
            "The provider refused a request that is too large. Reduce attachments or tool output, compact the conversation, then retry.",
        ),
        400..=499 => (
            "provider_request_rejected",
            "Provider rejected the request format or parameters. Check model and tool-calling support.",
        ),
        _ => return None,
    })
}
