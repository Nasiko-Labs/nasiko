use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::a2a::A2aClient;

/// Metadata about a discovered A2A agent.
#[derive(Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub endpoint: String,
    pub skills: Vec<AgentSkill>,
    /// A header this specific endpoint requires — e.g. the EE injection proxy's own credential,
    /// when `endpoint` is the proxy's address rather than the agent's real one
    /// (`ContainerRuntime::forwarding_endpoint`'s `ForwardTarget::headers`). `None` for every
    /// plain agent endpoint, which is every agent discovered from the control-plane registry.
    ///
    /// It is a credential, so it never crosses a serde boundary: `skip` keeps a remote registry
    /// response from setting a header on outbound calls, and keeps it out of anything the struct
    /// is serialized into. Only the orchestrator sets it, locally. `Debug` redacts the value.
    #[serde(skip)]
    pub auth_header: Option<(String, String)>,
}

impl std::fmt::Debug for AgentInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentInfo")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("description", &self.description)
            .field("endpoint", &self.endpoint)
            .field("skills", &self.skills)
            .field(
                "auth_header",
                &self
                    .auth_header
                    .as_ref()
                    .map(|(name, _)| format!("{name}: <redacted>")),
            )
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    /// The AgentCard's own `examples` — the literal inputs this skill expects, carried into the
    /// planner's preamble. Defaulted: a registry entry that predates them is still valid.
    #[serde(default)]
    pub examples: Vec<String>,
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
        cache
            .iter()
            .find(|a| a.id == name_or_id || a.name == name_or_id)
            .cloned()
    }

    /// Lists the agents from the control plane's A2A registry (`{base_url}/a2a/v1`).
    ///
    /// `api_key` is a control-plane session token. Without one the registry
    /// lists public agents only; with one, the agents that token may call.
    /// Each entry's address is the agent proxy path (`/api/agents/{id}`),
    /// relative to the control plane, so it is resolved against `base_url` and
    /// the same token is attached to calls to it: the proxy checks it on every
    /// call. The token is attached only to those control-plane paths, never to
    /// an absolute address a registry response names, so a registry cannot
    /// steer it elsewhere.
    async fn discover_from_cp(
        &self,
        base_url: &str,
        api_key: Option<&str>,
    ) -> Result<Vec<AgentInfo>, RegistryError> {
        let base_url = base_url.trim_end_matches('/');
        let url = format!("{base_url}/a2a/v1");
        let auth_header = api_key.map(|key| ("authorization".to_string(), format!("Bearer {key}")));

        let (_, response) = self
            .client
            .send_message_with_headers(
                &url,
                "list all agents",
                None,
                auth_header.as_slice(),
                &[],
                &[],
            )
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
            match serde_json::from_value::<AgentInfo>(val.clone()) {
                Ok(mut agent) => {
                    if agent.endpoint.starts_with('/') {
                        agent.endpoint = format!("{base_url}{}", agent.endpoint);
                        agent.auth_header = auth_header.clone();
                    }
                    agents.push(agent);
                }
                Err(_) => tracing::warn!("skipping unparseable agent entry: {val}"),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn info(auth_header: Option<(String, String)>) -> AgentInfo {
        AgentInfo {
            id: "a".into(),
            name: "agent".into(),
            description: "d".into(),
            endpoint: "http://agent".into(),
            skills: Vec::new(),
            auth_header,
        }
    }

    #[test]
    fn the_auth_header_is_never_read_from_a_registry_response() {
        let from_registry = serde_json::json!({
            "id": "a", "name": "agent", "description": "d", "endpoint": "http://agent",
            "skills": [], "auth_header": ["authorization", "Bearer attacker-chosen"]
        });
        let parsed: AgentInfo = serde_json::from_value(from_registry).unwrap();
        assert_eq!(parsed.auth_header, None);
    }

    #[test]
    fn the_auth_header_is_never_serialized_or_printed() {
        let secret = (
            "authorization".to_string(),
            "Bearer s3cret-token".to_string(),
        );
        let with = info(Some(secret));
        let json = serde_json::to_string(&with).unwrap();
        assert!(
            !json.contains("s3cret") && !json.contains("auth_header"),
            "{json}"
        );
        let debug = format!("{with:?}");
        assert!(!debug.contains("s3cret"), "{debug}");
        assert!(debug.contains("authorization: <redacted>"), "{debug}");
        assert!(format!("{:?}", info(None)).contains("auth_header: None"));
    }

    /// A registry answering `message/send` with one proxy-path entry and one
    /// absolute entry, and recording the `authorization` it was sent.
    async fn registry(
        server: &mut mockito::ServerGuard,
        expect_auth: Option<&str>,
    ) -> mockito::Mock {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": "1",
            "result": {"artifacts": [{"parts": [{"kind": "data", "data": {"agents": [
                {"id": "a1", "name": "proxied", "description": "d", "endpoint": "/api/agents/a1", "skills": []},
                {"id": "a2", "name": "elsewhere", "description": "d", "endpoint": "https://other.example/a2a", "skills": []}
            ]}}]}]}
        });
        let mock = server.mock("POST", "/a2a/v1");
        let mock = match expect_auth {
            Some(value) => mock.match_header("authorization", value),
            None => mock.match_header("authorization", mockito::Matcher::Missing),
        };
        mock.with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create_async()
            .await
    }

    #[tokio::test]
    async fn proxy_paths_resolve_against_the_control_plane_and_carry_the_key() {
        let mut server = mockito::Server::new_async().await;
        let mock = registry(&mut server, Some("Bearer user-token")).await;
        let source = RegistrySource::ControlPlane {
            base_url: format!("{}/", server.url()),
            api_key: Some("user-token".into()),
        };

        let agents = AgentRegistry::new(source).discover().await.unwrap();

        mock.assert_async().await;
        let proxied = agents.iter().find(|a| a.id == "a1").unwrap();
        assert_eq!(proxied.endpoint, format!("{}/api/agents/a1", server.url()));
        assert_eq!(
            proxied.auth_header,
            Some(("authorization".into(), "Bearer user-token".into()))
        );
        // An absolute address from the response never gets the key.
        let elsewhere = agents.iter().find(|a| a.id == "a2").unwrap();
        assert_eq!(elsewhere.endpoint, "https://other.example/a2a");
        assert_eq!(elsewhere.auth_header, None);
    }

    #[tokio::test]
    async fn without_a_key_discovery_sends_no_credential() {
        let mut server = mockito::Server::new_async().await;
        let mock = registry(&mut server, None).await;
        let source = RegistrySource::ControlPlane {
            base_url: server.url(),
            api_key: None,
        };

        let agents = AgentRegistry::new(source).discover().await.unwrap();

        mock.assert_async().await;
        let proxied = agents.iter().find(|a| a.id == "a1").unwrap();
        assert_eq!(proxied.endpoint, format!("{}/api/agents/a1", server.url()));
        assert_eq!(proxied.auth_header, None);
    }
}
