use serde::{Deserialize, Serialize};

/// Organizational role — determines what a user can do.
/// The hierarchy is: Admin > DepartmentManager > TeamLead > TeamMember > Member
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Member,
    TeamMember,
    TeamLead,
    DepartmentManager,
    Admin,
}

/// Identity extracted after authentication.
/// This is the canonical user representation that flows through the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub user_id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub is_superuser: bool,
    #[serde(default)]
    pub team_id: Option<String>,
    #[serde(default)]
    pub department_id: Option<String>,
    #[serde(default)]
    pub role: Option<Role>,
    // Legacy JWT-style fields (used by server auth middleware during transition)
    #[serde(default)]
    pub sub: String,
    #[serde(default)]
    pub exp: u64,
    #[serde(default)]
    pub iat: u64,
}

/// Authentication: validate a token and extract identity.
/// OSS uses SingleUserAuth (always superuser).
/// EE uses JwtAuthProvider (HS256 JWT + role hierarchy).
pub trait AuthProvider: Send + Sync + 'static {
    fn validate_token(&self, token: &str) -> Result<Identity, AuthError>;
}

/// Authorization: what can this identity do?
/// OSS uses NoopAuthorizer (allow all).
/// EE uses HierarchyAuthorizer (role/grant-based ACL with
/// admin > department_manager > team_lead > team_member > member hierarchy).
pub trait Authorizer: Send + Sync + 'static {
    fn can_access_agent(&self, identity: &Identity, agent_id: &str) -> bool;
    fn can_deploy(&self, identity: &Identity) -> bool;
    fn can_manage_secrets(&self, identity: &Identity) -> bool;
    fn can_discover_agent(&self, identity: &Identity, agent_id: &str) -> bool;
    fn can_manage_users(&self, identity: &Identity) -> bool;
    fn can_manage_pool(&self, identity: &Identity) -> bool;
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("missing token")]
    MissingToken,
    #[error("invalid token: {0}")]
    InvalidToken(String),
    #[error("expired token")]
    Expired,
}

// ─── OSS default implementations ────────────────────────────────────────────

/// Single-user auth: any token (or no token) is accepted as superuser.
/// No teams, no roles, no database — just always admin.
pub struct SingleUserAuth;

impl AuthProvider for SingleUserAuth {
    fn validate_token(&self, _token: &str) -> Result<Identity, AuthError> {
        Ok(Identity {
            user_id: "00000000-0000-0000-0000-000000000000".to_string(),
            sub: "00000000-0000-0000-0000-000000000000".to_string(),
            exp: u64::MAX,
            iat: 0,
            username: "admin".to_string(),
            is_superuser: true,
            team_id: None,
            department_id: None,
            role: Some(Role::Admin),
        })
    }
}

/// No-op authorizer: everything is allowed.
pub struct NoopAuthorizer;

impl Authorizer for NoopAuthorizer {
    fn can_access_agent(&self, _identity: &Identity, _agent_id: &str) -> bool {
        true
    }

    fn can_deploy(&self, _identity: &Identity) -> bool {
        true
    }

    fn can_manage_secrets(&self, _identity: &Identity) -> bool {
        true
    }

    fn can_discover_agent(&self, _identity: &Identity, _agent_id: &str) -> bool {
        true
    }

    fn can_manage_users(&self, _identity: &Identity) -> bool {
        true
    }

    fn can_manage_pool(&self, _identity: &Identity) -> bool {
        true
    }
}

// ─── Backwards-compatible aliases (used by server during transition) ─────────

/// Legacy re-export — server code uses Claims; will be migrated to Identity.
pub type Claims = Identity;

impl Identity {
    // Legacy accessors for server code that reads JWT-style fields
    pub fn sub(&self) -> &str { &self.user_id }
}

/// Legacy alias — server code uses AclChecker; will be migrated to Authorizer.
pub trait AclChecker: Authorizer {}
impl<T: Authorizer> AclChecker for T {}

/// Legacy alias — NoopAcl is the old name for NoopAuthorizer.
pub type NoopAcl = NoopAuthorizer;
