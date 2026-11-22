//! Records with the attributes RoomKit writes at the provider boundary.
use roomkit::model::ErrorRecord;
use roomkit_ai::{ProviderError, ProviderErrorOptions};
use serde_json::json;

use super::super::{Diagnostic, explain};

fn record(message: &str, status: Option<u16>, retryable: bool) -> ErrorRecord {
    let error = ProviderError::new(
        message,
        ProviderErrorOptions {
            provider: "anthropic".into(),
            status_code: status,
            retryable,
            ..Default::default()
        },
    );
    let mut record = ErrorRecord::new("ProviderError", error.to_string());
    record.attributes.extend([
        ("provider".into(), json!(error.provider)),
        ("status_code".into(), json!(error.status_code)),
        ("retryable".into(), json!(error.retryable)),
        ("context_overflow".into(), json!(error.context_overflow)),
        ("before_request".into(), json!(error.before_request)),
    ]);
    record
}

fn check(record: &ErrorRecord, code: &str, summary: &str) {
    let diagnostic = Diagnostic::from_record(record);
    assert_eq!(diagnostic.code, code);
    assert_eq!(diagnostic.summary, summary);
    assert_eq!(explain(record), summary);
}

#[test]
fn a_transport_timeout_explains_the_deadline() {
    check(
        &record("Request timed out.", None, true),
        "provider_timeout",
        "Model request timed out. Increase the provider timeout in your configuration, then retry.",
    );
}

#[test]
fn an_unreadable_body_is_an_invalid_response() {
    check(
        &record(
            "The provider answered with a body it could not read: missing content",
            None,
            false,
        ),
        "provider_malformed",
        "The provider returned an invalid response. Check protocol compatibility and host diagnostics.",
    );
}

#[test]
fn an_unclassified_stream_failure_uses_the_providers_retry_guidance() {
    for (retryable, summary) in [
        (
            false,
            "The provider request failed. Check the host diagnostics; completed tool actions may remain.",
        ),
        (
            true,
            "The provider request failed. Retry the request; completed tool actions may remain.",
        ),
    ] {
        // The stream's arbitrary text cannot establish a status or be echoed.
        let failed = record(
            "worker stopped after HTTP 403: private detail",
            None,
            retryable,
        );
        check(&failed, "provider_failed", summary);
        assert_eq!(Diagnostic::from_record(&failed).http_status, None);
    }
}

#[test]
fn the_recorded_status_identifies_a_refusal_without_a_display_envelope() {
    let refused = record(
        "Anthropic declined this request after HTTP 503: private detail",
        Some(403),
        false,
    );
    check(
        &refused,
        "provider_access_denied",
        "Provider access denied. Check the API key and endpoint permissions.",
    );
    assert_eq!(Diagnostic::from_record(&refused).http_status, Some(403));
}

#[test]
fn an_explicit_context_decision_supersedes_message_fragments() {
    let mut refused = record(
        "Anthropic declined the max_tokens setting",
        Some(400),
        false,
    );
    refused
        .attributes
        .insert("context_overflow".into(), true.into());
    assert!(Diagnostic::from_record(&refused).exceeds_context());
    refused.message = "Anthropic mentioned the context window".into();
    refused
        .attributes
        .insert("context_overflow".into(), false.into());
    assert_eq!(
        Diagnostic::from_record(&refused).code,
        "provider_request_rejected"
    );
}

#[test]
fn legacy_records_retain_their_text_diagnostics() {
    let mut legacy = ErrorRecord::new("ProviderError", "anthropic (401): retry later");
    let expected = Diagnostic::classify(&legacy.message);
    assert_eq!(Diagnostic::from_record(&legacy), expected);
    legacy
        .attributes
        .insert("provider".into(), "anthropic".into());
    assert_eq!(Diagnostic::from_record(&legacy), expected);
}

#[test]
fn words_come_only_from_the_envelope_that_identified_the_refusal() {
    let summary = "Provider access denied. Check the API key and endpoint permissions.";
    for message in [
        "HTTP 403: upstream chain; the provider answered HTTP 503: private detail",
        "openai (403): the provider answered HTTP 503: private detail",
        "the provider answered HTTP 403 without a delimiter, then HTTP 503: private detail",
    ] {
        let mut refused = record("", Some(403), false);
        refused.message = message.into();
        check(&refused, "provider_access_denied", summary);
    }
}

#[test]
fn a_provider_class_keeps_its_attributes_without_an_adapter_name()
-> Result<(), roomkit::model::ModelError> {
    for (class, status, expected) in [
        ("ProviderError", 403, "provider_access_denied"),
        (
            "roomkit.providers.ai.base:ProviderError",
            503,
            "provider_server_error",
        ),
    ] {
        let mut refused = ErrorRecord::new(class, "request failed");
        refused
            .attributes
            .insert("status_code".into(), status.into());
        let wrapped = roomkit::RoomKitError::Channel {
            channel_id: roomkit::model::ChannelId::new("test")?,
            error: refused.clone(),
        };
        assert_eq!(super::super::record(&wrapped), refused);
        let diagnostic = Diagnostic::from_record(&refused);
        assert_eq!(diagnostic.http_status, Some(status));
        assert_eq!(diagnostic.code, expected);
    }

    let mut foreign = ErrorRecord::new("other:ProviderError", "request failed");
    foreign.attributes.insert("status_code".into(), 503.into());
    assert_eq!(Diagnostic::from_record(&foreign).code, "engine_failed");

    let mut opaque = ErrorRecord::new("ProviderError", "openai (403): private detail");
    opaque
        .attributes
        .insert("status_code".into(), serde_json::Value::Null);
    check(
        &opaque,
        "provider_failed",
        "The provider request failed. Check the host diagnostics; completed tool actions may remain.",
    );
    Ok(())
}
