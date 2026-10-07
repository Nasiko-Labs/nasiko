//! Workspace artifacts: the object store the control plane serves them from.

pub mod acl;
pub mod catalog;
pub mod paths;

use std::sync::Arc;

use nasiko_config::Config;
use nasiko_workspace_store::{
    S3Settings, WorkspaceObjectStore, WorkspaceStoreError, build_from_url_with,
};

/// Builds the workspace object store every composition root receives. The derived
/// default lives inside the OCI bucket and therefore reuses the OCI store's `S3_*`
/// settings; an explicit `WORKSPACE_STORE_URL` authenticates from the provider's
/// own standard environment.
///
/// The endpoint and the addressing mode travel with those credentials because in
/// shared mode both stores address the *same bucket*: a deployment that sets
/// `S3_FORCE_PATH_STYLE=false` for real AWS or DigitalOcean Spaces would otherwise
/// leave the registry on virtual-hosted addressing and the workspace on path-style.
/// An explicit URL passes `None` and lets `object_store`'s own `AWS_ENDPOINT` and
/// `AWS_VIRTUAL_HOSTED_STYLE_REQUEST` govern.
pub async fn store_from_config(
    config: &Config,
) -> Result<Arc<dyn WorkspaceObjectStore>, WorkspaceStoreError> {
    let s3 = shared_s3_settings(config, registry_s3_endpoint());
    build_from_url_with(&config.workspace_store_url, s3.as_ref())
        .await
        .map(Arc::from)
}

/// The endpoint the embedded OCI registry resolves, read the way the registry
/// reads it: `S3_ENDPOINT` straight from the environment, with **unset meaning
/// real AWS** rather than any default (`nasiko_oci::storage::S3Storage::from_env`
/// applies an override only when the variable is present).
///
/// `Config::s3_endpoint` must not be used here. It carries the development
/// default `http://localhost:9000`, so on a real-AWS deployment the registry
/// would address `s3.<region>.amazonaws.com` while the workspace store addressed
/// a local MinIO — and because building an S3 client does no I/O, the server
/// would boot green and only fail, or silently write to the wrong place, on the
/// first transfer.
fn registry_s3_endpoint() -> Option<String> {
    std::env::var("S3_ENDPOINT").ok().filter(|s| !s.is_empty())
}

/// The `S3_*` settings to bridge into the workspace store, or `None` when the
/// store is not sharing the OCI bucket. `endpoint` is passed in rather than read
/// here so the rule above is testable without touching process environment.
fn shared_s3_settings(config: &Config, endpoint: Option<String>) -> Option<S3Settings> {
    config.workspace_store_shares_oci_s3.then(|| S3Settings {
        endpoint,
        access_key_id: Some(config.s3_access_key.clone()),
        secret_access_key: Some(config.s3_secret_key.clone()),
        region: Some(config.s3_region.clone()),
        force_path_style: Some(config.s3_force_path_style),
    })
}

#[cfg(test)]
mod tests {
    use super::{registry_s3_endpoint, shared_s3_settings};
    use nasiko_config::Config;
    use serial_test::serial;

    /// `Config::from_env` needs these; the values are fixtures, not secrets.
    const REQUIRED: [(&str, &str); 5] = [
        ("DATABASE_URL", "postgres://test:test@localhost/testdb"),
        ("REDIS_URL", "redis://localhost:6379"),
        ("S3_SECRET_KEY", "test-s3-secret"),
        ("SECRETS_ENCRYPTION_KEY", "test-encryption-key"),
        ("ADMIN_PASSWORD", "test-admin-password"),
    ];

    /// SAFETY: every caller is `#[serial]`, so no other thread reads env here.
    fn with_clean_env() {
        unsafe {
            for (k, v) in REQUIRED {
                std::env::set_var(k, v);
            }
            for k in ["WORKSPACE_STORE_URL", "STORAGE_PROVIDER", "S3_ENDPOINT"] {
                std::env::remove_var(k);
            }
        }
    }

    fn clear_env() {
        unsafe {
            for (k, _) in REQUIRED {
                std::env::remove_var(k);
            }
            std::env::remove_var("S3_ENDPOINT");
        }
    }

    #[test]
    #[serial]
    fn an_unset_s3_endpoint_leaves_the_bridged_endpoint_unset() {
        with_clean_env();
        let config = Config::from_env().expect("required vars are set");

        assert!(
            config.workspace_store_shares_oci_s3,
            "no WORKSPACE_STORE_URL on an S3 provider is shared mode"
        );
        assert_eq!(
            config.s3_endpoint, "http://localhost:9000",
            "the Config field still carries its development default"
        );

        let s3 = shared_s3_settings(&config, registry_s3_endpoint()).expect("shared mode bridges");
        assert_eq!(
            s3.endpoint, None,
            "unset S3_ENDPOINT must leave object_store's own AWS resolution in \
             place — handing it the Config default would point the workspace \
             store at a local MinIO while the registry talks to real AWS"
        );

        clear_env();
    }

    #[test]
    #[serial]
    fn a_set_s3_endpoint_is_bridged_verbatim_with_the_configured_addressing() {
        with_clean_env();
        unsafe { std::env::set_var("S3_ENDPOINT", "http://minio.internal:9000") };
        let config = Config::from_env().expect("required vars are set");

        let s3 = shared_s3_settings(&config, registry_s3_endpoint()).expect("shared mode bridges");
        assert_eq!(s3.endpoint.as_deref(), Some("http://minio.internal:9000"));
        assert_eq!(s3.force_path_style, Some(config.s3_force_path_style));
        assert_eq!(s3.region.as_deref(), Some(config.s3_region.as_str()));

        clear_env();
    }

    /// `Config::s3_force_path_style` replicates the registry's parse instead of
    /// calling it (`nasiko-config` must stay a leaf crate), so this pins the two
    /// against each other — the right-hand side *is* the registry's function, not
    /// a copy of its logic — for every shape the variable can take: unset, the two
    /// spellings that disable path-style, and values that must leave it on.
    #[test]
    #[serial]
    fn config_and_the_oci_registry_agree_on_path_style() {
        with_clean_env();
        for raw in [
            None,
            Some("false"),
            Some("0"),
            Some(" false "),
            Some("true"),
            Some("garbage"),
        ] {
            unsafe {
                match raw {
                    Some(raw) => std::env::set_var("S3_FORCE_PATH_STYLE", raw),
                    None => std::env::remove_var("S3_FORCE_PATH_STYLE"),
                }
            }
            let config = Config::from_env().expect("required vars are set");
            assert_eq!(
                config.s3_force_path_style,
                nasiko_oci::storage::force_path_style_from_env(),
                "Config replicates the OCI registry's parse; they must not diverge \
                 (S3_FORCE_PATH_STYLE={raw:?})"
            );
        }

        unsafe { std::env::remove_var("S3_FORCE_PATH_STYLE") };
        clear_env();
    }

    #[test]
    #[serial]
    fn an_explicit_store_url_bridges_nothing() {
        with_clean_env();
        unsafe { std::env::set_var("WORKSPACE_STORE_URL", "gs://elsewhere/artifacts") };
        let config = Config::from_env().expect("required vars are set");

        assert!(
            shared_s3_settings(&config, registry_s3_endpoint()).is_none(),
            "an explicit URL authenticates from the provider's own environment"
        );

        unsafe { std::env::remove_var("WORKSPACE_STORE_URL") };
        clear_env();
    }
}
