//! Unit tests for nasiko-auth — no database or runtime required.

use crate::{
    AuthProvider, Identity, Role, SimpleJwtAuth, SingleUserAuth,
    jwt::{DEFAULT_EXPIRY_SECS, JwtClaims, decode_jwt, encode_jwt, extract_jti, hash_jti},
};

fn test_identity(user_id: &str) -> Identity {
    Identity {
        user_id: user_id.to_owned(),
        sub: user_id.to_owned(),
        username: "testuser".to_owned(),
        is_superuser: false,
        role: Some(Role::TeamMember),
        team_id: None,
        department_id: None,
        exp: 0,
        iat: 0,
    }
}

// ── jwt encode / decode ──────────────────────────────────────────────────────

#[test]
fn jwt_roundtrip_preserves_all_fields() {
    let id = test_identity("aaaaaaaa-0000-0000-0000-000000000001");
    let token = encode_jwt("secret", DEFAULT_EXPIRY_SECS, &id).unwrap();
    let decoded = decode_jwt("secret", &token).unwrap();

    assert_eq!(decoded.user_id, id.user_id);
    assert_eq!(decoded.username, id.username);
    assert_eq!(decoded.is_superuser, id.is_superuser);
    assert_eq!(decoded.role, id.role);
}

#[test]
fn jwt_wrong_secret_is_rejected() {
    let id = test_identity("aaaaaaaa-0000-0000-0000-000000000002");
    let token = encode_jwt("correct-secret", DEFAULT_EXPIRY_SECS, &id).unwrap();
    let result = decode_jwt("wrong-secret", &token);
    assert!(result.is_err());
}

#[test]
fn jwt_expired_token_returns_expired_error() {
    use chrono::Utc;
    use jsonwebtoken::{EncodingKey, Header, encode};

    let id = test_identity("aaaaaaaa-0000-0000-0000-000000000003");

    // Directly encode a token with exp = now - 100 s (unambiguously in the past).
    // encode_jwt adds `now + expiry_secs`, so we build the claims manually here.
    let now = Utc::now().timestamp() as u64;
    let claims = JwtClaims {
        sub: id.user_id.clone(),
        jti: uuid::Uuid::new_v4().to_string(),
        exp: now.saturating_sub(100),
        iat: now.saturating_sub(200),
        username: id.username.clone(),
        is_superuser: id.is_superuser,
        team_id: None,
        department_id: None,
        role: None,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(b"secret"),
    )
    .unwrap();

    let result = decode_jwt("secret", &token);
    assert!(matches!(result, Err(crate::AuthError::Expired)));
}

#[test]
fn jwt_superuser_flag_roundtrips() {
    let mut id = test_identity("aaaaaaaa-0000-0000-0000-000000000004");
    id.is_superuser = true;
    id.role = Some(Role::Admin);

    let token = encode_jwt("s", DEFAULT_EXPIRY_SECS, &id).unwrap();
    let decoded = decode_jwt("s", &token).unwrap();
    assert!(decoded.is_superuser);
    assert_eq!(decoded.role, Some(Role::Admin));
}

// ── extract_jti ──────────────────────────────────────────────────────────────

#[test]
fn extract_jti_returns_some_for_valid_jwt() {
    let id = test_identity("aaaaaaaa-0000-0000-0000-000000000005");
    let token = encode_jwt("secret", DEFAULT_EXPIRY_SECS, &id).unwrap();
    let jti = extract_jti(&token);
    assert!(jti.is_some(), "JTI should be extractable from a fresh token");
    // JTI should look like a UUID
    let jti_val = jti.unwrap();
    assert!(!jti_val.is_empty());
    assert!(uuid::Uuid::parse_str(&jti_val).is_ok(), "JTI should be a valid UUID");
}

