use async_trait::async_trait;
use chrono::Utc;
use sqlx::PgPool;

use crate::{AuthError, AuthService, Identity, LoginResult};

const TOKEN_EXPIRY_SECS: u64 = 12 * 60 * 60; // 12 hours

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
        let Some(jti) = crate::jwt::extract_jti(token) else { return };
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
        let Some(jti) = crate::jwt::extract_jti(token) else { return };
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
        crate::jwt::encode_jwt(&self.jwt_secret, TOKEN_EXPIRY_SECS, identity)
    }

    async fn authenticate(&self, username: &str, password: &str) -> Result<LoginResult, AuthError> {
        #[derive(sqlx::FromRow)]
        struct CredRow {
            id: uuid::Uuid,
            username: String,
            is_superuser: bool,
            is_active: bool,
            access_secret_hash: String,
        }

        let row: Option<CredRow> = sqlx::query_as(
            r#"SELECT u.id, u.username, u.is_superuser, u.is_active,
                      uc.access_secret_hash
               FROM users u
               JOIN user_credentials uc ON uc.user_id = u.id
               WHERE (uc.access_key = $1 OR u.username = $1) AND u.deleted_at IS NULL"#,
        )
        .bind(username)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

        let row = row.ok_or(AuthError::InvalidToken("invalid credentials".into()))?;

        if !row.is_active {
            return Err(AuthError::InvalidToken("account disabled".into()));
        }

        if !crate::verify_password_async(password, &row.access_secret_hash).await {
            return Err(AuthError::InvalidToken("invalid credentials".into()));
        }

        let _ = sqlx::query("UPDATE users SET last_login = now() WHERE id = $1")
            .bind(row.id)
            .execute(&self.db)
            .await;

        let identity = Identity {
            user_id: row.id.to_string(),
            username: row.username.clone(),
            is_superuser: row.is_superuser,
        };

        let token = self.issue_token(&identity).await?;
        self.record_token(&token, row.id).await;

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
            Err(e) => return Err(AuthError::InvalidToken(e.to_string())),
        };

        sqlx::query(
            r#"INSERT INTO user_credentials (user_id, access_key, access_secret_hash)
               VALUES ($1, $2, $3)"#,
        )
        .bind(user_id)
        .bind(username)
        .bind(&access_secret_hash)
        .execute(&self.db)
        .await
        .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

        Ok(())
    }

    async fn issue_agent_token(&self, agent_id: &str) -> Result<String, AuthError> {
        let agent_uuid = agent_id
            .parse::<uuid::Uuid>()
            .map_err(|_| AuthError::InvalidToken("invalid agent_id".into()))?;

        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM agents WHERE id = $1 AND deleted_at IS NULL)",
        )
        .bind(agent_uuid)
        .fetch_one(&self.db)
        .await
        .unwrap_or(false);

        if !exists {
            return Err(AuthError::InvalidToken("agent not found".into()));
        }

        let identity = Identity {
            user_id: agent_id.to_owned(),
            username: format!("agent:{}", agent_id),
            is_superuser: false,
        };

        let token = self.issue_token(&identity).await?;
        self.record_agent_token(&token, agent_uuid).await;
        Ok(token)
    }

    async fn upsert_oauth_user(
        &self,
        provider: &str,
        provider_id: &str,
        username: &str,
    ) -> Result<LoginResult, AuthError> {
        let existing: Option<(uuid::Uuid,)> = sqlx::query_as(
            "SELECT user_id FROM user_identities WHERE provider = $1 AND provider_id = $2",
        )
        .bind(provider)
        .bind(provider_id)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

        let user_id = if let Some((uid,)) = existing {
            let _ = sqlx::query("UPDATE users SET last_login = now() WHERE id = $1")
                .bind(uid)
                .execute(&self.db)
                .await;
            uid
        } else {
            let email = format!("{}@{}.users", username, provider);
            let row: (uuid::Uuid,) = sqlx::query_as(
                "INSERT INTO users (username, email, is_superuser, is_active, last_login) VALUES ($1, $2, false, true, now()) RETURNING id",
            )
            .bind(username)
            .bind(&email)
            .fetch_one(&self.db)
            .await
            .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

            sqlx::query(
                r#"INSERT INTO user_identities (user_id, provider, provider_id, provider_username)
                   VALUES ($1, $2, $3, $4)
                   ON CONFLICT (provider, provider_id) DO UPDATE SET provider_username = EXCLUDED.provider_username"#,
            )
            .bind(row.0)
            .bind(provider)
            .bind(provider_id)
            .bind(username)
            .execute(&self.db)
            .await
            .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

            row.0
        };

        let identity = Identity {
            user_id: user_id.to_string(),
            username: username.to_owned(),
            is_superuser: false,
        };

        let token = self.issue_token(&identity).await?;
        self.record_token(&token, user_id).await;

        Ok(LoginResult {
            token,
            user_id: user_id.to_string(),
            username: username.to_owned(),
            is_superuser: false,
            expires_in: TOKEN_EXPIRY_SECS,
            access_key: None,
            access_secret: None,
        })
    }

    async fn lookup_user(&self, user_id: &str) -> Result<Identity, AuthError> {
        let user_uuid = user_id
            .parse::<uuid::Uuid>()
            .map_err(|_| AuthError::InvalidToken("invalid user_id".into()))?;

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
        .await
        .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

        let row = row.ok_or(AuthError::InvalidToken("user not found".into()))?;

        Ok(Identity {
            user_id: user_id.to_owned(),
            username: row.username,
            is_superuser: row.is_superuser,
        })
    }

    async fn record_user_token(&self, token: &str, user_id: &str) -> Result<(), AuthError> {
        let user_uuid = user_id
            .parse::<uuid::Uuid>()
            .map_err(|_| AuthError::InvalidToken("invalid user_id".into()))?;
        self.record_token(token, user_uuid).await;
        Ok(())
    }

    async fn revoke_tokens_for_user(&self, user_id: &str) -> Result<u64, AuthError> {
        let user_uuid = user_id
            .parse::<uuid::Uuid>()
            .map_err(|_| AuthError::InvalidToken("invalid user_id".into()))?;

        let result = sqlx::query(
            "UPDATE auth_tokens SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > now()",
        )
        .bind(user_uuid)
        .execute(&self.db)
        .await
        .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

        Ok(result.rows_affected())
    }

    async fn revoke_all_tokens(&self) -> Result<u64, AuthError> {
        let result = sqlx::query(
            "UPDATE auth_tokens SET revoked_at = now() WHERE revoked_at IS NULL AND expires_at > now()",
        )
        .execute(&self.db)
        .await
        .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

        Ok(result.rows_affected())
    }

    async fn revoke_tokens_for_agent(&self, agent_id: &str) -> Result<u64, AuthError> {
        let agent_uuid = agent_id
            .parse::<uuid::Uuid>()
            .map_err(|_| AuthError::InvalidToken("invalid agent_id".into()))?;

        let result = sqlx::query(
            "UPDATE auth_tokens SET revoked_at = now() WHERE agent_id = $1 AND revoked_at IS NULL AND expires_at > now()",
        )
        .bind(agent_uuid)
        .execute(&self.db)
        .await
        .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

        Ok(result.rows_affected())
    }

    async fn can_access_agent(&self, _identity: &Identity, _agent_id: &str) -> bool {
        true
    }
}
