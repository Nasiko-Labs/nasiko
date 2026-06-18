use crate::config::TlsConfig;
use pingora_core::listeners::tls::TlsSettings;
use std::path::Path;

/// Configure TLS for the gateway listener.
/// Returns None if no TLS config is provided (plaintext mode).
pub fn build_tls_settings(config: &Option<TlsConfig>) -> Option<TlsSettings> {
    let tls = config.as_ref()?;

    let cert_path = Path::new(&tls.cert_path);
    let key_path = Path::new(&tls.key_path);

    if !cert_path.exists() {
        tracing::error!("TLS cert not found: {}", tls.cert_path);
        return None;
    }
    if !key_path.exists() {
        tracing::error!("TLS key not found: {}", tls.key_path);
        return None;
    }

    let mut settings = TlsSettings::intermediate(&tls.cert_path, &tls.key_path)
        .expect("failed to load TLS cert/key");
    settings.enable_h2();

    Some(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_none_when_no_config() {
        let result = build_tls_settings(&None);
        assert!(result.is_none());
    }

    #[test]
    fn returns_none_for_missing_cert() {
        let config = Some(TlsConfig {
            cert_path: "/nonexistent/cert.pem".into(),
            key_path: "/nonexistent/key.pem".into(),
        });
        let result = build_tls_settings(&config);
        assert!(result.is_none());
    }
}