#[test]
fn extract_jti_returns_none_for_garbage() {
    assert!(extract_jti("not.a.jwt").is_none());
    assert!(extract_jti("only_one_part").is_none());
    // Two parts but invalid base64
    assert!(extract_jti("header.!!INVALID!!.sig").is_none());
}

#[test]
fn extract_jti_is_unique_per_token() {
    let id = test_identity("aaaaaaaa-0000-0000-0000-000000000006");
    let t1 = encode_jwt("s", DEFAULT_EXPIRY_SECS, &id).unwrap();
    let t2 = encode_jwt("s", DEFAULT_EXPIRY_SECS, &id).unwrap();
    assert_ne!(extract_jti(&t1), extract_jti(&t2), "each token should have a unique JTI");
}

// ── hash_jti ─────────────────────────────────────────────────────────────────

#[test]
fn hash_jti_is_deterministic() {
    assert_eq!(hash_jti("my-jti-value"), hash_jti("my-jti-value"));
}

#[test]
fn hash_jti_different_inputs_produce_different_hashes() {
    assert_ne!(hash_jti("jti-a"), hash_jti("jti-b"));
}

#[test]
fn hash_jti_is_64_hex_chars() {
    let h = hash_jti("any-jti");
    assert_eq!(h.len(), 64, "SHA-256 hex output should be 64 characters");
    assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
}

// ── SingleUserAuth ────────────────────────────────────────────────────────────

#[tokio::test]
async fn single_user_auth_accepts_any_token() {
    let auth = SingleUserAuth;
    let result = auth.validate_token("any-garbage-token").await;
    assert!(result.is_ok());
    let id = result.unwrap();
    assert!(id.is_superuser);
    assert_eq!(id.username, "admin");
}

#[tokio::test]
async fn single_user_auth_accepts_empty_token() {
    let auth = SingleUserAuth;
    assert!(auth.validate_token("").await.is_ok());
}

// ── SimpleJwtAuth ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn simple_jwt_auth_rejects_invalid_token() {
    let auth = SimpleJwtAuth { secret: "s".into(), expiry_secs: DEFAULT_EXPIRY_SECS };
    let result = auth.validate_token("not-a-jwt").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn simple_jwt_auth_roundtrip_via_issue_and_validate() {
    let auth = SimpleJwtAuth { secret: "roundtrip-secret".into(), expiry_secs: DEFAULT_EXPIRY_SECS };
    let id = test_identity("aaaaaaaa-0000-0000-0000-000000000010");
    let token = auth.issue_token(&id).await.unwrap();
    let decoded = auth.validate_token(&token).await.unwrap();
    assert_eq!(decoded.user_id, id.user_id);
    assert_eq!(decoded.username, id.username);
}

// ── Role ordering ─────────────────────────────────────────────────────────────

#[test]
fn role_ordering_is_correct() {
    use Role::*;
    assert!(Admin > DepartmentManager);
    assert!(DepartmentManager > TeamLead);
    assert!(TeamLead > TeamMember);
    assert!(TeamMember > Member);
    assert!(Admin >= Admin);
}

// ── generate_access_key / generate_access_secret ─────────────────────────────

#[test]
fn access_key_has_correct_prefix_and_length() {
    let key = crate::generate_access_key();
    assert!(key.starts_with("NASK_"), "key should start with NASK_");
    assert_eq!(key.len(), 27, "NASK_ (5) + 22 chars");
}

#[test]
fn access_keys_are_unique() {
    let k1 = crate::generate_access_key();
    let k2 = crate::generate_access_key();
    assert_ne!(k1, k2);
}

#[test]
fn access_secret_is_43_chars() {
    let s = crate::generate_access_secret();
    assert_eq!(s.len(), 43);
}

// ── password hashing ──────────────────────────────────────────────────────────

#[test]
fn password_hash_and_verify() {
    let hash = crate::hash_password("hunter2").unwrap();
    assert!(crate::verify_password("hunter2", &hash));
    assert!(!crate::verify_password("wrong", &hash));
}
