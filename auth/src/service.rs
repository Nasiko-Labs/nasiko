use async_trait::async_trait;
use chrono::Utc;
use sqlx::PgPool;

use crate::{AuthError, AuthService, Identity, LoginResult, TOKEN_EXPIRY_SECS};

/// Lock account after this many consecutive failed login attempts.
const LOGIN_LOCKOUT_THRESHOLD: i32 = 3;
/// Lockout duration in seconds (15 minutes).
const LOGIN_LOCKOUT_DURATION_SECS: u64 = 15 * 60;

/// DB-backed implementation of AuthService.
/// Handles user lookup, password verification, token issuance, and revocation.
#[derive(Clone)]
pub struct AuthServiceImpl {
    db: PgPool,
    jwt_secret: String,
}

impl AuthServiceImpl {
    pub fn new(db: PgPool, jwt_secret: String) -> Self {
        Self { db, jwt_secret }
    }

    /// Record a user token JTI so it can be revoked later.
    /// Fire-and-forget — a failure to record doesn't fail the login.
    async fn record_token(&self, token: &str, user_id: uuid::Uuid) {
        let Some(jti) = crate::jwt::extract_jti(token) else {
            return;
        };
        let hash = crate::jwt::hash_jti(&jti);
        let expires = Utc::now() + chrono::Duration::seconds(TOKEN_EXPIRY_SECS as i64);
        let _ = sqlx::query(
            "INSERT INTO auth_tokens (user_id, token_hash, expires_at)
             VALUES ($1, $2, $3)
             ON CONFLICT (token_hash) DO NOTHING",
        )
        .bind(user_id)
        .bind(hash)
        .bind(expires)
        .execute(&self.db)
        .await;
    }

    /// Record an agent token JTI so it can be revoked later.
    /// Agent tokens store `agent_id` instead of `user_id` (different subject table).
    async fn record_agent_token(&self, token: &str, agent_id: uuid::Uuid) {
        let Some(jti) = crate::jwt::extract_jti(token) else {
            return;
        };
        let hash = crate::jwt::hash_jti(&jti);
        let expires = Utc::now() + chrono::Duration::seconds(TOKEN_EXPIRY_SECS as i64);
        let _ = sqlx::query(
            "INSERT INTO auth_tokens (agent_id, token_hash, expires_at)
             VALUES ($1, $2, $3)
             ON CONFLICT (token_hash) DO NOTHING",
        )
        .bind(agent_id)
        .bind(hash)
        .bind(expires)
        .execute(&self.db)
        .await;
    }
}

#[async_trait]
impl AuthService for AuthServiceImpl {
    async fn validate_token(&self, token: &str) -> Result<Identity, AuthError> {
        crate::jwt::decode_jwt(&self.jwt_secret, token)
    }

    async fn issue_token(&self, identity: &Identity) -> Result<String, AuthError> {
        let token = crate::jwt::encode_jwt(&self.jwt_secret, TOKEN_EXPIRY_SECS, identity)?;
        // Record every issued token in auth_tokens so it is revocable (matches the
        // EE impl). Previously only `authenticate` recorded, so tokens minted via
        // `issue_token` directly — e.g. initialize-admin — were unrevocable.
        // Best-effort + ON CONFLICT DO NOTHING, so it's safe if a caller also records.
        if let Ok(uid) = identity.user_id.parse::<uuid::Uuid>() {
            self.record_token(&token, uid).await;
        }
        Ok(token)
    }

