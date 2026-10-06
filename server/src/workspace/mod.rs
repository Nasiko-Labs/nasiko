//! Workspace artifacts: the object store the control plane serves them from.

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
/// The addressing mode travels with those credentials because in shared mode both
/// stores address the *same bucket*: a deployment that sets `S3_FORCE_PATH_STYLE=false`
/// for real AWS or DigitalOcean Spaces would otherwise leave the registry on
/// virtual-hosted addressing and the workspace on path-style. An explicit URL passes
/// `None` and lets `object_store`'s own `AWS_VIRTUAL_HOSTED_STYLE_REQUEST` govern.
pub async fn store_from_config(
    config: &Config,
) -> Result<Arc<dyn WorkspaceObjectStore>, WorkspaceStoreError> {
    let s3 = config.workspace_store_shares_oci_s3.then(|| S3Settings {
        endpoint: Some(config.s3_endpoint.clone()),
        access_key_id: Some(config.s3_access_key.clone()),
        secret_access_key: Some(config.s3_secret_key.clone()),
        region: Some(config.s3_region.clone()),
        // `S3_FORCE_PATH_STYLE` has no `Config` field — the OCI store reads it
        // straight from the environment, so reuse that one source of truth
        // rather than re-parsing the variable here.
        force_path_style: Some(nasiko_oci::storage::force_path_style_from_env()),
    });
    build_from_url_with(&config.workspace_store_url, s3.as_ref())
        .await
        .map(Arc::from)
}
