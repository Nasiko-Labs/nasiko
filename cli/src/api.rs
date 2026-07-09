use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use ureq::Agent;

use crate::config;

/// Bail with a diagnosable message (URL + status + body) on HTTP >= 400.
/// A bare status code is useless for debugging — always say what was hit
/// and what came back.
fn check_status(resp: &mut ureq::http::Response<ureq::Body>, url: &str) -> Result<()> {
    let status = resp.status().as_u16();
    if status >= 400 {
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        let body = if body.trim().is_empty() {
            "(empty response body)".to_string()
        } else {
            body
        };
        let hint = if status == 401 {
            "\nhint: your session may have expired — run: nasiko auth login"
        } else {
            ""
        };
        bail!("HTTP {status} from {url}: {body}{hint}");
    }
    Ok(())
}

/// Client for the control plane API + its OCI registry.
pub struct Client {
    agent: Agent,
    base_url: String,
    token: Option<String>,
}

impl Client {
    pub fn from_active_cluster() -> Result<Self> {
        let (_, entry) = config::active_cluster()?;
        let agent = Agent::new_with_config(
            ureq::config::Config::builder()
                .timeout_global(None)
                .http_status_as_error(false)
                .build(),
        );
        Ok(Self {
            agent,
            base_url: entry.url.clone(),
            token: entry.token,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
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
        let _spin = crate::status::start_status(format!("GET {path}"));
        let url = self.api_url(path);
        let mut resp = self.auth_get(&url).call().context("request failed")?;
        check_status(&mut resp, &url)?;
        Ok(resp.body_mut().read_json()?)
    }

    pub fn get_text(&self, path: &str) -> Result<String> {
        let _spin = crate::status::start_status(format!("GET {path}"));
        let url = self.api_url(path);
        let mut resp = self.auth_get(&url).call().context("request failed")?;
        check_status(&mut resp, &url)?;
        Ok(resp.body_mut().read_to_string()?)
    }

    pub fn post_json<T: for<'de> Deserialize<'de>, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let _spin = crate::status::start_status(format!("POST {path}"));
        let url = self.api_url(path);
        let mut resp = self
            .auth_post(&url)
            .send_json(body)
            .context("request failed")?;
        check_status(&mut resp, &url)?;
        Ok(resp.body_mut().read_json()?)
    }

    pub fn put_json<T: for<'de> Deserialize<'de>, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let _spin = crate::status::start_status(format!("PUT {path}"));
        let url = self.api_url(path);
        let mut resp = self
            .auth_put(&url)
            .send_json(body)
            .context("request failed")?;
        check_status(&mut resp, &url)?;
        Ok(resp.body_mut().read_json()?)
    }

    /// POST with no body and ignore the response body (for endpoints that return 200/204 with no JSON).
    pub fn post_void(&self, path: &str) -> Result<()> {
        let _spin = crate::status::start_status(format!("POST {path}"));
        let url = self.api_url(path);
        let mut resp = self
            .auth_post(&url)
            .send(&[] as &[u8])
            .context("request failed")?;
        check_status(&mut resp, &url)?;
        Ok(())
    }

    /// POST a JSON body and ignore the response body (for endpoints that return 200/204 with no JSON).
    pub fn post_json_void<B: Serialize>(&self, path: &str, body: &B) -> Result<()> {
        let _spin = crate::status::start_status(format!("POST {path}"));
        let url = self.api_url(path);
        let mut resp = self
            .auth_post(&url)
            .send_json(body)
            .context("request failed")?;
        check_status(&mut resp, &url)?;
        Ok(())
    }

    pub fn delete(&self, path: &str) -> Result<()> {
        let _spin = crate::status::start_status(format!("DELETE {path}"));
        let url = self.api_url(path);
        let mut req = self.agent.delete(&url);
        if let Some(ref t) = self.token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        let mut resp = req.call().context("request failed")?;
        check_status(&mut resp, &url)?;
        Ok(())
    }

