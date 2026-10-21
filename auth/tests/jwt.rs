//! Integration tests for JWT encoding, decoding, and related helpers.

mod common;

use nasiko_auth::{
    AuthError, AuthService, AuthServiceImpl, Identity, SimpleJwtAuth,
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
    assert!(
        json.get("team_id").is_none(),
        "Identity must not expose team_id"
    );
    assert!(
        json.get("department_id").is_none(),
        "Identity must not expose department_id"
    );
}

// ─── Wrong secret ─────────────────────────────────────────────────────────────

#[test]
fn jwt_wrong_secret_is_rejected() {
    let id = make_identity("aaaaaaaa-0000-0000-0000-000000000010");
    let token = encode_jwt("correct-secret", DEFAULT_EXPIRY_SECS, &id).unwrap();
    let result = decode_jwt("wrong-secret", &token);
    assert!(
        result.is_err(),
        "token signed with a different secret must be rejected"
    );
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
    let token = jwt_encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(b"secret"),
    )
    .unwrap();
    let result = decode_jwt("secret", &token);
    assert!(
        matches!(result, Err(AuthError::Expired)),
        "expected Expired, got: {:?}",
        result
    );
}

// ─── extract_jti ─────────────────────────────────────────────────────────────

#[test]
fn extract_jti_returns_some_for_valid_jwt() {
    let id = make_identity("aaaaaaaa-0000-0000-0000-000000000040");
    let token = encode_jwt("secret", DEFAULT_EXPIRY_SECS, &id).unwrap();
    assert!(
        extract_jti(&token).is_some(),
        "JTI must be present in a freshly issued token"
    );
}

#[test]
fn extract_jti_value_is_a_valid_uuid() {
    let id = make_identity("aaaaaaaa-0000-0000-0000-000000000041");
    let token = encode_jwt("secret", DEFAULT_EXPIRY_SECS, &id).unwrap();
    let jti = extract_jti(&token).unwrap();
    assert!(
        uuid::Uuid::parse_str(&jti).is_ok(),
        "JTI should be a valid UUID, got: {jti}"
    );
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
    assert_ne!(
        extract_jti(&t1),
        extract_jti(&t2),
        "each token must have a unique JTI"
    );
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
    assert!(
        h.chars().all(|c| c.is_ascii_hexdigit()),
        "hash must be lowercase hex"
    );
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
    let auth = SimpleJwtAuth {
        secret: "roundtrip-secret".into(),
        expiry_secs: DEFAULT_EXPIRY_SECS,
    };
    let id = make_identity("aaaaaaaa-0000-0000-0000-000000000050");
    let token = auth.issue_token(&id).await.unwrap();
    let decoded = auth.validate_token(&token).await.unwrap();
    assert_eq!(decoded.user_id, id.user_id);
    assert_eq!(decoded.username, id.username);
}

#[tokio::test]
async fn simple_jwt_auth_rejects_garbage_token() {
    let auth = SimpleJwtAuth {
        secret: "s".into(),
        expiry_secs: DEFAULT_EXPIRY_SECS,
    };
    assert!(auth.validate_token("not-a-jwt").await.is_err());
}

#[tokio::test]
async fn simple_jwt_auth_rejects_token_from_different_secret() {
    let signer = SimpleJwtAuth {
        secret: "correct".into(),
        expiry_secs: DEFAULT_EXPIRY_SECS,
    };
    let verifier = SimpleJwtAuth {
        secret: "wrong".into(),
        expiry_secs: DEFAULT_EXPIRY_SECS,
    };
    let id = make_identity("aaaaaaaa-0000-0000-0000-000000000051");
    let token = signer.issue_token(&id).await.unwrap();
    assert!(verifier.validate_token(&token).await.is_err());
}

#[tokio::test]
async fn simple_jwt_auth_can_access_agent_always_true() {
    let auth = SimpleJwtAuth {
        secret: "s".into(),
        expiry_secs: DEFAULT_EXPIRY_SECS,
    };
    let id = make_identity("aaaaaaaa-0000-0000-0000-000000000052");
    assert!(auth.can_access_agent(&id, "any-agent-id").await);
}

// ─── Token revocation (requires DB — ignored) ────────────────────────────────

