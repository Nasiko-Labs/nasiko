use async_trait::async_trait;
use chrono::Utc;
use sqlx::PgPool;

use crate::DelegatedIdentity;
use crate::{AuthError, AuthService, Identity, LoginRefusal, LoginResult, TOKEN_EXPIRY_SECS};

/// Lock account after this many consecutive failed login attempts.
const LOGIN_LOCKOUT_THRESHOLD: i32 = 3;

/// What a login is verified against when no account has the name, so an unknown name
/// costs the same bcrypt work as a known one. A cost-12 hash (`hash_password`'s cost) of
/// a random string that was thrown away when the hash was made, so no password matches
/// it. A constant rather than a hash built at run time: hashing at cost 12 is blocking
/// work that belongs on the blocking pool, and one built on first use would make the
/// first unknown-name login after a start slower than the rest. The test below keeps it
/// well formed at cost 12.
const DUMMY_HASH: &str = "$2b$12$nUYnav7XHaAjjWOPq.UHzus3g9wflHitJOghjyswdO5LSq6fkx0xi";

/// The hash to verify a login password against: the account's own, or the dummy.
fn hash_for(stored: Option<&str>) -> &str {
    stored.unwrap_or(DUMMY_HASH)
}

/// One line per login attempt under `nasiko::auth`, for the log pipeline. Never the name
/// typed (people type passwords into that field), and not a hash of it either: an unknown
/// name is only flagged.
fn log_login_ok(user_id: &str) {
    tracing::info!(target: "nasiko::auth", event = "login", outcome = "ok", user_id, unknown_name = false);
}

fn log_login_refused(reason: LoginRefusal, user_id: Option<&str>) {
    tracing::info!(
        target: "nasiko::auth",
        event = "login",
        outcome = "refused",
        reason = reason.as_str(),
        user_id,
        unknown_name = reason == LoginRefusal::UnknownName,
    );
}

