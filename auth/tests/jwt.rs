//! Integration tests for JWT encoding, decoding, and related helpers.

use nasiko_auth::{
    AuthError, AuthService, Identity, SimpleJwtAuth,
    jwt::{DEFAULT_EXPIRY_SECS, decode_jwt, encode_jwt, extract_jti, hash_jti},
};

fn make_identity(user_id: &str) -> Identity {
    Identity {
        user_id: user_id.to_owned(),
        username: "testuser".to_owned(),
        is_superuser: false,
    }
}

// ─── encode_jwt / decode_jwt roundtrip ───────────────────────────────────────

#[test]
fn jwt_roundtrip_preserves_user_id() {
    let id = make_identity("aaaaaaaa-0000-0000-0000-000000000001");
    let token = encode_jwt("secret", DEFAULT_EXPIRY_SECS, &id).unwrap();
    let decoded = decode_jwt("secret", &token).unwrap();
    assert_eq!(decoded.user_id, id.user_id);
}

#[test]
fn jwt_roundtrip_preserves_username() {
    let id = make_identity("aaaaaaaa-0000-0000-0000-000000000002");
    let token = encode_jwt("secret", DEFAULT_EXPIRY_SECS, &id).unwrap();
    let decoded = decode_jwt("secret", &token).unwrap();
    assert_eq!(decoded.username, id.username);
}

#[test]
fn jwt_roundtrip_preserves_superuser_false() {
    let id = make_identity("aaaaaaaa-0000-0000-0000-000000000003");
    let token = encode_jwt("secret", DEFAULT_EXPIRY_SECS, &id).unwrap();
    let decoded = decode_jwt("secret", &token).unwrap();
    assert!(!decoded.is_superuser);
}

#[test]
fn jwt_roundtrip_preserves_superuser_true() {
    let mut id = make_identity("aaaaaaaa-0000-0000-0000-000000000004");
    id.is_superuser = true;
    let token = encode_jwt("supersecret", DEFAULT_EXPIRY_SECS, &id).unwrap();
    let decoded = decode_jwt("supersecret", &token).unwrap();
    assert!(decoded.is_superuser);
}

#[test]
fn jwt_identity_has_no_enterprise_fields() {
    // The shared Identity must never carry role/team_id/department_id.
    let id = make_identity("aaaaaaaa-0000-0000-0000-000000000006");
    let token = encode_jwt("s", DEFAULT_EXPIRY_SECS, &id).unwrap();
    let decoded = decode_jwt("s", &token).unwrap();
    let json = serde_json::to_value(&decoded).unwrap();
    assert!(json.get("role").is_none(), "Identity must not expose role");
    assert!(json.get("team_id").is_none(), "Identity must not expose team_id");
    assert!(json.get("department_id").is_none(), "Identity must not expose department_id");
}

// ─── Wrong secret ─────────────────────────────────────────────────────────────

#[test]
fn jwt_wrong_secret_is_rejected() {
    let id = make_identity("aaaaaaaa-0000-0000-0000-000000000010");
    let token = encode_jwt("correct-secret", DEFAULT_EXPIRY_SECS, &id).unwrap();
    let result = decode_jwt("wrong-secret", &token);
    assert!(result.is_err(), "token signed with a different secret must be rejected");
}

#[test]
fn jwt_empty_secret_differs_from_nonempty_secret() {
    let id = make_identity("aaaaaaaa-0000-0000-0000-000000000011");
    let token = encode_jwt("nonempty", DEFAULT_EXPIRY_SECS, &id).unwrap();
    assert!(decode_jwt("", &token).is_err());
}

// ─── Expired token ────────────────────────────────────────────────────────────

#[test]
fn jwt_expired_token_returns_expired_error() {
    use chrono::Utc;
    use jsonwebtoken::{EncodingKey, Header, encode as jwt_encode};
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    struct MinClaims {
        sub: String,
        #[serde(default)]
        jti: String,
        exp: u64,
        iat: u64,
        username: String,
        is_superuser: bool,
    }

    let now = Utc::now().timestamp() as u64;
    let claims = MinClaims {
        sub: "aaaaaaaa-0000-0000-0000-000000000020".into(),
        jti: uuid::Uuid::new_v4().to_string(),
        exp: now.saturating_sub(100),
        iat: now.saturating_sub(200),
        username: "expired_user".into(),
        is_superuser: false,
    };
    let token = jwt_encode(&Header::default(), &claims, &EncodingKey::from_secret(b"secret")).unwrap();
    let result = decode_jwt("secret", &token);
    assert!(matches!(result, Err(AuthError::Expired)), "expected Expired, got: {:?}", result);
}

// ─── extract_jti ─────────────────────────────────────────────────────────────

