use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use ureq::Agent;

use crate::config;

/// Client for the control plane API + its OCI registry.
pub struct Client {
    agent: Agent,
    base_url: String,
    token: Option<String>,
}

impl Client {
    pub fn from_active_cluster() -> Result<Self> {
        let (_, entry) = config::active_cluster()?;
        Ok(Self {
            agent: Agent::new_with_defaults(),
            base_url: entry.url.clone(),
            token: entry.token,
        })
    }

    fn api_url(&self, path: &str) -> String {
        format!("{}/api{}", self.base_url, path)
    }

    fn raw_url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn auth_get(&self, url: &str) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
        let mut r = self.agent.get(url);
        if let Some(ref t) = self.token {
            r = r.header("Authorization", &format!("Bearer {t}"));
        }
        r
    }

    fn auth_post(&self, url: &str) -> ureq::RequestBuilder<ureq::typestate::WithBody> {
        let mut r = self.agent.post(url);
        if let Some(ref t) = self.token {
            r = r.header("Authorization", &format!("Bearer {t}"));
        }
        r
    }

    fn auth_put(&self, url: &str) -> ureq::RequestBuilder<ureq::typestate::WithBody> {
        let mut r = self.agent.put(url);
        if let Some(ref t) = self.token {
            r = r.header("Authorization", &format!("Bearer {t}"));
        }
        r
    }

    // ─── Authenticated CP API calls (/api/*) ────────────────────────────────

    pub fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        let mut resp = self.auth_get(&self.api_url(path)).call().context("request failed")?;
        if resp.status().as_u16() >= 400 {
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            bail!("HTTP {}: {}", resp.status().as_u16(), body);
        }
        Ok(resp.body_mut().read_json()?)
    }

    pub fn get_text(&self, path: &str) -> Result<String> {
        let mut resp = self.auth_get(&self.api_url(path)).call().context("request failed")?;
        if resp.status().as_u16() >= 400 {
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            bail!("HTTP {}: {}", resp.status().as_u16(), body);
        }
        Ok(resp.body_mut().read_to_string()?)
    }

    pub fn post_json<T: for<'de> Deserialize<'de>, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let mut resp = self
            .auth_post(&self.api_url(path))
            .send_json(body)
            .context("request failed")?;
        if resp.status().as_u16() >= 400 {
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            bail!("HTTP {}: {}", resp.status().as_u16(), body);
        }
        Ok(resp.body_mut().read_json()?)
    }

    pub fn put_json<T: for<'de> Deserialize<'de>, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let mut resp = self
            .auth_put(&self.api_url(path))
            .send_json(body)
            .context("request failed")?;
        if resp.status().as_u16() >= 400 {
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            bail!("HTTP {}: {}", resp.status().as_u16(), body);
        }
        Ok(resp.body_mut().read_json()?)
    }

    pub fn post_empty<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        self.post_json(path, &serde_json::json!({}))
    }

    pub fn delete(&self, path: &str) -> Result<()> {
        let mut req = self.agent.delete(&self.api_url(path));
        if let Some(ref t) = self.token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        let mut resp = req.call().context("request failed")?;
        if resp.status().as_u16() >= 400 {
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            bail!("HTTP {}: {}", resp.status().as_u16(), body);
        }
        Ok(())
    }

    // ─── Public endpoints (no /api prefix, no auth) ─────────────────────────

    pub fn get_public_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        let mut resp = self.agent.get(&self.raw_url(path)).call().context("request failed")?;
        if resp.status().as_u16() >= 400 {
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            bail!("HTTP {}: {}", resp.status().as_u16(), body);
        }
        Ok(resp.body_mut().read_json()?)
    }

    pub fn health_check(url: &str) -> Result<()> {
        let agent = Agent::new_with_defaults();
        let url = format!("{}/health", url.trim_end_matches('/'));
        let resp = agent.get(&url).call().context("cannot reach control plane")?;
        if resp.status().as_u16() >= 400 {
            bail!("health check returned HTTP {}", resp.status().as_u16());
        }
        Ok(())
    }
}

/// Client for OCI Distribution operations.
/// Used against CP's OCI registry (image push) and artifact registry (template pull).
pub struct OciClient {
    agent: Agent,
    base_url: String,
    auth_header: Option<String>,
}

impl OciClient {
    pub fn for_cp() -> Result<Self> {
        let (_, entry) = config::active_cluster()?;
        Ok(Self {
            agent: Agent::new_with_defaults(),
            base_url: entry.url.clone(),
            auth_header: entry.token.map(|t| format!("Bearer {t}")),
        })
    }

    pub fn for_artifact_registry() -> Result<Option<Self>> {
        match config::artifact_registry_url() {
            Some(url) => {
                use base64::Engine;
                let auth = match (
                    std::env::var("NASIKO_REGISTRY_USER"),
                    std::env::var("NASIKO_REGISTRY_PASS"),
                ) {
                    (Ok(user), Ok(pass)) => {
                        let encoded = base64::engine::general_purpose::STANDARD
                            .encode(format!("{user}:{pass}"));
                        Some(format!("Basic {encoded}"))
                    }
                    _ => None,
                };
                Ok(Some(Self {
                    agent: Agent::new_with_defaults(),
                    base_url: url.trim_end_matches('/').to_string(),
                    auth_header: auth,
                }))
            }
            None => Ok(None),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn get(&self, url: &str) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
        let mut r = self.agent.get(url);
        if let Some(ref auth) = self.auth_header {
            r = r.header("Authorization", auth);
        }
        r
    }

    fn post(&self, url: &str) -> ureq::RequestBuilder<ureq::typestate::WithBody> {
        let mut r = self.agent.post(url);
        if let Some(ref auth) = self.auth_header {
            r = r.header("Authorization", auth);
        }
        r
    }

    fn put(&self, url: &str) -> ureq::RequestBuilder<ureq::typestate::WithBody> {
        let mut r = self.agent.put(url);
        if let Some(ref auth) = self.auth_header {
            r = r.header("Authorization", auth);
        }
        r
    }

    fn head(&self, url: &str) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
        let mut r = self.agent.head(url);
        if let Some(ref auth) = self.auth_header {
            r = r.header("Authorization", auth);
        }
        r
    }

