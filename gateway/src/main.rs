use std::sync::Arc;

use pingora_core::prelude::*;
use pingora_proxy::http_proxy_service;


use nasiko_gateway::config::GatewayConfig;
use nasiko_gateway::proxy::GatewayProxy;
use nasiko_gateway::tls::build_tls_settings;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,nasiko_gateway=debug".parse().unwrap()),
        )
        .init();

    let config_path =
        std::env::var("GATEWAY_CONFIG").unwrap_or_else(|_| "gateway.json".into());

    let config = GatewayConfig::from_file(&config_path).unwrap_or_else(|e| {
        tracing::warn!("Failed to load config from {}: {}. Using example config.", config_path, e);
        GatewayConfig::example()
    });

    tracing::info!(
        listen = %config.listen_addr,
        routes = config.routes.len(),
        "Starting nasiko gateway (OSS)"
    );

    let auth_provider: Arc<dyn nasiko_auth::AuthProvider> =
        Arc::new(nasiko_auth::SimpleJwtAuth {
            secret: config.jwt_secret.clone(),
            expiry_secs: nasiko_auth::jwt::DEFAULT_EXPIRY_SECS,
        });

    let mut server = Server::new(None).unwrap();
    server.bootstrap();

    let proxy = GatewayProxy::new(&config, auth_provider);
    let mut service = http_proxy_service(&server.configuration, proxy);

    // Configure listener with optional TLS
    match build_tls_settings(&config.tls) {
        Some(tls_settings) => {
            service.add_tls_with_settings(&config.listen_addr, None, tls_settings);
            tracing::info!("TLS enabled");
        }
        None => {
            service.add_tcp(&config.listen_addr);
            tracing::info!("Running in plaintext mode (no TLS)");
        }
    }

    server.add_service(service);
    server.run_forever();
}