    // ─── Public endpoints (no /api prefix, no auth) ─────────────────────────

    pub fn get_public_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        let _spin = crate::status::start_status(format!("GET {path}"));
        let url = self.raw_url(path);
        let mut resp = self.agent.get(&url).call().context("request failed")?;
        check_status(&mut resp, &url)?;
        Ok(resp.body_mut().read_json()?)
    }

    pub fn health_check(url: &str) -> Result<()> {
        let agent = Agent::new_with_config(
            ureq::config::Config::builder()
                .http_status_as_error(false)
                .build(),
        );
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
            agent: Agent::new_with_config(
                ureq::config::Config::builder()
                    .http_status_as_error(false)
                    .build(),
            ),
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
                    agent: Agent::new_with_config(
                        ureq::config::Config::builder()
                            .http_status_as_error(false)
                            .build(),
                    ),
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
        let put_url = blob_put_url(&self.base_url, &location, digest);

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

/// Build the blob-upload PUT URL from the initiate-upload response's `Location` header.
/// Handles both absolute `Location` values (used as-is) and relative ones (joined to
/// `base_url`), and appends `digest=` with the correct separator depending on whether
/// `location` already carries a query string (e.g. `?_state=...` from some registries) —
/// blindly appending `?digest=...` would otherwise produce an invalid `...?_state=x?digest=y`.
fn blob_put_url(base_url: &str, location: &str, digest: &str) -> String {
    let sep = if location.contains('?') { "&" } else { "?" };
    if location.starts_with("http") {
        format!("{location}{sep}digest={digest}")
    } else {
        format!("{base_url}{location}{sep}digest={digest}")
    }
}

/// Minimal percent-encoding for query-string values (RFC 3986 unreserved kept as-is).
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Build the `/v1/search` request path (incl. query string) for a discovery request.
/// Pure function — separated from the HTTP call so it can be unit-tested.
fn search_query(
    query: Option<&str>,
    artifact_type: Option<&str>,
    framework: Option<&str>,
    limit: u32,
    min_score: Option<f32>,
) -> String {
    let mut params = Vec::new();
    if let Some(q) = query {
        params.push(format!("q={}", urlencode(q)));
    }
    if let Some(t) = artifact_type {
        params.push(format!("type={}", urlencode(t)));
    }
    if let Some(fw) = framework {
        params.push(format!("framework={}", urlencode(fw)));
    }
    if let Some(ms) = min_score {
        params.push(format!("min_score={ms}"));
    }
    params.push(format!("limit={limit}"));
    format!("/v1/search?{}", params.join("&"))
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
            agent: Agent::new_with_config(
                ureq::config::Config::builder()
                    .http_status_as_error(false)
                    .build(),
            ),
            base_url: url.trim_end_matches('/').to_string(),
        })
    }

    pub fn search(&self, query: Option<&str>, artifact_type: Option<&str>, framework: Option<&str>) -> Result<Vec<Artifact>> {
        self.search_opts(query, artifact_type, framework, 100, None)
    }

    pub fn search_opts(
        &self,
        query: Option<&str>,
        artifact_type: Option<&str>,
        framework: Option<&str>,
        limit: u32,
        min_score: Option<f32>,
    ) -> Result<Vec<Artifact>> {
        let path = search_query(query, artifact_type, framework, limit, min_score);
        let url = format!("{}{path}", self.base_url);
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

impl Client {
    /// Upload a zip file to `POST /api/agents/upload` and return the queued build info.
    pub fn upload_agent(
        &self,
        zip_path: &std::path::Path,
        name: &str,
        version_tag: &str,
        ports: &[u16],
        env: &std::collections::HashMap<String, String>,
    ) -> anyhow::Result<UploadQueued> {
        let file_bytes = std::fs::read(zip_path)
            .with_context(|| format!("cannot read {}", zip_path.display()))?;

        let boundary = "NasikoCloudBoundary1234567890";
        let mut body: Vec<u8> = Vec::new();

        // name (required)
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\n{name}\r\n").as_bytes(),
        );
        // version_tag
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"version_tag\"\r\n\r\n{version_tag}\r\n").as_bytes(),
        );
        // ports (comma-separated)
        if !ports.is_empty() {
            let ports_str = ports.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(",");
            body.extend_from_slice(
                format!("--{boundary}\r\nContent-Disposition: form-data; name=\"ports\"\r\n\r\n{ports_str}\r\n").as_bytes(),
            );
        }
        // env (JSON)
        if !env.is_empty() {
            let env_json = serde_json::to_string(env).unwrap_or_else(|_| "{}".into());
            body.extend_from_slice(
                format!("--{boundary}\r\nContent-Disposition: form-data; name=\"env\"\r\n\r\n{env_json}\r\n").as_bytes(),
            );
        }
        // source (the zip file)
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"source\"; filename=\"upload.zip\"\r\nContent-Type: application/zip\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(&file_bytes);
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

        let url = self.api_url("/agents/upload");
        let mut req = self.agent.post(&url)
            .header("Content-Type", &format!("multipart/form-data; boundary={boundary}"));
        if let Some(ref t) = self.token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        let _spin = crate::status::start_status(format!(
            "uploading {name} ({} KB)",
            body.len() / 1024
        ));
        let mut resp = req.send(&body).context("upload request failed")?;
        drop(_spin);
        if resp.status().as_u16() >= 400 {
            let b = resp.body_mut().read_to_string().unwrap_or_default();
            bail!("HTTP {}: {}", resp.status().as_u16(), b);
        }
        Ok(resp.body_mut().read_json()?)
    }

    /// Poll `GET /api/agents/deploys/{build_id}/stream` (SSE) until the build finishes.
    /// Streams status transitions as they arrive, with a live spinner in between.
    /// Returns Ok(()) on success, Err on failure.
    pub fn poll_build_status(&self, build_id: &str) -> anyhow::Result<()> {
        use std::io::BufRead;

        let url = self.api_url(&format!("/agents/deploys/{build_id}/stream"));
        let resp = self.auth_get(&url).call().context("status stream failed")?;
        if resp.status().as_u16() >= 400 {
            bail!("status stream: HTTP {}", resp.status().as_u16());
        }

        let (_parts, body) = resp.into_parts();
        let reader = std::io::BufReader::new(body.into_reader());
        let mut last_status = String::new();
        let mut succeeded = false;
        let mut failed = false;
        let mut spin = Some(crate::status::start_status("waiting for build"));

        for line in reader.lines() {
            let Ok(line) = line else { break };
            let Some(data) = line.strip_prefix("data: ") else { continue };
            let Ok(val) = serde_json::from_str::<serde_json::Value>(data) else { continue };
            let status = val.get("status").and_then(|s| s.as_str()).unwrap_or("unknown");
            if status == last_status { continue; }
            last_status = status.to_string();
            // Drop first so the spinner line is cleared before the transition prints.
            spin = None;
            match status {
                "queued"    => spin = Some(crate::status::start_status("queued")),
                "building"  => spin = Some(crate::status::start_status("building image")),
                "success"   => { println!("  build succeeded"); succeeded = true; }
                "failed"    => { println!("  build failed"); failed = true; }
                other       => println!("  {other}"),
            }
        }
        drop(spin);

        if failed {
            bail!("build failed");
        }
        if !succeeded {
            bail!("build did not complete successfully");
        }
        Ok(())
    }
}

