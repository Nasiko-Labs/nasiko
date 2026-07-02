use serde::{Deserialize, Serialize};

/// Claims extracted from gateway-injected headers — local to cp-lib so we can impl axum extractors.
use nasiko_auth::Role;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub is_superuser: bool,
    #[serde(default)]
    pub role: Option<Role>,
}

impl From<nasiko_auth::Identity> for Claims {
    fn from(c: nasiko_auth::Identity) -> Self {
        Self {
            sub: c.user_id,
            username: c.username,
            is_superuser: c.is_superuser,
            role: c.role,
        }
    }
}

impl From<Claims> for nasiko_auth::Identity {
    fn from(c: Claims) -> Self {
        Self {
            user_id: c.sub,
            username: c.username,
            is_superuser: c.is_superuser,
            role: c.role,
        }
    }
}
