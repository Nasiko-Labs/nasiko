use async_trait::async_trait;
use pingora_core::prelude::*;
use pingora_core::upstreams::peer::HttpPeer;
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::{ProxyHttp, Session};
use std::sync::Arc;

use nasiko_auth::{
    Identity, HEADER_USER_ID, HEADER_USERNAME, HEADER_IS_SUPERUSER, HEADER_USER_ROLE,
    HEADER_TEAM_ID, HEADER_DEPT_ID, TRUST_HEADERS,
};

use crate::auth::{AuthError, GatewayAuth};
use crate::config::GatewayConfig;
use crate::rate_limit::RateLimiter;
use crate::routing::{RouteMatch, Router};
use crate::translation::Translator;

/// Per-request context passed through the Pingora filter chain.
pub struct GatewayCtx {
    pub route_match: Option<RouteMatch>,
    pub client_ip: Option<String>,
    pub identity: Option<Identity>,
}

/// The main gateway service implementing Pingora's ProxyHttp trait.
pub struct GatewayProxy {
    pub auth: Arc<GatewayAuth>,
    pub router: Arc<Router>,
    pub rate_limiter: Arc<RateLimiter>,
    pub translator: Arc<Translator>,
    pub cors: Arc<CorsHandler>,
}

pub struct CorsHandler {
    pub allowed_origins: Vec<String>,
    pub allowed_methods: String,
    pub allowed_headers: String,
}

impl CorsHandler {
    pub fn from_config(config: &crate::config::CorsConfig) -> Self {
        Self {
            allowed_origins: config.allowed_origins.clone(),
            allowed_methods: config.allowed_methods.join(", "),
            allowed_headers: config.allowed_headers.join(", "),
        }
    }

    pub fn origin_allowed(&self, origin: &str) -> bool {
        self.allowed_origins.iter().any(|o| o == "*" || o == origin)
    }
}

impl GatewayProxy {
    pub fn new(config: &GatewayConfig, auth_provider: Arc<dyn nasiko_auth::AuthProvider>) -> Self {
        Self {
            auth: Arc::new(GatewayAuth::new(auth_provider)),
            router: Arc::new(Router::new(&config.routes)),
            rate_limiter: Arc::new(RateLimiter::new(&config.rate_limits)),
            translator: Arc::new(Translator::default_translator()),
            cors: Arc::new(CorsHandler::from_config(&config.cors)),
        }
    }
}

#[async_trait]
impl ProxyHttp for GatewayProxy {
    type CTX = GatewayCtx;

    fn new_ctx(&self) -> Self::CTX {
        GatewayCtx {
            route_match: None,
            client_ip: None,
            identity: None,
        }
    }

