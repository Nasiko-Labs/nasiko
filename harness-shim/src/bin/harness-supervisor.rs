//! `harness-supervisor` binary: tracing and environment wiring only. See
//! `harness_shim::supervisor` for behaviour.

use anyhow::Result;
use harness_shim::supervisor::{SupervisorConfig, run};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    run(SupervisorConfig::from_env()?, None).await
}
