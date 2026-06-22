use async_trait::async_trait;
use sqlx::PgPool;
use std::sync::Arc;

use crate::{AuthError, AuthProvider, Identity, LoginResult, Role, TokenService, UserAuthService};

const TOKEN_EXPIRY_SECS: u64 = 7 * 24 * 60 * 60;

/// DB-backed implementation of UserAuthService + TokenService.
/// Handles user lookup, password verification, and token issuance against Postgres.
/// Used in both OSS and EE — EE passes `JwtAuthProvider` as the inner auth for token recording.
#[derive(Clone)]
pub struct UserAuthServiceImpl {
    db: PgPool,
    auth: Arc<dyn AuthProvider>,
}

impl UserAuthServiceImpl {
    pub fn new(db: PgPool, auth: Arc<dyn AuthProvider>) -> Self {
        Self { db, auth }
    }
}

fn parse_role(role_str: Option<&str>) -> Option<Role> {
    role_str.and_then(|r| serde_json::from_value(serde_json::Value::String(r.to_owned())).ok())
}

#[async_trait]
impl UserAuthService for UserAuthServiceImpl {
    async fn authenticate(&self, access_key: &str, access_secret: &str) -> Result<LoginResult, AuthError> {
        #[derive(sqlx::FromRow)]
        struct CredRow {
            id: uuid::Uuid,
            username: String,
            is_superuser: bool,
            is_active: bool,
            role: Option<String>,
            team_id: Option<String>,
            department_id: Option<String>,
            access_secret_hash: String,
        }

        let row: Option<CredRow> = sqlx::query_as(
            r#"SELECT u.id, u.username, u.is_superuser, u.is_active,
                      u.role::text as role,
                      u.team_id::text as team_id,
                      u.department_id::text as department_id,
                      uc.access_secret_hash
               FROM users u
               JOIN user_credentials uc ON uc.user_id = u.id
               WHERE uc.access_key = $1 AND u.deleted_at IS NULL"#,
        )
        .bind(access_key)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

        let row = row.ok_or(AuthError::InvalidToken("invalid credentials".into()))?;

        if !row.is_active {
            return Err(AuthError::InvalidToken("account disabled".into()));
        }

        if !crate::verify_password(access_secret, &row.access_secret_hash) {
            return Err(AuthError::InvalidToken("invalid credentials".into()));
        }

        let _ = sqlx::query("UPDATE users SET last_login = now() WHERE id = $1")
            .bind(row.id)
            .execute(&self.db)
            .await;

        let role = parse_role(row.role.as_deref());
        let role_str = row.role.clone().unwrap_or_else(|| "member".into());

        let identity = Identity {
            user_id: row.id.to_string(),
            sub: row.id.to_string(),
            username: row.username.clone(),
            is_superuser: row.is_superuser,
            role,
            team_id: row.team_id.clone(),
            department_id: row.department_id.clone(),
            exp: 0,
            iat: 0,
        };

        let token = self.auth.issue_token(&identity).await?;

        Ok(LoginResult {
            token,
            user_id: row.id.to_string(),
            username: row.username,
            is_superuser: row.is_superuser,
            role: role_str,
            team_id: row.team_id,
            department_id: row.department_id,
            expires_in: TOKEN_EXPIRY_SECS,
            access_key: None,
            access_secret: None,
        })
    }

    async fn initialize_admin(&self, username: &str, email: &str) -> Result<LoginResult, AuthError> {
        let admin_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM users WHERE role = 'admin' AND deleted_at IS NULL",
        )
        .fetch_one(&self.db)
        .await
        .unwrap_or(0);

        if admin_count > 0 {
            return Err(AuthError::InvalidToken("admin already exists".into()));
        }

        let access_key = crate::generate_access_key();
        let access_secret = crate::generate_access_secret();
        let access_secret_hash = crate::hash_password(&access_secret)?;

        let result: Result<(uuid::Uuid,), _> = sqlx::query_as(
            r#"INSERT INTO users (username, email, is_superuser, is_active, role)
               VALUES ($1, $2, true, true, 'admin'::user_role)
               RETURNING id"#,
        )
        .bind(username)
        .bind(email)
        .fetch_one(&self.db)
        .await;

        let user_id = match result {
            Ok((id,)) => id,
            Err(e) if e.to_string().contains("unique") || e.to_string().contains("duplicate") => {
                return Err(AuthError::InvalidToken("username or email already exists".into()));
            }
            Err(e) => return Err(AuthError::InvalidToken(e.to_string())),
        };

        sqlx::query(
            r#"INSERT INTO user_credentials (user_id, access_key, access_secret_hash)
               VALUES ($1, $2, $3)"#,
        )
        .bind(user_id)
        .bind(&access_key)
        .bind(&access_secret_hash)
        .execute(&self.db)
        .await
        .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

        let identity = Identity {
            user_id: user_id.to_string(),
            sub: user_id.to_string(),
            username: username.to_owned(),
            is_superuser: true,
            role: Some(Role::Admin),
            team_id: None,
            department_id: None,
            exp: 0,
            iat: 0,
        };

        let token = self.auth.issue_token(&identity).await?;

        Ok(LoginResult {
            token,
            user_id: user_id.to_string(),
            username: username.to_owned(),
            is_superuser: true,
            role: "admin".into(),
            team_id: None,
            department_id: None,
            expires_in: TOKEN_EXPIRY_SECS,
            access_key: Some(access_key),
            access_secret: Some(access_secret),
        })
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
            sub: agent_id.to_owned(),
            username: format!("agent:{}", agent_id),
            is_superuser: false,
            role: None,
            team_id: None,
            department_id: None,
            exp: 0,
            iat: 0,
        };

        self.auth.issue_token(&identity).await
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
            sub: user_id.to_string(),
            username: username.to_owned(),
            is_superuser: false,
            role: Some(Role::Member),
            team_id: None,
            department_id: None,
            exp: 0,
            iat: 0,
        };

        let token = self.auth.issue_token(&identity).await?;

        Ok(LoginResult {
            token,
            user_id: user_id.to_string(),
            username: username.to_owned(),
            is_superuser: false,
            role: "member".into(),
            team_id: None,
            department_id: None,
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
            role: Option<String>,
            team_id: Option<String>,
            department_id: Option<String>,
        }

        let row: Option<UserRow> = sqlx::query_as(
            r#"SELECT is_superuser, username,
                      role::text as role,
                      team_id::text as team_id,
                      department_id::text as department_id
               FROM users WHERE id = $1 AND deleted_at IS NULL"#,
        )
        .bind(user_uuid)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

        let row = row.ok_or(AuthError::InvalidToken("user not found".into()))?;

        let role = parse_role(row.role.as_deref());

        Ok(Identity {
            user_id: user_id.to_owned(),
            sub: user_id.to_owned(),
            username: row.username,
            is_superuser: row.is_superuser,
            role,
            team_id: row.team_id,
            department_id: row.department_id,
            exp: 0,
            iat: 0,
        })
    }
}

#[async_trait]
impl TokenService for UserAuthServiceImpl {
    async fn revoke_for_user(&self, user_id: &str) -> Result<u64, AuthError> {
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

    async fn revoke_all(&self) -> Result<u64, AuthError> {
        let result = sqlx::query(
            "UPDATE auth_tokens SET revoked_at = now() WHERE revoked_at IS NULL AND expires_at > now()",
        )
        .execute(&self.db)
        .await
        .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

        Ok(result.rows_affected())
    }
}
