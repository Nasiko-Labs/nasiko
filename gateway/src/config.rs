use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct GatewayConfig {
    pub listen_addr: String,
    pub tls: Option<TlsConfig>,
    pub jwt_secret: String,
    pub rate_limits: RateLimitConfig,
    pub routes: Vec<RouteConfig>,
    pub cors: CorsConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TlsConfig {
    pub cert_path: String,
    pub key_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitConfig {
    pub ip_requests_per_second: isize,
    pub user_requests_per_second: isize,
    pub burst_multiplier: usize,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            ip_requests_per_second: 100,
            user_requests_per_second: 50,
            burst_multiplier: 2,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RouteConfig {
    pub path_prefix: String,
    pub upstream: String,
    #[serde(default)]
    pub strip_prefix: bool,
    #[serde(default)]
    pub require_auth: bool,
    #[serde(default)]
    pub required_role: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CorsConfig {
    #[serde(default = "default_origins")]
    pub allowed_origins: Vec<String>,
    #[serde(default = "default_methods")]
    pub allowed_methods: Vec<String>,
    #[serde(default = "default_headers")]
    pub allowed_headers: Vec<String>,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allowed_origins: default_origins(),
            allowed_methods: default_methods(),
            allowed_headers: default_headers(),
        }
    }
}

fn default_origins() -> Vec<String> {
    vec!["*".into()]
}

fn default_methods() -> Vec<String> {
    vec!["GET", "POST", "PUT", "DELETE", "OPTIONS"]
        .into_iter()
        .map(Into::into)
        .collect()
}

fn default_headers() -> Vec<String> {
    vec!["Content-Type", "Authorization", "traceparent"]
        .into_iter()
        .map(Into::into)
        .collect()
}

impl GatewayConfig {
    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = serde_json::from_str(&content)?;
        Ok(config)
    }

    pub fn example() -> Self {
        Self {
            listen_addr: "0.0.0.0:8443".into(),
            tls: None,
            jwt_secret: "dev-secret-change-me".into(),
            rate_limits: RateLimitConfig::default(),
            routes: vec![
                RouteConfig {
                    path_prefix: "/api/".into(),
                    upstream: "127.0.0.1:8080".into(),
                    strip_prefix: false,
                    require_auth: true,
                    required_role: None,
                },
                RouteConfig {
                    path_prefix: "/health".into(),
                    upstream: "127.0.0.1:8080".into(),
                    strip_prefix: false,
                    require_auth: false,
                    required_role: None,
                },
                RouteConfig {
                    path_prefix: "/.well-known/".into(),
                    upstream: "127.0.0.1:8080".into(),
                    strip_prefix: false,
                    require_auth: false,
                    required_role: None,
                },
                // LLM router (OpenAI-compatible agent egress). Strip `/llm` so
                // `/llm/v1/chat/completions` reaches the server as `/v1/...`. No edge
                // auth: the agent-identity JWT is verified inside the LLM router.
                RouteConfig {
                    path_prefix: "/llm/".into(),
                    upstream: "127.0.0.1:8080".into(),
                    strip_prefix: true,
                    require_auth: false,
                    required_role: None,
                },
            ],
            cors: CorsConfig::default(),
        }
    }
}