    async fn authenticate(&self, username: &str, password: &str) -> Result<LoginResult, AuthError> {
        #[derive(sqlx::FromRow)]
        struct CredRow {
            id: uuid::Uuid,
            username: String,
            is_superuser: bool,
            is_active: bool,
            access_secret_hash: String,
            failed_login_attempts: i32,
            locked_until: Option<chrono::DateTime<chrono::Utc>>,
        }

        let row: Option<CredRow> = sqlx::query_as(
            r#"SELECT u.id, u.username, u.is_superuser, u.is_active,
                      uc.access_secret_hash, u.failed_login_attempts, u.locked_until
               FROM users u
               JOIN user_credentials uc ON uc.user_id = u.id
               WHERE (uc.access_key = $1 OR u.username = $1) AND u.deleted_at IS NULL"#,
        )
        .bind(username)
        .fetch_optional(&self.db)
        .await?;

        let row = row.ok_or(AuthError::InvalidCredentials {
            remaining_attempts: None,
        })?;

        if !row.is_active {
            return Err(AuthError::Disabled);
        }

        // Check lockout: 3 consecutive failures → locked for 15 minutes.
        if let Some(locked_until) = row.locked_until {
            let now = chrono::Utc::now();
            if now < locked_until {
                let remaining = (locked_until - now).num_seconds().max(0) as u64;
                return Err(AuthError::AccountLocked {
                    retry_after_secs: remaining,
                });
            }
            // Lockout expired — clear it so the attempt proceeds.
            let _ = sqlx::query(
                "UPDATE users SET failed_login_attempts = 0, locked_until = NULL WHERE id = $1",
            )
            .bind(row.id)
            .execute(&self.db)
            .await;
        }

        if !crate::verify_password_async(password, &row.access_secret_hash).await {
            let attempts = row.failed_login_attempts + 1;
            if attempts >= LOGIN_LOCKOUT_THRESHOLD {
                let _ = sqlx::query(
                    "UPDATE users SET failed_login_attempts = $2, \
                     locked_until = now() + interval '15 minutes' WHERE id = $1",
                )
                .bind(row.id)
                .bind(attempts)
                .execute(&self.db)
                .await;
                return Err(AuthError::AccountLocked {
                    retry_after_secs: LOGIN_LOCKOUT_DURATION_SECS,
                });
            }
            let _ = sqlx::query("UPDATE users SET failed_login_attempts = $2 WHERE id = $1")
                .bind(row.id)
                .bind(attempts)
                .execute(&self.db)
                .await;
            let remaining = LOGIN_LOCKOUT_THRESHOLD - attempts;
            return Err(AuthError::InvalidCredentials {
                remaining_attempts: Some(remaining),
            });
        }

        // Successful login — reset failed attempts and update last_login.
        let _ = sqlx::query(
            "UPDATE users SET last_login = now(), failed_login_attempts = 0, locked_until = NULL WHERE id = $1",
        )
        .bind(row.id)
        .execute(&self.db)
        .await;

        let identity = Identity {
            user_id: row.id.to_string(),
            username: row.username.clone(),
            is_superuser: row.is_superuser,
        };

        // issue_token now records the token itself, so no explicit record here.
        let token = self.issue_token(&identity).await?;

        Ok(LoginResult {
            token,
            user_id: row.id.to_string(),
            username: row.username,
            is_superuser: row.is_superuser,
            expires_in: TOKEN_EXPIRY_SECS,
            access_key: None,
            access_secret: None,
        })
    }

    async fn bootstrap_admin(&self, username: &str, password: &str) -> Result<(), AuthError> {
        let admin_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM users WHERE role = 'admin' AND deleted_at IS NULL",
        )
        .fetch_one(&self.db)
        .await
        .unwrap_or(0);

        if admin_count > 0 {
            return Ok(());
        }

        let access_secret_hash = crate::hash_password_async(password).await?;

        let email = format!("{}@localhost", username);
        let result: Result<(uuid::Uuid,), _> = sqlx::query_as(
            r#"INSERT INTO users (username, email, is_superuser, is_active, role)
               VALUES ($1, $2, true, true, 'admin'::user_role)
               RETURNING id"#,
        )
        .bind(username)
        .bind(&email)
        .fetch_one(&self.db)
        .await;

        let user_id = match result {
            Ok((id,)) => id,
            Err(e) if e.to_string().contains("unique") || e.to_string().contains("duplicate") => {
                return Ok(());
            }
            Err(e) => return Err(AuthError::Database(e)),
        };

        sqlx::query(
            r#"INSERT INTO user_credentials (user_id, access_key, access_secret_hash)
               VALUES ($1, $2, $3)"#,
        )
        .bind(user_id)
        .bind(username)
        .bind(&access_secret_hash)
        .execute(&self.db)
        .await?;