    async fn request_filter(
        &self,
        session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<bool> {
        // Strip trust headers from client requests — gateway is the only one that sets these
        for trust_header in TRUST_HEADERS {
            session.req_header_mut().remove_header(*trust_header);
        }

        let header = session.req_header();

        // Extract client IP
        ctx.client_ip = session
            .client_addr()
            .map(|addr| addr.to_string())
            .or_else(|| {
                header
                    .headers
                    .get("x-forwarded-for")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
            });

        let path = header.uri.path();

        // Handle CORS preflight
        if header.method == http::Method::OPTIONS {
            let mut resp = ResponseHeader::build(204, None)?;
            if let Some(origin) = header.headers.get("origin").and_then(|v| v.to_str().ok()) {
                if self.cors.origin_allowed(origin) {
                    resp.insert_header("Access-Control-Allow-Origin", origin)?;
                    resp.insert_header("Access-Control-Allow-Methods", &self.cors.allowed_methods)?;
                    resp.insert_header("Access-Control-Allow-Headers", &self.cors.allowed_headers)?;
                    resp.insert_header("Access-Control-Max-Age", "86400")?;
                }
            }
            session.write_response_header(Box::new(resp), true).await?;
            return Ok(true);
        }

        // IP-based rate limiting
        if let Some(ref ip) = ctx.client_ip {
            if self.rate_limiter.check_ip(ip).is_limited() {
                tracing::warn!(ip = %ip, "IP rate limited");
                let resp = ResponseHeader::build(429, None)?;
                session.write_response_header(Box::new(resp), true).await?;
                return Ok(true);
            }
        }

        // Route resolution
        let route_match = match self.router.resolve(path) {
            Some(m) => m,
            None => {
                let resp = ResponseHeader::build(404, None)?;
                session.write_response_header(Box::new(resp), true).await?;
                return Ok(true);
            }
        };

        // Auth check
        if route_match.require_auth {
            match self.auth.extract_and_validate(header).await {
                Ok(identity) => {
                    // User-based rate limiting
                    if self.rate_limiter.check_user(&identity.user_id).is_limited() {
                        tracing::warn!(user = %identity.user_id, "User rate limited");
                        let resp = ResponseHeader::build(429, None)?;
                        session.write_response_header(Box::new(resp), true).await?;
                        return Ok(true);
                    }

                    // Role check
                    if let Some(ref required_role) = route_match.required_role {
                        if !GatewayAuth::check_role(&identity, required_role) {
                            let resp = ResponseHeader::build(403, None)?;
                            session.write_response_header(Box::new(resp), true).await?;
                            return Ok(true);
                        }
                    }

                    ctx.identity = Some(identity);
                }
                Err(AuthError::MissingToken) | Err(AuthError::Expired) | Err(AuthError::InvalidToken(_)) => {
                    let resp = ResponseHeader::build(401, None)?;
                    session.write_response_header(Box::new(resp), true).await?;
                    return Ok(true);
                }
                Err(_) => {
                    let resp = ResponseHeader::build(403, None)?;
                    session.write_response_header(Box::new(resp), true).await?;
                    return Ok(true);
                }
            }
        }

        ctx.route_match = Some(route_match);
        Ok(false)
    }

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        let route = ctx.route_match.as_ref().ok_or_else(|| {
            pingora_core::Error::new(pingora_core::ErrorType::HTTPStatus(502))
        })?;

        let peer = HttpPeer::new(
            (&*route.upstream_host, route.upstream_port),
            false, // plaintext to backends
            String::new(),
        );

        Ok(Box::new(peer))
    }

    async fn upstream_request_filter(
        &self,
        _session: &mut Session,
        upstream_request: &mut RequestHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        // Rewrite path if needed
        if let Some(ref route) = ctx.route_match {
            if upstream_request.uri.path() != route.rewritten_path {
                let new_uri = http::Uri::builder()
                    .path_and_query(route.rewritten_path.as_str())
                    .build()
                    .map_err(|e| {
                        pingora_core::Error::because(
                            pingora_core::ErrorType::HTTPStatus(500),
                            "failed to build URI",
                            e,
                        )
                    })?;
                upstream_request.set_uri(new_uri);
            }
        }

        // Apply translation rules
        self.translator
            .translate_request(upstream_request, ctx.client_ip.as_deref());

        // Forward full identity to backend server
        if let Some(ref identity) = ctx.identity {
            upstream_request.insert_header(HEADER_USER_ID, identity.user_id.as_str())?;
            upstream_request.insert_header(HEADER_USERNAME, identity.username.as_str())?;
            upstream_request.insert_header(
                HEADER_IS_SUPERUSER,
                if identity.is_superuser { "true" } else { "false" },
            )?;
            if let Some(ref role) = identity.role {
                let role_str = serde_json::to_value(role)
                    .ok()
                    .and_then(|v| v.as_str().map(|s| s.to_owned()))
                    .unwrap_or_default();
                if !role_str.is_empty() {
                    upstream_request.insert_header(HEADER_USER_ROLE, role_str.as_str())?;
                }
            }
            if let Some(ref team_id) = identity.team_id {
                upstream_request.insert_header(HEADER_TEAM_ID, team_id.as_str())?;
            }
            if let Some(ref dept_id) = identity.department_id {
                upstream_request.insert_header(HEADER_DEPT_ID, dept_id.as_str())?;
            }
        }

        Ok(())
    }

    async fn response_filter(
        &self,
        session: &mut Session,
        upstream_response: &mut ResponseHeader,
        _ctx: &mut Self::CTX,
    ) -> Result<()> {
        // Add CORS headers to responses
        if let Some(origin) = session
            .req_header()
            .headers
            .get("origin")
            .and_then(|v| v.to_str().ok())
        {
            if self.cors.origin_allowed(origin) {
                upstream_response.insert_header("Access-Control-Allow-Origin", origin)?;
            }
        }

        // Security headers
        upstream_response.insert_header("X-Content-Type-Options", "nosniff")?;
        upstream_response.insert_header("X-Frame-Options", "DENY")?;

        Ok(())
    }
}