/// The `jti` of a token this service just signed. Every token it issues
/// carries one, so a missing `jti` is an internal fault, not a bad credential.
fn issued_jti(token: &str) -> Result<String, AuthError> {
    crate::jwt::extract_jti(token)
        .ok_or_else(|| AuthError::Internal("issued token has no jti".into()))
}

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
    /// A failure fails the caller: revocation only finds recorded tokens, so a
    /// token handed out unrecorded could never be revoked.
    async fn record_token(&self, token: &str, user_id: uuid::Uuid) -> Result<(), AuthError> {
        let jti = issued_jti(token)?;
        let hash = crate::jwt::hash_jti(&jti);
        let expires = Utc::now() + chrono::Duration::seconds(TOKEN_EXPIRY_SECS as i64);
        sqlx::query(
            "INSERT INTO auth_tokens (user_id, token_hash, expires_at)
             VALUES ($1, $2, $3)
             ON CONFLICT (token_hash) DO NOTHING",
        )
        .bind(user_id)
        .bind(hash)
        .bind(expires)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Record an agent token JTI so it can be revoked later.
    /// Agent tokens store `agent_id` instead of `user_id` (different subject table).
    async fn record_agent_token(&self, token: &str, agent_id: uuid::Uuid) -> Result<(), AuthError> {
        let jti = issued_jti(token)?;
        let hash = crate::jwt::hash_jti(&jti);
        let expires = Utc::now() + chrono::Duration::seconds(TOKEN_EXPIRY_SECS as i64);
        sqlx::query(
            "INSERT INTO auth_tokens (agent_id, token_hash, expires_at)
             VALUES ($1, $2, $3)
             ON CONFLICT (token_hash) DO NOTHING",
        )
        .bind(agent_id)
        .bind(hash)
        .bind(expires)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Whether this `(provider, provider_id)` OAuth login resolves to a user
    /// that already exists: an exact identity match, or a *verified* email that
    /// already belongs to a user (how one person keeps a single CP user across
    /// Google + GitHub). Synthetic email placeholders
    /// (`{username}@{provider}.users`) are never matched: callers only pass
    /// emails they have verified. Backs `would_create_user`, which must resolve
    /// "new" exactly as `upsert_oauth_user`'s own find-or-create does, or a cap
    /// on new accounts and the write would disagree.
    ///
    /// A username never matches: `insert_oauth_user` gives a first-time
    /// sign-in whose username is taken a numbered username of its own.
    async fn resolves_to_existing_user(
        &self,
        provider: &str,
        provider_id: &str,
        verified_email: Option<&str>,
    ) -> Result<bool, AuthError> {
        let existing: Option<(uuid::Uuid,)> = sqlx::query_as(
            "SELECT user_id FROM user_identities WHERE provider = $1 AND provider_id = $2",
        )
        .bind(provider)
        .bind(provider_id)
        .fetch_optional(&self.db)
        .await?;
        if existing.is_some() {
            return Ok(true);
        }

        let linked: Option<(uuid::Uuid,)> = match verified_email {
            Some(email) => {
                sqlx::query_as("SELECT id FROM users WHERE email = $1 AND deleted_at IS NULL")
                    .bind(email)
                    .fetch_optional(&self.db)
                    .await?
            }
            None => None,
        };
        Ok(linked.is_some())
    }
}

/// How many usernames `insert_oauth_user` tries: the provider's own, then
/// `-2` up to `-{MAX_OAUTH_USERNAME_ATTEMPTS}`.
const MAX_OAUTH_USERNAME_ATTEMPTS: u32 = 20;

impl AuthServiceImpl {
    /// Creates the account for a first-time external sign-in and returns its
    /// id. When the provider's username is already taken
    /// by another account (live or soft-deleted), a numbered variant is used
    /// instead; the existing account is never touched.
    async fn insert_oauth_user(
        &self,
        username: &str,
        provider: &str,
        verified_email: Option<&str>,
    ) -> Result<uuid::Uuid, AuthError> {
        for attempt in 1..=MAX_OAUTH_USERNAME_ATTEMPTS {
            let candidate = if attempt == 1 {
                username.to_owned()
            } else {
                format!("{username}-{attempt}")
            };
            // Persist the real verified email when present; otherwise a
            // synthetic placeholder keeps the NOT NULL UNIQUE `email` column
            // satisfied (single-tenant path).
            let email = verified_email
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{candidate}@{provider}.users"));
            // DO NOTHING on any unique conflict (username or email): no
            // existing row is updated, and RETURNING yields nothing.
            let row: Option<(uuid::Uuid,)> = sqlx::query_as(
                "INSERT INTO users (username, email, is_superuser, is_active, last_login) \
                 VALUES ($1, $2, false, true, now()) \
                 ON CONFLICT DO NOTHING \
                 RETURNING id",
            )
            .bind(&candidate)
            .bind(&email)
            .fetch_optional(&self.db)
            .await?;
            if let Some((id,)) = row {
                return Ok(id);
            }
        }
        Err(AuthError::Conflict(format!(
            "no free username derived from '{username}'"
        )))
    }
}

#[async_trait]
impl AuthService for AuthServiceImpl {
    async fn validate_token(&self, token: &str) -> Result<Identity, AuthError> {
        crate::jwt::decode_jwt(&self.jwt_secret, token)
    }

    /// A superuser, or an active, non-deleted user with `role = 'admin'`. The
    /// role is read from the database rather than the token, so a demotion
    /// takes effect at once. A failed lookup denies.
    async fn can_manage_users(&self, identity: &Identity) -> bool {
        if identity.is_superuser {
            return true;
        }
        let Ok(user_id) = identity.user_id.parse::<uuid::Uuid>() else {
            return false;
        };
        // A lookup error denies (`unwrap_or(None)`); this crate has no logger,
        // and the caller answers 403 either way.
        let is_admin: Option<bool> = sqlx::query_scalar(
            "SELECT role = 'admin' FROM users \
             WHERE id = $1 AND is_active AND deleted_at IS NULL",
        )
        .bind(user_id)
        .fetch_optional(&self.db)
        .await
        .unwrap_or(None);
        is_admin == Some(true)
    }

    async fn issue_delegated_token(
        &self,
        identity: &Identity,
        request: &crate::DelegationRequest,
    ) -> Result<crate::IssuedDelegation, AuthError> {
        let scopes = crate::check_delegation_scopes(&request.scopes)?;
        let ttl = request.ttl_secs.clamp(1, crate::DELEGATION_MAX_TTL_SECS);
        let (token, jti) = crate::jwt::encode_delegated_jwt(
            &self.jwt_secret,
            ttl,
            identity,
            &request.audience,
            &scopes,
            &request.parent_jti,
        )?;
        // Recorded with the user's id and its OWN short expiry, so
        // revoke_tokens_for_user / revoke_token(jti) cover it and the row
        // ages out quickly.
        let user_uuid = identity
            .user_id
            .parse::<uuid::Uuid>()
            .map_err(|_| AuthError::NotFound)?;
        let expires = Utc::now() + chrono::Duration::seconds(ttl as i64);
        sqlx::query(
            "INSERT INTO auth_tokens (user_id, token_hash, expires_at)
             VALUES ($1, $2, $3)
             ON CONFLICT (token_hash) DO NOTHING",
        )
        .bind(user_uuid)
        .bind(crate::jwt::hash_jti(&jti))
        .bind(expires)
        .execute(&self.db)
        .await
        .map_err(AuthError::Database)?;
        Ok(crate::IssuedDelegation {
            token,
            jti,
            expires_in: ttl,
            scopes,
        })
    }

    async fn validate_delegated_token(
        &self,
        token: &str,
        expected_audience: &str,
    ) -> Result<DelegatedIdentity, AuthError> {
        let d = crate::jwt::decode_delegated_jwt(&self.jwt_secret, token, expected_audience)?;
        // Same revocation + caller-exists rule as a session (fail closed).
        let caller_id = d.identity.user_id.parse::<uuid::Uuid>().ok();
        let (revoked, caller_exists): (bool, bool) = sqlx::query_as(
            "SELECT
                 EXISTS(SELECT 1 FROM auth_tokens
                         WHERE token_hash = $1 AND revoked_at IS NOT NULL),
                 EXISTS(SELECT 1 FROM users
                         WHERE id = $2 AND deleted_at IS NULL)",
        )
        .bind(crate::jwt::hash_jti(&d.jti))
        .bind(caller_id)
        .fetch_one(&self.db)
        .await
        .map_err(AuthError::Database)?;
        if revoked {
            return Err(AuthError::Revoked);
        }
        if caller_id.is_none() || !caller_exists {
            return Err(AuthError::InvalidToken(
                "delegated user no longer exists".into(),
            ));
        }
        Ok(d)
    }

    async fn issue_token(&self, identity: &Identity) -> Result<String, AuthError> {
        let token = crate::jwt::encode_jwt(&self.jwt_secret, TOKEN_EXPIRY_SECS, identity)?;
        // Record every issued token in auth_tokens so it is revocable (matches the
        // EE impl). Previously only `authenticate` recorded, so tokens minted via
        // `issue_token` directly — e.g. initialize-admin — were unrevocable.
        // ON CONFLICT DO NOTHING, so it's safe if a caller also records. A token
        // that cannot be recorded is not handed out: it could never be revoked.
        let uid = identity
            .user_id
            .parse::<uuid::Uuid>()
            .map_err(|_| AuthError::NotFound)?;
        self.record_token(&token, uid).await?;
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

        // One bcrypt check for every attempt, before anything is decided, so the bcrypt cost
        // of a refusal never says whether the name exists, is deactivated or is locked. One
        // difference in duration remains: a wrong password on a live account, and the first
        // login after a lock ends, also write to `users` (one or two `UPDATE` round trips,
        // small next to bcrypt), and the other refusals write nothing. No write is added to
        // the others to match: it would make the cheapest request to send, an unknown name,
        // cost the database a write each.
        let password_ok = crate::verify_password_async(
            password,
            hash_for(row.as_ref().map(|r| r.access_secret_hash.as_str())),
        )
        .await;

        let Some(row) = row else {
            log_login_refused(LoginRefusal::UnknownName, None);
            return Err(AuthError::InvalidCredentials {
                reason: LoginRefusal::UnknownName,
                user_id: None,
            });
        };
        // Every refusal of a known account is built here, so each logs its one line.
        let user_id = row.id.to_string();
        let refused = |reason| {
            log_login_refused(reason, Some(&user_id));
            AuthError::InvalidCredentials {
                reason,
                user_id: Some(user_id.clone()),
            }
        };

        if !row.is_active {
            return Err(refused(LoginRefusal::Deactivated));
        }

        // Check lockout: 3 consecutive failures → locked for 15 minutes. Even a correct
        // password is refused until it ends, and a wrong one neither counts nor extends it.
        let mut failures_before = row.failed_login_attempts;
        if let Some(locked_until) = row.locked_until {
            if Utc::now() < locked_until {
                return Err(refused(LoginRefusal::Locked));
            }
            // Lockout expired: clear it so the attempt proceeds, and count from zero: the
            // stored count is the one that set the lock, so a typo after the wait would
            // otherwise lock the account again at once.
            failures_before = 0;
            let _ = sqlx::query(
                "UPDATE users SET failed_login_attempts = 0, locked_until = NULL WHERE id = $1",
            )
            .bind(row.id)
            .execute(&self.db)
            .await;
        }

        if !password_ok {
            let attempts = failures_before + 1;
            if attempts >= LOGIN_LOCKOUT_THRESHOLD {
                let _ = sqlx::query(
                    "UPDATE users SET failed_login_attempts = $2, \
                     locked_until = now() + interval '15 minutes' WHERE id = $1",
                )
                .bind(row.id)
                .bind(attempts)
                .execute(&self.db)
                .await;
                return Err(refused(LoginRefusal::Locked));
            }
            let _ = sqlx::query("UPDATE users SET failed_login_attempts = $2 WHERE id = $1")
                .bind(row.id)
                .bind(attempts)
                .execute(&self.db)
                .await;
            return Err(refused(LoginRefusal::WrongPassword));
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
        log_login_ok(&identity.user_id);

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
        self.record_agent_token(&token, agent_uuid).await?;
        Ok(token)
    }

    async fn upsert_oauth_user(
        &self,
        provider: &str,
        provider_id: &str,
        username: &str,
        verified_email: Option<&str>,
    ) -> Result<LoginResult, AuthError> {
        // Join `users`: `user_identities.user_id` cascades only on a *hard* delete
        // (0001_schema.sql), so a soft-deleted user keeps every identity row
        // pointing at it. Reading the identity alone would hand back a
        // deprovisioned user_id, and the `is_superuser` read at the end of this
        // function — filtered on `deleted_at IS NULL` — would then fail as
        // RowNotFound, surfacing a deactivated account as an opaque 500-class
        // error. Reject here instead, before any row is touched.
        let existing: Option<(uuid::Uuid, bool, bool)> = sqlx::query_as(
            "SELECT u.id, u.is_active, (u.deleted_at IS NULL) \
             FROM user_identities ui JOIN users u ON u.id = ui.user_id \
             WHERE ui.provider = $1 AND ui.provider_id = $2",
        )
        .bind(provider)
        .bind(provider_id)
        .fetch_optional(&self.db)
        .await?;

        let user_id = if let Some((uid, is_active, not_deleted)) = existing {
            // Same bar as the password path (`login` above): a deactivated or
            // deprovisioned account cannot trade an IdP assertion for a token.
            if !not_deleted || !is_active {
                return Err(AuthError::Disabled);
            }
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
            // `is_active` comes back with the id rather than being folded into the
            // WHERE: a deactivated owner of this email must be *rejected*, not
            // skipped. Filtering it out here would fall through to the INSERT
            // below and trip the UNIQUE `email` constraint instead.
            let linked: Option<(uuid::Uuid, bool)> =
                match verified_email {
                    Some(email) => sqlx::query_as(
                        "SELECT id, is_active FROM users WHERE email = $1 AND deleted_at IS NULL",
                    )
                    .bind(email)
                    .fetch_optional(&self.db)
                    .await?,
                    None => None,
                };

            let user_id = if let Some((uid, is_active)) = linked {
                if !is_active {
                    return Err(AuthError::Disabled);
                }
                let _ = sqlx::query("UPDATE users SET last_login = now() WHERE id = $1")
                    .bind(uid)
                    .execute(&self.db)
                    .await;
                uid
            } else {
                // Brand-new user. Never adopt an existing row because its username
                // matches: the username comes from the identity provider and is
                // chosen by whoever owns that account, so matching on it would
                // hand a local account (a seeded admin, say) to anyone who
                // registers the same login. Linking to an existing user goes only
                // through the verified-email branch above.
                self.insert_oauth_user(username, provider, verified_email)
                    .await?
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
        //
        // `fetch_optional` + `Disabled`, never `fetch_one`: this read is filtered
        // on `deleted_at IS NULL`, so `fetch_one` turns every deprovisioned
        // account into `Database(RowNotFound)` — a 500-class error for what is an
        // ordinary, expected rejection. The branches above reject early; this is
        // the backstop that keeps any future one from reintroducing the crash.
        let live: Option<(bool, bool, String)> = sqlx::query_as(
            "SELECT is_superuser, is_active, username FROM users WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(user_id)
        .fetch_optional(&self.db)
        .await?;

        let (is_superuser, is_active, stored_username) = live.ok_or(AuthError::Disabled)?;
        if !is_active {
            return Err(AuthError::Disabled);
        }

        // The account's own username, not the provider's: they differ when the
        // account was created under a de-duplicated name or linked by email.
        let username = stored_username.as_str();
        let identity = Identity {
            user_id: user_id.to_string(),
            username: username.to_owned(),
            is_superuser,
        };

        let token = self.issue_token(&identity).await?;
        self.record_token(&token, user_id).await?;

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

    async fn would_create_user(
        &self,
        provider: &str,
        provider_id: &str,
        verified_email: Option<&str>,
    ) -> Result<bool, AuthError> {
        let existing = self
            .resolves_to_existing_user(provider, provider_id, verified_email)
            .await?;
        Ok(!existing)
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
        self.record_token(token, user_uuid).await
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

    /// OSS access rule: owner ∪ public ∪ user-grant (superuser sees all), where no grant reaches a
    /// harness agent ([`crate::sharing`]).
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
        !self
            .accessible_agents(identity, &[agent_uuid])
            .await
            .is_empty()
    }

    async fn accessible_agents(
        &self,
        identity: &Identity,
        agent_ids: &[uuid::Uuid],
    ) -> Vec<uuid::Uuid> {
        if identity.is_superuser {
            return agent_ids.to_vec();
        }
        let Ok(user_uuid) = identity.user_id.parse::<uuid::Uuid>() else {
            return Vec::new();
        };

        sqlx::query_scalar::<_, uuid::Uuid>(&format!(
            r#"SELECT a.id FROM agents a
                WHERE a.id = ANY($2)
                  AND a.deleted_at IS NULL
                  AND (
                      a.owner_id = $1
                      OR a.is_public = TRUE
                      OR ({grants_reach} AND EXISTS (
                          SELECT 1 FROM agent_grants ag
                          WHERE ag.agent_id = a.id
                            AND (
                                (ag.grant_type = 'public' AND ag.grantee_id = '*')
                             OR (ag.grant_type = 'user'   AND ag.grantee_id = $1::text)
                            )
                      ))
                  )"#,
            grants_reach = crate::sharing::grants_reach_sql("a"),
        ))
        .bind(user_uuid)
        .bind(agent_ids)
        .fetch_all(&self.db)
        .await
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::{DUMMY_HASH, hash_for};

    #[test]
    fn hash_for_nothing_is_the_dummy_hash_and_the_dummy_hash_verifies_nothing_typed() {
        assert_eq!(hash_for(None), DUMMY_HASH);
        assert_eq!(hash_for(Some("$2b$12$stored")), "$2b$12$stored");

        // Cost 12 and well formed, so verifying against it takes the same work as a real
        // account's hash. An unparseable hash would answer `Err` here, and fast.
        assert_eq!(DUMMY_HASH.split('$').nth(2), Some("12"));
        for typed in ["", "password", "changeme", DUMMY_HASH] {
            assert!(
                matches!(bcrypt::verify(typed, DUMMY_HASH), Ok(false)),
                "the dummy hash must verify no typed password"
            );
        }
    }

    #[test]
    fn the_dummy_hash_has_the_cost_of_a_real_account_hash() {
        // `hash_password` is what makes a real account's hash. If its cost changes, the dummy
        // must follow, or an unknown name would answer faster (or slower) than a known one.
        let real = crate::hash_password("any password").expect("a hash");
        assert_eq!(
            DUMMY_HASH.split('$').nth(2),
            real.split('$').nth(2),
            "the dummy hash's cost differs from hash_password's"
        );
    }
}