#[test]
fn extract_jti_returns_some_for_valid_jwt() {
    let id = make_identity("aaaaaaaa-0000-0000-0000-000000000040");
    let token = encode_jwt("secret", DEFAULT_EXPIRY_SECS, &id).unwrap();
    assert!(extract_jti(&token).is_some(), "JTI must be present in a freshly issued token");
}

#[test]
fn extract_jti_value_is_a_valid_uuid() {
    let id = make_identity("aaaaaaaa-0000-0000-0000-000000000041");
    let token = encode_jwt("secret", DEFAULT_EXPIRY_SECS, &id).unwrap();
    let jti = extract_jti(&token).unwrap();
    assert!(uuid::Uuid::parse_str(&jti).is_ok(), "JTI should be a valid UUID, got: {jti}");
}

#[test]
fn extract_jti_returns_none_for_garbage_string() {
    assert!(extract_jti("not-a-jwt").is_none());
}

#[test]
fn extract_jti_returns_none_for_single_segment() {
    assert!(extract_jti("onlyone").is_none());
}

#[test]
fn extract_jti_returns_none_for_invalid_base64_payload() {
    assert!(extract_jti("header.!!INVALID!!.sig").is_none());
}

#[test]
fn extract_jti_is_unique_across_tokens_for_same_identity() {
    let id = make_identity("aaaaaaaa-0000-0000-0000-000000000042");
    let t1 = encode_jwt("s", DEFAULT_EXPIRY_SECS, &id).unwrap();
    let t2 = encode_jwt("s", DEFAULT_EXPIRY_SECS, &id).unwrap();
    assert_ne!(extract_jti(&t1), extract_jti(&t2), "each token must have a unique JTI");
}

// ─── hash_jti ────────────────────────────────────────────────────────────────

#[test]
fn hash_jti_is_deterministic() {
    assert_eq!(hash_jti("my-jti-value"), hash_jti("my-jti-value"));
}

#[test]
fn hash_jti_different_inputs_give_different_hashes() {
    assert_ne!(hash_jti("jti-a"), hash_jti("jti-b"));
}

#[test]
fn hash_jti_output_is_64_hex_chars() {
    let h = hash_jti("any-jti-string");
    assert_eq!(h.len(), 64, "SHA-256 hex must be exactly 64 chars");
    assert!(h.chars().all(|c| c.is_ascii_hexdigit()), "hash must be lowercase hex");
}

#[test]
fn hash_jti_empty_string_produces_valid_hash() {
    let h = hash_jti("");
    assert_eq!(h.len(), 64);
    assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
}

// ─── SimpleJwtAuth (AuthService trait) ───────────────────────────────────────

#[tokio::test]
async fn simple_jwt_auth_issue_and_validate_roundtrip() {
    let auth = SimpleJwtAuth { secret: "roundtrip-secret".into(), expiry_secs: DEFAULT_EXPIRY_SECS };
    let id = make_identity("aaaaaaaa-0000-0000-0000-000000000050");
    let token = auth.issue_token(&id).await.unwrap();
    let decoded = auth.validate_token(&token).await.unwrap();
    assert_eq!(decoded.user_id, id.user_id);
    assert_eq!(decoded.username, id.username);
}

#[tokio::test]
async fn simple_jwt_auth_rejects_garbage_token() {
    let auth = SimpleJwtAuth { secret: "s".into(), expiry_secs: DEFAULT_EXPIRY_SECS };
    assert!(auth.validate_token("not-a-jwt").await.is_err());
}

#[tokio::test]
async fn simple_jwt_auth_rejects_token_from_different_secret() {
    let signer = SimpleJwtAuth { secret: "correct".into(), expiry_secs: DEFAULT_EXPIRY_SECS };
    let verifier = SimpleJwtAuth { secret: "wrong".into(), expiry_secs: DEFAULT_EXPIRY_SECS };
    let id = make_identity("aaaaaaaa-0000-0000-0000-000000000051");
    let token = signer.issue_token(&id).await.unwrap();
    assert!(verifier.validate_token(&token).await.is_err());
}

#[tokio::test]
async fn simple_jwt_auth_can_access_agent_always_true() {
    let auth = SimpleJwtAuth { secret: "s".into(), expiry_secs: DEFAULT_EXPIRY_SECS };
    let id = make_identity("aaaaaaaa-0000-0000-0000-000000000052");
    assert!(auth.can_access_agent(&id, "any-agent-id").await);
}

// ─── Token revocation (requires DB — ignored) ────────────────────────────────

#[tokio::test]
#[ignore = "requires live Postgres database"]
async fn auth_service_impl_revoke_tokens_for_user_requires_db() {
    todo!("wire up test PgPool and AuthServiceImpl")
}

#[tokio::test]
#[ignore = "requires live Postgres database"]
async fn auth_service_impl_revoke_all_tokens_requires_db() {
    todo!("wire up test PgPool and AuthServiceImpl")
}
