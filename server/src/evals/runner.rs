//! The batch runner replay evals run on, held in `AppState` and set once at startup.
//!
//! The state is built before the edition decides which container backend runs batch jobs, so
//! it starts with one that refuses ([`UnavailableBatchRunner`]) and the composition root
//! installs the real one: Docker in OSS ([`connect_docker_runner`]), Kubernetes in EE.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use nasiko_config::Config;
use nasiko_runtime::{BatchJob, BatchOutcome, BatchRunner, Result, UnavailableBatchRunner};

/// A [`BatchRunner`] whose backend the composition root installs after the state is built.
pub struct SwappableBatchRunner(RwLock<Arc<dyn BatchRunner>>);

impl SwappableBatchRunner {
    /// Starts refusing every job, naming why.
    pub fn unavailable(reason: &str) -> Self {
        Self(RwLock::new(Arc::new(UnavailableBatchRunner::new(reason))))
    }

    /// Replaces the runner for every clone of the state this cell lives in.
    pub fn install(&self, runner: Arc<dyn BatchRunner>) {
        *self.0.write().expect("batch runner lock poisoned") = runner;
    }

    fn current(&self) -> Arc<dyn BatchRunner> {
        self.0.read().expect("batch runner lock poisoned").clone()
    }
}

#[async_trait]
impl BatchRunner for SwappableBatchRunner {
    fn is_available(&self) -> bool {
        self.current().is_available()
    }

    async fn build_image(&self, context_tar: &[u8], tag: &str) -> Result<String> {
        self.current().build_image(context_tar, tag).await
    }

    async fn run(&self, job: BatchJob) -> Result<BatchOutcome> {
        self.current().run(job).await
    }

    async fn cleanup(&self, label: Option<&str>) -> Result<usize> {
        self.current().cleanup(label).await
    }
}

/// The Docker batch runner for this server, when evals are enabled: its gateway forwards to
/// `EVAL_ROUTER_URL` over the agents' network (`DOCKER_AGENT_NETWORK`).
pub async fn connect_docker_runner(config: &Config) -> Result<Arc<dyn BatchRunner>> {
    let runtime =
        nasiko_runtime::DockerRuntime::new(crate::runtime::docker_runtime_config(config)).await?;
    Ok(Arc::new(nasiko_runtime::DockerBatchRunner::new(
        runtime,
        nasiko_runtime::DockerBatchConfig {
            router_upstream: config.evals.router_url.clone(),
            upstream_network: config.docker_agent_network.clone(),
            ..Default::default()
        },
    )))
}
