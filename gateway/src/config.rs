/// Gateway configuration, entirely from environment variables.
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub listen_addr: String,
    pub server_upstream: String,
    pub jwt_secret: String,
    pub database_url: String,
    pub redis_url: String,
}

impl GatewayConfig {
    pub fn from_env() -> Self {
        Self {
            listen_addr: std::env::var("GATEWAY_LISTEN")
                .unwrap_or_else(|_| "0.0.0.0:8443".into()),
            server_upstream: std::env::var("GATEWAY_UPSTREAM")
                .unwrap_or_else(|_| "http://127.0.0.1:8080".into()),
            jwt_secret: std::env::var("JWT_SECRET")
                .unwrap_or_else(|_| "dev-secret-change-me".into()),
            database_url: std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://nasiko:nasiko@localhost:5432/nasiko".into()),
            redis_url: std::env::var("REDIS_URL")
                .unwrap_or_else(|_| "redis://localhost:6379".into()),
        }
    }
}
