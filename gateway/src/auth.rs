use nasiko_auth::{AuthProvider, Identity, Role};
use pingora_http::RequestHeader;
use std::sync::Arc;

/// Gateway authentication layer. Uses a pluggable AuthProvider.
/// OSS: SingleUserAuth (always allows). EE: JwtAuthProvider (real JWT validation).
#[derive(Clone)]
pub struct GatewayAuth {
    provider: Arc<dyn AuthProvider>,
}

impl GatewayAuth {
    pub fn new(provider: Arc<dyn AuthProvider>) -> Self {
        Self { provider }
    }

    pub fn extract_and_validate(&self, header: &RequestHeader) -> Result<Identity, AuthError> {
        let token = self.extract_token(header)?;
        self.provider
            .validate_token(&token)
            .map_err(|e| match e {
                nasiko_auth::AuthError::MissingToken => AuthError::MissingToken,
                nasiko_auth::AuthError::Expired => AuthError::Expired,
                nasiko_auth::AuthError::InvalidToken(msg) => AuthError::InvalidToken(msg),
            })
    }

    fn extract_token(&self, header: &RequestHeader) -> Result<String, AuthError> {
        // Try Authorization: Bearer <token>
        if let Some(auth) = header.headers.get("authorization") {
            if let Ok(value) = auth.to_str() {
                if let Some(token) = value.strip_prefix("Bearer ") {
                    return Ok(token.to_string());
                }
            }
        }

        // Try cookie
        if let Some(cookie) = header.headers.get("cookie") {
            if let Ok(value) = cookie.to_str() {
                for part in value.split(';') {
                    if let Some(token) = part.trim().strip_prefix("access_token=") {
                        return Ok(token.to_string());
                    }
                }
            }
        }

        Err(AuthError::MissingToken)
    }

    pub fn check_role(identity: &Identity, required_role: &str) -> bool {
        if identity.is_superuser {
            return true;
        }
        let required = match required_role {
            "member" => Role::Member,
            "team_member" => Role::TeamMember,
            "team_lead" => Role::TeamLead,
            "department_manager" => Role::DepartmentManager,
            "admin" => Role::Admin,
            _ => return false,
        };
        identity.role.as_ref().is_some_and(|r| *r >= required)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("missing token")]
    MissingToken,
    #[error("invalid token: {0}")]
    InvalidToken(String),
    #[error("expired token")]
    Expired,
    #[error("insufficient permissions")]
    Forbidden,
}

#[cfg(test)]
mod tests {
    use super::*;
    use nasiko_auth::SingleUserAuth;

    #[test]
    fn single_user_auth_always_passes() {
        let auth = GatewayAuth::new(Arc::new(SingleUserAuth));
        let mut header = RequestHeader::build("GET", b"/", None).unwrap();
        header
            .insert_header("authorization", "Bearer any-token".to_string())
            .unwrap();

        let result = auth.extract_and_validate(&header);
        assert!(result.is_ok());
        let identity = result.unwrap();
        assert!(identity.is_superuser);
        assert_eq!(identity.username, "admin");
    }

    #[test]
    fn extracts_bearer_token() {
        let auth = GatewayAuth::new(Arc::new(SingleUserAuth));
        let mut header = RequestHeader::build("GET", b"/", None).unwrap();
        header
            .insert_header("authorization", "Bearer my-token-123".to_string())
            .unwrap();

        let token = auth.extract_token(&header).unwrap();
        assert_eq!(token, "my-token-123");
    }

    #[test]
    fn extracts_cookie_token() {
        let auth = GatewayAuth::new(Arc::new(SingleUserAuth));
        let mut header = RequestHeader::build("GET", b"/", None).unwrap();
        header
            .insert_header("cookie", "other=val; access_token=cookie-tok; x=y".to_string())
            .unwrap();

        let token = auth.extract_token(&header).unwrap();
        assert_eq!(token, "cookie-tok");
    }

    #[test]
    fn missing_token_error() {
        let auth = GatewayAuth::new(Arc::new(SingleUserAuth));
        let header = RequestHeader::build("GET", b"/", None).unwrap();

        assert!(matches!(
            auth.extract_token(&header),
            Err(AuthError::MissingToken)
        ));
    }

    #[test]
    fn check_role_superuser_bypasses() {
        let identity = Identity {
            user_id: "u1".into(),
            username: "admin".into(),
            is_superuser: true,
            team_id: None,
            department_id: None,
            role: None,
        };
        assert!(GatewayAuth::check_role(&identity, "admin"));
        assert!(GatewayAuth::check_role(&identity, "department_manager"));
    }

    #[test]
    fn check_role_enforces_hierarchy() {
        let member = Identity {
            user_id: "u1".into(),
            username: "user".into(),
            is_superuser: false,
            team_id: None,
            department_id: None,
            role: Some(Role::TeamMember),
        };
        assert!(GatewayAuth::check_role(&member, "member"));
        assert!(GatewayAuth::check_role(&member, "team_member"));
        assert!(!GatewayAuth::check_role(&member, "team_lead"));
        assert!(!GatewayAuth::check_role(&member, "admin"));
    }
}
