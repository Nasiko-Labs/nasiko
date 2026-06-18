use serde::{Deserialize, Serialize};

/// Claims extracted from JWT — local to cp-lib so we can impl axum extractors.
/// Matches the field layout of nasiko_auth::Claims.
use nasiko_auth::Role;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: u64,
    #[serde(default)]
    pub iat: u64,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub is_superuser: bool,
    #[serde(default)]
    pub team_id: Option<String>,
    #[serde(default)]
    pub role: Option<Role>,
}

impl From<nasiko_auth::Claims> for Claims {
    fn from(c: nasiko_auth::Claims) -> Self {
        Self {
            sub: c.sub,
            exp: c.exp,
            iat: c.iat,
            username: c.username,
            is_superuser: c.is_superuser,
            team_id: c.team_id,
            role: c.role,
        }
    }
}

impl From<Claims> for nasiko_auth::Claims {
    fn from(c: Claims) -> Self {
        Self {
            user_id: c.sub.clone(),
            sub: c.sub,
            exp: c.exp,
            iat: c.iat,
            username: c.username,
            is_superuser: c.is_superuser,
            team_id: c.team_id,
            department_id: None,
            role: c.role,
        }
    }
}
