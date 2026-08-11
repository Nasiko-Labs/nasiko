use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Claims extracted from gateway-injected headers — local to cp-lib so we can impl axum extractors.
/// Mirrors the shared `nasiko_auth::Identity`: no enterprise fields (role/team/dept).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub is_superuser: bool,
}

impl Claims {
    /// Parse the subject (`sub`) as the caller's user UUID.
    ///
    /// A subject that doesn't parse means the gateway forwarded a malformed
    /// identity, so the caller is treated as unauthenticated (401). This is the
    /// single place that parse happens — callers must **never** coerce a bad
    /// subject to `Uuid::nil()`, which would silently authorize every failed
    /// parse as the all-zero user (an authorization bypass).
    pub fn user_uuid(&self) -> Result<Uuid, (StatusCode, &'static str)> {
        self.sub
            .parse::<Uuid>()
            .map_err(|_| (StatusCode::UNAUTHORIZED, "invalid user identity"))
    }
}

impl From<nasiko_auth::Identity> for Claims {
    fn from(c: nasiko_auth::Identity) -> Self {
        Self {
            sub: c.user_id,
            username: c.username,
            is_superuser: c.is_superuser,
        }
    }
}

impl From<Claims> for nasiko_auth::Identity {
    fn from(c: Claims) -> Self {
        Self {
            user_id: c.sub,
            username: c.username,
            is_superuser: c.is_superuser,
        }
    }
}