        Ok(())
    }

    async fn issue_agent_token(&self, agent_id: &str) -> Result<String, AuthError> {
        let agent_uuid = agent_id
            .parse::<uuid::Uuid>()
            .map_err(|_| AuthError::NotFound)?;

        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM agents WHERE id = $1 AND deleted_at IS NULL)",
        )
        .bind(agent_uuid)
        .fetch_one(&self.db)
        .await
        .unwrap_or(false);

        if !exists {
            return Err(AuthError::NotFound);
        }

        let identity = Identity {
            user_id: agent_id.to_owned(),
            username: format!("agent:{}", agent_id),
            is_superuser: false,
        };

        // Use encode_agent_jwt so token_type = "agent" — prevents the token from
        // being accepted by decode_jwt as a human-user credential (AUTH-3).
        let token = crate::jwt::encode_agent_jwt(&self.jwt_secret, TOKEN_EXPIRY_SECS, &identity)?;
        self.record_agent_token(&token, agent_uuid).await;
        Ok(token)
    }

    async fn upsert_oauth_user(
        &self,
        provider: &str,
        provider_id: &str,
        username: &str,
        verified_email: Option<&str>,
    ) -> Result<LoginResult, AuthError> {
        let existing: Option<(uuid::Uuid,)> = sqlx::query_as(
            "SELECT user_id FROM user_identities WHERE provider = $1 AND provider_id = $2",
        )
        .bind(provider)
        .bind(provider_id)
        .fetch_optional(&self.db)
        .await?;

        let user_id = if let Some((uid,)) = existing {
            let _ = sqlx::query("UPDATE users SET last_login = now() WHERE id = $1")
                .bind(uid)
                .execute(&self.db)
                .await;
            uid
        } else {
            // First time we've seen this (provider, provider_id). If the provider
            // gave us a *verified* email that already belongs to a user, link this
            // new identity to that existing user instead of creating a duplicate —
            // this is how one person keeps a single CP user across Google + GitHub.
            // Synthetic placeholders (`{username}@{provider}.users`) are never
            // matched: callers only pass emails they have verified.
            let linked: Option<(uuid::Uuid,)> = match verified_email {
                Some(email) => {
                    sqlx::query_as("SELECT id FROM users WHERE email = $1 AND deleted_at IS NULL")
                        .bind(email)
                        .fetch_optional(&self.db)
                        .await?
                }
                None => None,
            };

            let user_id = if let Some((uid,)) = linked {
                let _ = sqlx::query("UPDATE users SET last_login = now() WHERE id = $1")
                    .bind(uid)
                    .execute(&self.db)
                    .await;
                uid
            } else {
                // Brand-new user. Persist the real verified email when present;
                // otherwise fall back to the synthetic placeholder so the NOT NULL
                // UNIQUE `email` column stays satisfied (single-tenant path, unchanged).
                let email = verified_email
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("{}@{}.users", username, provider));
                // ON CONFLICT: if the username already exists (e.g. the
                // provider label changed since a previous login, or the user
                // was seeded manually), link to the existing row rather than
                // failing — the identity row created below is what matters.
                let row: (uuid::Uuid,) = sqlx::query_as(
                    "INSERT INTO users (username, email, is_superuser, is_active, last_login) \
                     VALUES ($1, $2, false, true, now()) \
                     ON CONFLICT (username) DO UPDATE SET last_login = now() \
                     RETURNING id",
                )
                .bind(username)
                .bind(&email)
                .fetch_one(&self.db)
                .await?;
                row.0
            };

            // Link this provider identity to the user (new or existing). ON CONFLICT
            // keeps it idempotent if two logins race.
            sqlx::query(
                r#"INSERT INTO user_identities (user_id, provider, provider_id, provider_username)
                   VALUES ($1, $2, $3, $4)
                   ON CONFLICT (provider, provider_id) DO UPDATE SET provider_username = EXCLUDED.provider_username"#,
            )
            .bind(user_id)
            .bind(provider)
            .bind(provider_id)
            .bind(username)
            .execute(&self.db)
            .await?;

            user_id
        };

        // Read the real superuser flag from the DB instead of hardcoding false.
        // A user seeded via the admin API with `is_superuser=true` (e.g. the
        // multi-tenant workspace CREATOR, pre-designated by tenant-server's
        // finalize) MUST keep it when they first sign in through SSO — hardcoding
        // false silently demoted them to a plain member and made the whole
        // "creator is admin" path a no-op. A brand-new SSO user was just INSERTed
        // above with `is_superuser=false`, so this reads false for them: the
        // "new SSO users land as members" default is unchanged.
        let is_superuser: bool = sqlx::query_scalar(
            "SELECT is_superuser FROM users WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(user_id)
        .fetch_one(&self.db)
        .await?;

        let identity = Identity {
            user_id: user_id.to_string(),
            username: username.to_owned(),
            is_superuser,
        };

        let token = self.issue_token(&identity).await?;
        self.record_token(&token, user_id).await;

        Ok(LoginResult {
            token,
            user_id: user_id.to_string(),
            username: username.to_owned(),
            is_superuser,
            expires_in: TOKEN_EXPIRY_SECS,
            access_key: None,
            access_secret: None,
        })
    }

    async fn lookup_user(&self, user_id: &str) -> Result<Identity, AuthError> {
        let user_uuid = user_id
            .parse::<uuid::Uuid>()
            .map_err(|_| AuthError::NotFound)?;

        #[derive(sqlx::FromRow)]
        struct UserRow {
            is_superuser: bool,
            username: String,
        }

        let row: Option<UserRow> = sqlx::query_as(
            "SELECT is_superuser, username FROM users WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(user_uuid)
        .fetch_optional(&self.db)
        .await?;

        let row = row.ok_or(AuthError::NotFound)?;

        Ok(Identity {
            user_id: user_id.to_owned(),
            username: row.username,
            is_superuser: row.is_superuser,
        })
    }

    async fn record_user_token(&self, token: &str, user_id: &str) -> Result<(), AuthError> {
        let user_uuid = user_id
            .parse::<uuid::Uuid>()
            .map_err(|_| AuthError::NotFound)?;
        self.record_token(token, user_uuid).await;
        Ok(())
    }

    async fn revoke_token(&self, jti: &str) -> Result<u64, AuthError> {
        let hash = crate::jwt::hash_jti(jti);

        let result = sqlx::query(
            "UPDATE auth_tokens SET revoked_at = now() WHERE token_hash = $1 AND revoked_at IS NULL AND expires_at > now()",
        )
        .bind(&hash)
        .execute(&self.db)
        .await
?;

        Ok(result.rows_affected())
    }

    async fn revoke_tokens_for_user(&self, user_id: &str) -> Result<u64, AuthError> {
        let user_uuid = user_id
            .parse::<uuid::Uuid>()
            .map_err(|_| AuthError::NotFound)?;

        let result = sqlx::query(
            "UPDATE auth_tokens SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > now()",
        )
        .bind(user_uuid)
        .execute(&self.db)
        .await
?;

        Ok(result.rows_affected())
    }

    async fn revoke_all_tokens(&self) -> Result<u64, AuthError> {
        let result = sqlx::query(
            "UPDATE auth_tokens SET revoked_at = now() WHERE revoked_at IS NULL AND expires_at > now()",
        )
        .execute(&self.db)
        .await
?;

        Ok(result.rows_affected())
    }

    async fn revoke_tokens_for_agent(&self, agent_id: &str) -> Result<u64, AuthError> {
        let agent_uuid = agent_id
            .parse::<uuid::Uuid>()
            .map_err(|_| AuthError::NotFound)?;

        let result = sqlx::query(
            "UPDATE auth_tokens SET revoked_at = now() WHERE agent_id = $1 AND revoked_at IS NULL AND expires_at > now()",
        )
        .bind(agent_uuid)
        .execute(&self.db)
        .await
?;

        Ok(result.rows_affected())
    }

    /// OSS access rule: owner ∪ public ∪ user-grant (superuser sees all).
    /// Team/department grants are EE-only — `EeAuthService` overrides this with the
    /// fuller check. This is the single source of truth for per-agent access in OSS;
    /// handlers reach it via `state.auth.can_access_agent`, so the check is
    /// edition-aware without duplicating SQL per handler.
    async fn can_access_agent(&self, identity: &Identity, agent_id: &str) -> bool {
        if identity.is_superuser {
            return true;
        }
        let Ok(agent_uuid) = agent_id.parse::<uuid::Uuid>() else {
            return false;
        };
        let Ok(user_uuid) = identity.user_id.parse::<uuid::Uuid>() else {
            return false;
        };

        sqlx::query_scalar::<_, bool>(
            r#"SELECT EXISTS(
                SELECT 1 FROM agents a
                WHERE a.id = $2
                  AND a.deleted_at IS NULL
                  AND (
                      a.owner_id = $1
                      OR a.is_public = TRUE
                      OR EXISTS (
                          SELECT 1 FROM agent_grants ag
                          WHERE ag.agent_id = a.id
                            AND (
                                (ag.grant_type = 'public' AND ag.grantee_id = '*')
                             OR (ag.grant_type = 'user'   AND ag.grantee_id = $1::text)
                            )
                      )
                  )
            )"#,
        )
        .bind(user_uuid)
        .bind(agent_uuid)
        .fetch_one(&self.db)
        .await
        .unwrap_or(false)
    }
}