#[tokio::test]
#[serial_test::serial]
#[ignore = "requires live Postgres database"]
async fn auth_service_impl_revoke_tokens_for_user_requires_db() {
    let test_db = common::TestDb::create().await;
    let auth = AuthServiceImpl::new(test_db.pool.clone(), "test-revocation-secret".into());

    let (user_a_id, id_a) = test_db.create_user("revocation_user_a").await;
    let (user_b_id, id_b) = test_db.create_user("revocation_user_b").await;

    // Mint two tokens for user A and one token for user B
    let token_a1 = auth.issue_token(&id_a).await.expect("mint token a1");
    let token_a2 = auth.issue_token(&id_a).await.expect("mint token a2");
    let token_b1 = auth.issue_token(&id_b).await.expect("mint token b1");

    // Validate minted tokens
    let decoded_a1 = auth.validate_token(&token_a1).await.expect("validate a1");
    assert_eq!(decoded_a1.user_id, user_a_id.to_string());
    let decoded_b1 = auth.validate_token(&token_b1).await.expect("validate b1");
    assert_eq!(decoded_b1.user_id, user_b_id.to_string());

    // Verify all 3 tokens are recorded in auth_tokens with revoked_at IS NULL
    let active_before: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM auth_tokens WHERE revoked_at IS NULL",
    )
    .fetch_one(&test_db.pool)
    .await
    .expect("count active tokens");
    assert_eq!(active_before, 3);

    // Revoke tokens for user A only
    let revoked_count = auth
        .revoke_tokens_for_user(&user_a_id.to_string())
        .await
        .expect("revoke tokens for user a");
    assert_eq!(revoked_count, 2, "must revoke exactly 2 tokens belonging to user A");

    // Verify User A tokens now have revoked_at set
    let user_a_revoked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM auth_tokens WHERE user_id = $1 AND revoked_at IS NOT NULL",
    )
    .bind(user_a_id)
    .fetch_one(&test_db.pool)
    .await
    .expect("count user A revoked tokens");
    assert_eq!(user_a_revoked, 2);

    // Verify User B token is still active (unrevoked)
    let user_b_active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM auth_tokens WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_b_id)
    .fetch_one(&test_db.pool)
    .await
    .expect("count user B active tokens");
    assert_eq!(user_b_active, 1);

    // Revoking again should be idempotent and return 0 affected rows
    let second_revocation = auth
        .revoke_tokens_for_user(&user_a_id.to_string())
        .await
        .expect("repeat revoke for user a");
    assert_eq!(second_revocation, 0);

    test_db.cleanup().await;
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "requires live Postgres database"]
async fn auth_service_impl_revoke_all_tokens_requires_db() {
    let test_db = common::TestDb::create().await;
    let auth = AuthServiceImpl::new(test_db.pool.clone(), "test-revocation-all-secret".into());

    let (user_a_id, id_a) = test_db.create_user("revoke_all_user_a").await;
    let (user_b_id, id_b) = test_db.create_user("revoke_all_user_b").await;
    let agent_id = test_db.create_agent("revoke_all_agent", user_a_id).await;

    // Mint user tokens and an agent token
    let token_a = auth.issue_token(&id_a).await.expect("mint token a");
    let token_b = auth.issue_token(&id_b).await.expect("mint token b");
    let token_agent = auth
        .issue_agent_token(&agent_id.to_string())
        .await
        .expect("mint agent token");

    // Validate tokens
    assert_eq!(
        auth.validate_token(&token_a).await.unwrap().user_id,
        user_a_id.to_string()
    );
    assert_eq!(
        auth.validate_token(&token_b).await.unwrap().user_id,
        user_b_id.to_string()
    );

    // All 3 tokens should be active in auth_tokens
    let active_before: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM auth_tokens WHERE revoked_at IS NULL",
    )
    .fetch_one(&test_db.pool)
    .await
    .expect("count active before");
    assert_eq!(active_before, 3);

    // Revoke all tokens across the database
    let revoked_count = auth.revoke_all_tokens().await.expect("revoke all tokens");
    assert_eq!(revoked_count, 3, "must revoke all 3 active tokens");

    // Verify 0 active tokens remain
    let active_after: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM auth_tokens WHERE revoked_at IS NULL",
    )
    .fetch_one(&test_db.pool)
    .await
    .expect("count active after");
    assert_eq!(active_after, 0);

    // Verify all 3 tokens are marked revoked
    let revoked_total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM auth_tokens WHERE revoked_at IS NOT NULL",
    )
    .fetch_one(&test_db.pool)
    .await
    .expect("count revoked after");
    assert_eq!(revoked_total, 3);

    // Repeated call should return 0 affected rows
    let second_revocation = auth.revoke_all_tokens().await.expect("repeat revoke all");
    assert_eq!(second_revocation, 0);

    test_db.cleanup().await;
}