    pub fn blob_exists(&self, repo: &str, digest: &str) -> bool {
        let url = self.url(&format!("/v2/{repo}/blobs/{digest}"));
        self.head(&url)
            .call()
            .map(|r| r.status().as_u16() == 200)
            .unwrap_or(false)
    }

    pub fn push_blob(&self, repo: &str, digest: &str, data: &[u8]) -> Result<()> {
        if self.blob_exists(repo, digest) {
            return Ok(());
        }

        // POST to initiate
        let url = self.url(&format!("/v2/{repo}/blobs/uploads/"));
        let resp = self.post(&url).send(&[] as &[u8]).context("initiate upload failed")?;
        if resp.status().as_u16() >= 400 {
            bail!("initiate upload: HTTP {}", resp.status().as_u16());
        }

        let location = resp
            .headers()
            .get("Location")
            .and_then(|v| v.to_str().ok())
            .context("no Location header")?
            .to_string();

        // PUT with full body + digest
        let put_url = if location.starts_with("http") {
            format!("{location}?digest={digest}")
        } else {
            format!("{}{}?digest={digest}", self.base_url, location)
        };

        let mut resp = self
            .put(&put_url)
            .header("Content-Type", "application/octet-stream")
            .send(data)
            .context("blob upload failed")?;
        if resp.status().as_u16() >= 400 {
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            bail!("blob upload: HTTP {}: {}", resp.status().as_u16(), body);
        }
        Ok(())
    }

    pub fn push_manifest(
        &self,
        repo: &str,
        reference: &str,
        content_type: &str,
        manifest: &[u8],
    ) -> Result<String> {
        let url = self.url(&format!("/v2/{repo}/manifests/{reference}"));
        let resp = self
            .put(&url)
            .header("Content-Type", content_type)
            .send(manifest)
            .context("manifest push failed")?;
        if resp.status().as_u16() >= 400 {
            bail!("manifest push: HTTP {}", resp.status().as_u16());
        }
        let digest = resp
            .headers()
            .get("Docker-Content-Digest")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        Ok(digest)
    }

    pub fn list_tags(&self, repo: &str) -> Result<String> {
        let url = self.url(&format!("/v2/{repo}/tags/list"));
        let mut resp = self.get(&url).call().context("tags list failed")?;
        if resp.status().as_u16() >= 400 {
            bail!("tags list: HTTP {}", resp.status().as_u16());
        }
        Ok(resp.body_mut().read_to_string()?)
    }

    pub fn pull_manifest(&self, repo: &str, reference: &str) -> Result<String> {
        let url = self.url(&format!("/v2/{repo}/manifests/{reference}"));
        let mut resp = self.get(&url).call().context("manifest pull failed")?;
        if resp.status().as_u16() >= 400 {
            bail!("manifest pull: HTTP {}", resp.status().as_u16());
        }
        Ok(resp.body_mut().read_to_string()?)
    }

    pub fn pull_blob(&self, repo: &str, digest: &str) -> Result<Vec<u8>> {
        let url = self.url(&format!("/v2/{repo}/blobs/{digest}"));
        let mut resp = self.get(&url).call().context("blob pull failed")?;
        if resp.status().as_u16() >= 400 {
            bail!("blob pull: HTTP {}", resp.status().as_u16());
        }
        Ok(resp.body_mut().read_to_vec()?)
    }
}

// ─── Artifact Registry V1 Client ───────────────────────────────────────────

pub use nasiko_types::registry::{Artifact, SearchResponse};

pub struct RegistryClient {
    agent: Agent,
    base_url: String,
}

impl RegistryClient {
    pub fn new() -> Option<Self> {
        let url = config::artifact_registry_url()?;
        Some(Self {
            agent: Agent::new_with_defaults(),
            base_url: url.trim_end_matches('/').to_string(),
        })
    }

    pub fn search(&self, query: Option<&str>, artifact_type: Option<&str>, framework: Option<&str>) -> Result<Vec<Artifact>> {
        let mut params = Vec::new();
        if let Some(q) = query {
            params.push(format!("q={q}"));
        }
        if let Some(t) = artifact_type {
            params.push(format!("type={t}"));
        }
        if let Some(fw) = framework {
            params.push(format!("framework={fw}"));
        }
        params.push("limit=100".to_string());
        let qs = params.join("&");
        let url = format!("{}/v1/search?{qs}", self.base_url);
        let mut resp = self.agent.get(&url).call().context("registry search failed")?;
        if resp.status().as_u16() >= 400 {
            bail!("registry search: HTTP {}", resp.status().as_u16());
        }
        let sr: SearchResponse = resp.body_mut().read_json()?;
        Ok(sr.data)
    }

    pub fn list_templates(&self) -> Result<Vec<Artifact>> {
        self.search(None, Some("agent"), None)
    }

    pub fn list_skills(&self, framework: Option<&str>) -> Result<Vec<Artifact>> {
        self.search(None, Some("skill"), framework)
    }
}

// ─── API types ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ContainerStatus {
    pub name: String,
    pub state: String,
    pub image: String,
    #[serde(default)]
    pub node_id: String,
    #[serde(default)]
    pub port: u16,
}

#[derive(Debug, Serialize)]
pub struct DeploySpec {
    pub image: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub env: std::collections::HashMap<String, String>,
}
