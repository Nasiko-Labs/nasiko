use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::a2a::A2aClient;

/// Metadata about a discovered A2A agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub endpoint: String,
    pub skills: Vec<AgentSkill>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
}

/// Where agents are discovered from.
#[derive(Debug, Clone)]
pub enum RegistrySource {
    /// Nasiko control plane A2A registry.
    ControlPlane {
        base_url: String,
        api_key: Option<String>,
    },
    /// Static list (useful for testing and local dev).
    Static(Vec<AgentInfo>),
}

/// Discovers and caches the set of available A2A agents.
#[derive(Clone)]
pub struct AgentRegistry {
    source: RegistrySource,
    client: A2aClient,
    cache: Arc<RwLock<Vec<AgentInfo>>>,
}

impl AgentRegistry {
    pub fn new(source: RegistrySource) -> Self {
        Self {
            source,
            client: A2aClient::new(),
            cache: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn with_client(source: RegistrySource, client: A2aClient) -> Self {
        Self {
            source,
            client,
            cache: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Refresh the agent list from the source.
    pub async fn discover(&self) -> Result<Vec<AgentInfo>, RegistryError> {
        let agents = match &self.source {
            RegistrySource::Static(list) => list.clone(),
            RegistrySource::ControlPlane { base_url, api_key } => {
                self.discover_from_cp(base_url, api_key.as_deref()).await?
            }
        };

        let mut cache = self.cache.write().await;
        *cache = agents.clone();
        Ok(agents)
    }

    /// Return cached agents.
    pub async fn agents(&self) -> Vec<AgentInfo> {
        self.cache.read().await.clone()
    }

    /// Find a specific agent by name or ID.
    pub async fn find(&self, name_or_id: &str) -> Option<AgentInfo> {
        let cache = self.cache.read().await;
        cache.iter().find(|a| a.id == name_or_id || a.name == name_or_id).cloned()
    }

    async fn discover_from_cp(
        &self,
        base_url: &str,
        _api_key: Option<&str>,
    ) -> Result<Vec<AgentInfo>, RegistryError> {
        let url = format!("{}/a2a/v1", base_url.trim_end_matches('/'));

        let response = self
            .client
            .send_message(&url, "list all agents", None)
            .await
            .map_err(|e| RegistryError::Discovery(e.to_string()))?;

        let result = response
            .result
            .ok_or_else(|| RegistryError::Discovery("empty result".into()))?;

        let agents_data = result
            .pointer("/artifacts/0/parts/0/data/agents")
            .and_then(|a| a.as_array())
            .ok_or_else(|| RegistryError::Discovery("unexpected response shape".into()))?;

        let mut agents = Vec::new();
        for val in agents_data {
            if let Ok(agent) = serde_json::from_value::<AgentInfo>(val.clone()) {
                agents.push(agent);
            } else {
                tracing::warn!("skipping unparseable agent entry: {val}");
            }
        }

        Ok(agents)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("discovery failed: {0}")]
    Discovery(String),
}
