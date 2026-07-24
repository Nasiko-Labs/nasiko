use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::{
    ContainerRuntime,
    error::{Result, RuntimeError},
    types::{ContainerId, DeploymentSpec, DeploymentStatus, RuntimeState, validate_build_inputs},
};

/// `ContainerRuntime` backend for benchmarking control-plane throughput without
/// real Docker/K8s scheduling cost.
///
/// `deploy`/`destroy`/`scale`/`restart`/`status`/`list` are pure in-memory
/// bookkeeping. `endpoint()` always resolves to the same shared
/// `simulated-agent` process address (see `oss/agents/simulated-agent`)
/// regardless of which agent asked — real HTTP traffic still round-trips
/// over the network to a real process, so the proxy/networking code path
/// stays honest while container startup and LLM latency are removed as
/// variables.
pub struct SimulatedRuntime {
    sim_agent_endpoint: String,
    containers: Mutex<HashMap<ContainerId, DeploymentStatus>>,
}

impl SimulatedRuntime {
    /// `sim_agent_endpoint` is the address of the shared `simulated-agent`
    /// process, e.g. `"http://127.0.0.1:9999"`.
    pub fn new(sim_agent_endpoint: impl Into<String>) -> Self {
        let sim_agent_endpoint = sim_agent_endpoint.into();
        tracing::warn!(
            %sim_agent_endpoint,
            "AGENT_RUNTIME=simulated — no real containers will be deployed; \
             agent endpoints resolve to this address. For benchmarking/load-testing only."
        );
        Self {
            sim_agent_endpoint,
            containers: Mutex::new(HashMap::new()),
        }
    }
}

fn running_status(container_id: &ContainerId, endpoint: &str, replicas: u32) -> DeploymentStatus {
    DeploymentStatus {
        container_id: container_id.clone(),
        state: if replicas == 0 {
            RuntimeState::Stopped
        } else {
            RuntimeState::Running
        },
        replicas_live: replicas,
        endpoint: Some(endpoint.to_owned()),
        message: None,
        restart_count: 0,
    }
}

#[async_trait]
impl ContainerRuntime for SimulatedRuntime {
    async fn deploy(&self, spec: &DeploymentSpec) -> Result<DeploymentStatus> {
        spec.validate()?;
        let status = running_status(
            &spec.container_id,
            &self.sim_agent_endpoint,
            spec.min_replicas,
        );
        self.containers
            .lock()
            .unwrap()
            .insert(spec.container_id.clone(), status.clone());
        Ok(status)
    }

    async fn destroy(&self, container_id: &ContainerId) -> Result<()> {
        self.containers.lock().unwrap().remove(container_id);
        Ok(())
    }

    async fn scale(&self, container_id: &ContainerId, replicas: u32) -> Result<()> {
        let mut containers = self.containers.lock().unwrap();
        let status = containers
            .get_mut(container_id)
            .ok_or_else(|| RuntimeError::ContainerNotFound(container_id.clone()))?;
        status.replicas_live = replicas;
        status.state = if replicas == 0 {
            RuntimeState::Stopped
        } else {
            RuntimeState::Running
        };
        Ok(())
    }

    async fn restart(&self, container_id: &ContainerId) -> Result<()> {
        let mut containers = self.containers.lock().unwrap();
        containers
            .get_mut(container_id)
            .ok_or_else(|| RuntimeError::ContainerNotFound(container_id.clone()))?
            .restart_count += 1;
        Ok(())
    }

    async fn status(&self, container_id: &ContainerId) -> Result<DeploymentStatus> {
        let containers = self.containers.lock().unwrap();
        Ok(containers
            .get(container_id)
            .cloned()
            .unwrap_or_else(|| DeploymentStatus {
                container_id: container_id.clone(),
                state: RuntimeState::Unknown,
                replicas_live: 0,
                endpoint: None,
                message: None,
                restart_count: 0,
            }))
    }

    async fn list(&self) -> Result<Vec<DeploymentStatus>> {
        Ok(self.containers.lock().unwrap().values().cloned().collect())
    }

    async fn endpoint(&self, container_id: &ContainerId) -> Result<String> {
        let containers = self.containers.lock().unwrap();
        if !containers.contains_key(container_id) {
            return Err(RuntimeError::ContainerNotFound(container_id.clone()));
        }
        Ok(self.sim_agent_endpoint.clone())
    }

    async fn logs(&self, _container_id: &ContainerId, _tail: u32) -> Result<Vec<String>> {
        Ok(vec![
            "[simulated] SimulatedRuntime runs no real process — see simulated-agent logs instead"
                .to_owned(),
        ])
    }

    async fn build(&self, tar_context: &[u8], image_tag: &str) -> Result<String> {
        validate_build_inputs(tar_context, image_tag)?;
        Ok(image_tag.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DeploymentSpec;

    fn spec(id: &str) -> DeploymentSpec {
        DeploymentSpec {
            container_id: ContainerId::new(id),
            name: id.to_owned(),
            image: "example/agent:latest".to_owned(),
            min_replicas: 1,
            max_replicas: 1,
            env_vars: HashMap::new(),
            ports: vec![8080],
            resources: None,
            image_pull_secret_name: None,
            image_pull_credential_seed: None,
            harden: false,
            network_override: None,
            workload_kind: Default::default(),
        }
    }

    #[tokio::test]
    async fn deploy_then_endpoint_resolves_to_shared_simulated_agent() {
        let runtime = SimulatedRuntime::new("http://127.0.0.1:9999");
        let s = spec("agent-a");
        runtime.deploy(&s).await.unwrap();
        assert_eq!(
            runtime.endpoint(&s.container_id).await.unwrap(),
            "http://127.0.0.1:9999"
        );

        let s2 = spec("agent-b");
        runtime.deploy(&s2).await.unwrap();
        assert_eq!(
            runtime.endpoint(&s2.container_id).await.unwrap(),
            "http://127.0.0.1:9999"
        );
    }

    #[tokio::test]
    async fn endpoint_before_deploy_is_container_not_found() {
        let runtime = SimulatedRuntime::new("http://127.0.0.1:9999");
        let id = ContainerId::new("missing");
        assert!(matches!(
            runtime.endpoint(&id).await,
            Err(RuntimeError::ContainerNotFound(_))
        ));
    }

    #[tokio::test]
    async fn destroy_removes_from_list() {
        let runtime = SimulatedRuntime::new("http://127.0.0.1:9999");
        let s = spec("agent-c");
        runtime.deploy(&s).await.unwrap();
        assert_eq!(runtime.list().await.unwrap().len(), 1);
        runtime.destroy(&s.container_id).await.unwrap();
        assert_eq!(runtime.list().await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn scale_to_zero_marks_stopped() {
        let runtime = SimulatedRuntime::new("http://127.0.0.1:9999");
        let s = spec("agent-d");
        runtime.deploy(&s).await.unwrap();
        runtime.scale(&s.container_id, 0).await.unwrap();
        let status = runtime.status(&s.container_id).await.unwrap();
        assert_eq!(status.state, RuntimeState::Stopped);
        assert_eq!(status.replicas_live, 0);
    }
}
