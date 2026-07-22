//! Composio webhook handling.
//!
//! Verifies the inbound HMAC signature and processes
//! `composio.connected_account.expired` events: mark the matching connection
//! `EXPIRED` and invalidate the user's cached session so the next resolve
//! re-syncs (dropping the dead toolkit). Simpler than the PoC's per-session
//! patch loop because our sessions are per-user and resolved on demand.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;

use crate::error::Result;
use crate::repo;
use crate::session;
use crate::state::McpState;

type HmacSha256 = Hmac<Sha256>;

/// The event type we act on.
const EXPIRED_EVENT: &str = "composio.connected_account.expired";

/// Verify a Composio webhook signature (HMAC-SHA256, base64).
///
/// `signing_string = "{webhook_id}.{webhook_timestamp}.{raw_body}"`. The
/// signature header may carry a `v1,` scheme prefix, which is stripped before
/// comparison. Comparison is constant-time.
pub fn verify_signature(
    webhook_id: &str,
    webhook_timestamp: &str,
    body: &str,
    signature: &str,
    secret: &str,
) -> bool {
    let signing_string = format!("{webhook_id}.{webhook_timestamp}.{body}");
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(signing_string.as_bytes());

    // Header may be "v1,<sig>" — take the part after the last comma.
    let received = signature.rsplit(',').next().unwrap_or(signature);
    let Ok(received_bytes) = B64.decode(received) else {
        return false;
    };
    mac.verify_slice(&received_bytes).is_ok()
}

/// Outcome of processing a webhook payload (for the route to log / respond).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookOutcome {
    /// An event type we don't handle — acknowledged, no action.
    Ignored,
    /// Expiry event for an account we don't have (already deleted/unknown).
    UnknownAccount,
    /// Connection was already EXPIRED — no-op.
    AlreadyExpired,
    /// Connection marked EXPIRED and the user's session cache invalidated.
    Expired,
}

/// Process a parsed webhook payload. Signature verification is the route's
/// responsibility (it has the raw body + headers); this handles the effect.
pub async fn process_event(state: &McpState, payload: &Value) -> Result<WebhookOutcome> {
    let event_type = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if event_type != EXPIRED_EVENT {
        tracing::debug!(event_type, "ignoring composio webhook event");
        return Ok(WebhookOutcome::Ignored);
    }

    let data = payload.get("data");
    let Some(account_id) = data.and_then(|d| d.get("id")).and_then(|v| v.as_str()) else {
        tracing::warn!("composio expiry webhook missing data.id — ignoring");
        return Ok(WebhookOutcome::UnknownAccount);
    };

    let Some(connection) = repo::get_connection_by_account_id(&state.db, account_id).await? else {
        tracing::warn!(
            account_id,
            "expiry webhook for unknown connection — already deleted or unknown"
        );
        return Ok(WebhookOutcome::UnknownAccount);
    };

    if connection.status == "EXPIRED" {
        return Ok(WebhookOutcome::AlreadyExpired);
    }

    repo::update_connection_status(&state.db, connection.id, "EXPIRED").await?;
    session::invalidate_session_cache(state, connection.user_id).await;

    tracing::warn!(
        account_id,
        user_id = %connection.user_id,
        connector_id = %connection.connector_id,
        "composio connection expired — marked EXPIRED and invalidated session cache",
    );
    Ok(WebhookOutcome::Expired)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_verifies_and_rejects() {
        let secret = "whsec_test";
        let (id, ts, body) = ("wh_1", "1700000000", r#"{"type":"x"}"#);
        let signing = format!("{id}.{ts}.{body}");
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(signing.as_bytes());
        let sig = B64.encode(mac.finalize().into_bytes());

        assert!(verify_signature(id, ts, body, &sig, secret));
        assert!(verify_signature(id, ts, body, &format!("v1,{sig}"), secret));
        assert!(!verify_signature(id, ts, body, &sig, "wrong-secret"));
        assert!(!verify_signature(id, ts, "tampered", &sig, secret));
    }

    fn sign(id: &str, ts: &str, body: &str, secret: &str) -> String {
        let signing = format!("{id}.{ts}.{body}");
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(signing.as_bytes());
        B64.encode(mac.finalize().into_bytes())
    }

    #[test]
    fn semantically_equivalent_but_differently_serialized_body_fails() {
        // Proves this verifies the RAW body bytes, not a re-parsed/normalized
        // JSON value — two JSON documents that are semantically identical but
        // differ in key order/whitespace must NOT be interchangeable.
        let secret = "whsec_test";
        let (id, ts) = ("wh_1", "1700000000");
        let body_a = r#"{"type":"x","data":{"id":"1"}}"#;
        let body_b = r#"{"data": {"id": "1"}, "type": "x"}"#; // same meaning, different bytes
        let sig_over_a = sign(id, ts, body_a, secret);

        assert!(verify_signature(id, ts, body_a, &sig_over_a, secret));
        assert!(!verify_signature(id, ts, body_b, &sig_over_a, secret));
    }

    #[test]
    fn truncated_and_extended_signature_are_rejected() {
        let secret = "whsec_test";
        let (id, ts, body) = ("wh_1", "1700000000", r#"{"type":"x"}"#);
        let sig = sign(id, ts, body, secret);

        let truncated = &sig[..sig.len() - 4];
        assert!(!verify_signature(id, ts, body, truncated, secret));

        let extended = format!("{sig}AAAA");
        assert!(!verify_signature(id, ts, body, &extended, secret));
    }

    #[test]
    fn empty_body_with_correct_signature_passes_wrong_signature_fails() {
        let secret = "whsec_test";
        let (id, ts, body) = ("wh_1", "1700000000", "");
        let sig_over_empty = sign(id, ts, body, secret);
        assert!(verify_signature(id, ts, body, &sig_over_empty, secret));

        // A signature computed over a non-empty body must not validate an
        // empty body.
        let sig_over_nonempty = sign(id, ts, "not empty", secret);
        assert!(!verify_signature(id, ts, body, &sig_over_nonempty, secret));
    }

    #[test]
    fn empty_signature_string_is_rejected_without_panicking() {
        let secret = "whsec_test";
        assert!(!verify_signature("wh_1", "1700000000", "{}", "", secret));
        // Malformed non-base64 signature must also fail cleanly, not panic.
        assert!(!verify_signature(
            "wh_1",
            "1700000000",
            "{}",
            "not-base64!!",
            secret
        ));
    }
}