// ─── API types ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct AgentRecord {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    /// JSON-RPC path from the agent's card (e.g. "/jsonrpc"), set by the server.
    #[serde(default)]
    pub transport_path: Option<String>,
    #[serde(default)]
    pub framework: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UploadQueued {
    pub build_id: String,
    pub agent_id: String,
    pub name: String,
    pub image_tag: String,
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct UploadInfo {
    #[serde(default)]
    pub upload_type: Option<String>,
    #[serde(default)]
    pub upload_status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UploadedAgent {
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub upload_info: Option<UploadInfo>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct ContainerStatus {
    pub container_id: String,
    pub state: String,
    #[serde(default)]
    pub replicas_live: u32,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DeploySpec {
    pub image: String,
    pub name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<u16>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub env: std::collections::HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_escapes_query_text() {
        assert_eq!(urlencode("nutrition planning"), "nutrition%20planning");
        assert_eq!(urlencode("a&b=c"), "a%26b%3Dc");
        assert_eq!(urlencode("safe-_.~"), "safe-_.~");
    }

    #[test]
    fn artifact_parses_relevance_score() {
        let json = r#"{
            "id": "1", "owner": "acme", "name": "nutrition", "version": "1.0.0",
            "artifact_type": "skill", "status": "stable", "score": 0.873
        }"#;
        let a: Artifact = serde_json::from_str(json).unwrap();
        assert_eq!(a.score, Some(0.873));
    }

    #[test]
    fn artifact_score_absent_is_none() {
        let json = r#"{
            "id": "1", "owner": "acme", "name": "nutrition", "version": "1.0.0",
            "artifact_type": "skill", "status": "stable"
        }"#;
        let a: Artifact = serde_json::from_str(json).unwrap();
        assert_eq!(a.score, None);
    }

    #[test]
    fn search_query_encodes_and_orders_params() {
        let p = search_query(Some("healthy eating"), Some("skill"), Some("a2a"), 10, Some(0.3));
        assert_eq!(p, "/v1/search?q=healthy%20eating&type=skill&framework=a2a&min_score=0.3&limit=10");
    }

    #[test]
    fn search_query_omits_absent_filters() {
        let p = search_query(Some("taxes"), None, None, 100, None);
        assert_eq!(p, "/v1/search?q=taxes&limit=100");
    }

    #[test]
    fn search_query_browse_has_no_query_param() {
        // `nasiko registry list` path: no query, no score.
        let p = search_query(None, Some("agent"), None, 100, None);
        assert_eq!(p, "/v1/search?type=agent&limit=100");
    }

    #[test]
    fn search_query_escapes_special_chars() {
        let p = search_query(Some("a & b?c"), None, None, 5, None);
        assert_eq!(p, "/v1/search?q=a%20%26%20b%3Fc&limit=5");
    }

    #[test]
    fn blob_put_url_absolute_location_no_query() {
        let url = blob_put_url("https://cp.example.com", "https://cp.example.com/v2/repo/blobs/uploads/abc", "sha256:deadbeef");
        assert_eq!(url, "https://cp.example.com/v2/repo/blobs/uploads/abc?digest=sha256:deadbeef");
    }

    #[test]
    fn blob_put_url_absolute_location_with_existing_query() {
        // Regression: registries that return a Location already carrying a query string
        // (e.g. `?_state=xyz`) must get `&digest=...`, not a second `?digest=...`.
        let url = blob_put_url("https://cp.example.com", "https://cp.example.com/v2/repo/blobs/uploads/abc?_state=xyz", "sha256:deadbeef");
        assert_eq!(url, "https://cp.example.com/v2/repo/blobs/uploads/abc?_state=xyz&digest=sha256:deadbeef");
    }

    #[test]
    fn blob_put_url_relative_location_no_query() {
        let url = blob_put_url("https://cp.example.com", "/v2/repo/blobs/uploads/abc", "sha256:deadbeef");
        assert_eq!(url, "https://cp.example.com/v2/repo/blobs/uploads/abc?digest=sha256:deadbeef");
    }

    #[test]
    fn blob_put_url_relative_location_with_existing_query() {
        let url = blob_put_url("https://cp.example.com", "/v2/repo/blobs/uploads/abc?_state=xyz", "sha256:deadbeef");
        assert_eq!(url, "https://cp.example.com/v2/repo/blobs/uploads/abc?_state=xyz&digest=sha256:deadbeef");
    }
}
