//! Connecting a [`DockerSandboxProvider`] without the caller depending on `bollard`.
//!
//! `DockerSandboxProvider::new` takes a connected client. A composition root that only
//! links `nasiko-runtime` (the server) cannot name that type, so this connects the same way
//! `DockerRuntime::new` does (platform default socket, same ping-and-retry, 3 attempts at 100 ms / 500 ms backoff) and hands back
//! the provider.

use std::sync::Arc;
use std::time::Duration;

use bollard::Docker;

use crate::{DockerRuntimeConfig, DockerSandboxProvider, ImageSource, Result, RuntimeError};

/// Connect to the local Docker daemon and wrap it in a [`DockerSandboxProvider`].
///
/// `config` and `image_source` are the same values the deployment-shaped `DockerRuntime`
/// gets, so both pull images identically.
pub async fn connect_docker_sandbox_provider(
    config: DockerRuntimeConfig,
    image_source: Option<Arc<dyn ImageSource>>,
) -> Result<DockerSandboxProvider> {
    let client = Docker::connect_with_local_defaults()
        .map_err(|e| RuntimeError::BackendUnreachable(e.to_string()))?;
    ping_with_retry(&client).await?;
    Ok(DockerSandboxProvider::new(client, config, image_source))
}

/// Same schedule as `DockerRuntime::new`: up to 3 pings, 100 ms then 500 ms apart.
async fn ping_with_retry(client: &Docker) -> Result<()> {
    let mut last_err = String::new();
    for attempt in 0u32..3 {
        match client.ping().await {
            Ok(_) => return Ok(()),
            Err(e) => {
                last_err = e.to_string();
                if attempt < 2 {
                    let delay = if attempt == 0 { 100 } else { 500 };
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
            }
        }
    }
    Err(RuntimeError::BackendUnreachable(last_err))
}
