use anyhow::{Context, Result, bail};
use nasiko_utils::display::opt_dash;
use serde::{Deserialize, Serialize};
use tabled::Tabled;
use ureq::Agent;

use crate::config;

/// Bail with a diagnosable message (URL + status + body) on HTTP >= 400.
/// A bare status code is useless for debugging — always say what was hit
/// and what came back.
fn check_status(resp: &mut ureq::http::Response<ureq::Body>, url: &str) -> Result<()> {
    let status = resp.status().as_u16();
    if status < 400 {
        return Ok(());
    }
    let body = resp.body_mut().read_to_string().unwrap_or_default();
    let detail = extract_error_detail(&body);
    let hint = status_hint(status);
    bail!("HTTP {status} from {url}: {detail}{hint}");
}

/// Pull a human-readable message out of a server error body.
/// Tries the standard `{"message":"..."}` and `{"error":"..."}` shapes first;
/// falls back to the raw body when neither is present.
fn extract_error_detail(body: &str) -> String {
    if body.trim().is_empty() {
        return "(empty response)".to_string();
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        // Try {"data": {"message": "..."}} (MCP envelope)
        if let Some(s) = v
            .get("data")
            .and_then(|d| d.get("message"))
            .and_then(|m| m.as_str())
            && !s.is_empty()
        {
            return s.to_string();
        }
        // Try {"message": "..."} and {"error": "..."}
        for key in ["message", "error"] {
            if let Some(s) = v.get(key).and_then(|m| m.as_str())
                && !s.is_empty()
            {
                return s.to_string();
            }
        }
    }
    body.trim().to_string()
}

fn status_hint(status: u16) -> &'static str {
    match status {
        401 => "\nhint: session expired or invalid — run: nasiko auth login",
        403 => "\nhint: you don't have permission to do this",
        404 => "\nhint: resource not found — check the ID or name",
        // No blanket hint for 409: the server's detail message (already shown)
        // covers several distinct conflicts — name collision, version-not-greater,
        // rollback-not-eligible, etc. — and a fixed "resource already exists" hint
        // is actively misleading for the non-name-collision cases.
        422 => "\nhint: request was rejected due to invalid input",
        429 => "\nhint: rate limit exceeded — wait a moment and retry",
        500..=599 => "\nhint: server error — check server logs or try again",
        _ => "",
    }
}

/// Every MCP management endpoint replies with the shared
/// `{data, status_code, message}` envelope (`oss/server/src/mcp/mod.rs::ApiResponse`)
/// instead of a bare body. Unwraps `data` and deserializes it into `T`.
pub(crate) fn unwrap_data<T: for<'de> Deserialize<'de>>(value: serde_json::Value) -> Result<T> {
    let data = value
        .get("data")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Ok(serde_json::from_value(data)?)
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

    /// Build a client against an arbitrary base URL (mock server in tests),
    /// bypassing `~/.nasiko/config.json`.
    #[cfg(test)]
    pub(crate) fn for_test(base_url: &str, token: Option<&str>) -> Self {
        Self {
            agent: Agent::new_with_config(
                ureq::config::Config::builder()
                    .http_status_as_error(false)
                    .build(),
            ),
            base_url: base_url.to_string(),
            token: token.map(str::to_string),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The caller's own user id, decoded locally from the stored JWT's `sub`
    /// claim. `None` if there's no token, or it's not a JWT this can decode.
    pub fn current_user_id(&self) -> Option<String> {
        crate::config::token_subject(self.token.as_deref()?)
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

    fn auth_patch(&self, url: &str) -> ureq::RequestBuilder<ureq::typestate::WithBody> {
        let mut r = self.agent.patch(url);
        if let Some(ref t) = self.token {
            r = r.header("Authorization", &format!("Bearer {t}"));
        }
        r
    }

    // ─── Authenticated CP API calls (/api/*) ────────────────────────────────

    pub fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        let _spin = nasiko_utils::term::start_status(format!("GET {path}"));
        let url = self.api_url(path);
        let mut resp = self
            .auth_get(&url)
            .call()
            .context("cannot reach control plane")?;
        check_status(&mut resp, &url)?;
        Ok(resp.body_mut().read_json()?)
    }

    /// GET returning `Ok(None)` on 404 instead of erroring, while still bailing
    /// on other failures. Lets callers tell "resource is gone" (e.g. a stale
    /// local agent binding after a server-side delete / DB reset) apart from a
    /// real error, so they can recover rather than fail.
    pub fn get_json_optional<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<Option<T>> {
        let _spin = nasiko_utils::term::start_status(format!("GET {path}"));
        let url = self.api_url(path);
        let mut resp = self
            .auth_get(&url)
            .call()
            .context("cannot reach control plane")?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        check_status(&mut resp, &url)?;
        Ok(Some(resp.body_mut().read_json()?))
    }

    /// GET returning `Ok(None)` on 403 instead of erroring. For endpoints where
    /// a 403 means "not entitled yet" rather than a real error — e.g. GitHub
    /// repositories before the account is connected — so callers can show a
    /// friendly "not connected" message instead of a raw HTTP error.
    pub fn get_json_optional_on_forbidden<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
    ) -> Result<Option<T>> {
        let _spin = nasiko_utils::term::start_status(format!("GET {path}"));
        let url = self.api_url(path);
        let mut resp = self.auth_get(&url).call().context("request failed")?;
        if resp.status().as_u16() == 403 {
            return Ok(None);
        }
        check_status(&mut resp, &url)?;
        Ok(Some(resp.body_mut().read_json()?))
    }

    pub fn get_text(&self, path: &str) -> Result<String> {
        let _spin = nasiko_utils::term::start_status(format!("GET {path}"));
        let url = self.api_url(path);
        let mut resp = self
            .auth_get(&url)
            .call()
            .context("cannot reach control plane")?;
        check_status(&mut resp, &url)?;
        Ok(resp.body_mut().read_to_string()?)
    }

    pub fn post_json<T: for<'de> Deserialize<'de>, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let _spin = nasiko_utils::term::start_status(format!("POST {path}"));
        let url = self.api_url(path);
        let mut resp = self
            .auth_post(&url)
            .send_json(body)
            .context("cannot reach control plane")?;
        check_status(&mut resp, &url)?;
        Ok(resp.body_mut().read_json()?)
    }

    pub fn put_json<T: for<'de> Deserialize<'de>, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let _spin = nasiko_utils::term::start_status(format!("PUT {path}"));
        let url = self.api_url(path);
        let mut resp = self
            .auth_put(&url)
            .send_json(body)
            .context("cannot reach control plane")?;
        check_status(&mut resp, &url)?;
        Ok(resp.body_mut().read_json()?)
    }

    pub fn patch_json<T: for<'de> Deserialize<'de>, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let _spin = nasiko_utils::term::start_status(format!("PATCH {path}"));
        let url = self.api_url(path);
        let mut resp = self
            .auth_patch(&url)
            .send_json(body)
            .context("cannot reach control plane")?;
        check_status(&mut resp, &url)?;
        Ok(resp.body_mut().read_json()?)
    }

    /// POST with no body and ignore the response body (for endpoints that return 200/204 with no JSON).
    pub fn post_void(&self, path: &str) -> Result<()> {
        let _spin = nasiko_utils::term::start_status(format!("POST {path}"));
        let url = self.api_url(path);
        let mut resp = self
            .auth_post(&url)
            .send(&[] as &[u8])
            .context("cannot reach control plane")?;
        check_status(&mut resp, &url)?;
        Ok(())
    }

    /// POST a JSON body and ignore the response body (for endpoints that return 200/204 with no JSON).
    pub fn post_json_void<B: Serialize>(&self, path: &str, body: &B) -> Result<()> {
        let _spin = nasiko_utils::term::start_status(format!("POST {path}"));
        let url = self.api_url(path);
        let mut resp = self
            .auth_post(&url)
            .send_json(body)
            .context("cannot reach control plane")?;
        check_status(&mut resp, &url)?;
        Ok(())
    }

    pub fn delete(&self, path: &str) -> Result<()> {
        let _spin = nasiko_utils::term::start_status(format!("DELETE {path}"));
        let url = self.api_url(path);
        let mut req = self.agent.delete(&url);
        if let Some(ref t) = self.token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        let mut resp = req.call().context("cannot reach control plane")?;
        check_status(&mut resp, &url)?;
        Ok(())
    }

    /// DELETE and parse the JSON response body (for endpoints that return
    /// details about what was torn down, e.g. `DELETE /agents/{id}`).
    pub fn delete_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        let _spin = nasiko_utils::term::start_status(format!("DELETE {path}"));
        let url = self.api_url(path);
        let mut req = self.agent.delete(&url);
        if let Some(ref t) = self.token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        let mut resp = req.call().context("request failed")?;
        check_status(&mut resp, &url)?;
        Ok(resp.body_mut().read_json()?)
    }

    /// DELETE and parse a JSON response body (for routes that return a
    /// descriptive body — e.g. a disconnect confirmation message — instead
    /// of a bare 204).
    pub fn delete_and_read<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        let _spin = nasiko_utils::term::start_status(format!("DELETE {path}"));
        let url = self.api_url(path);
        let mut req = self.agent.delete(&url);
        if let Some(ref t) = self.token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        let mut resp = req.call().context("cannot reach control plane")?;
        check_status(&mut resp, &url)?;
        Ok(resp.body_mut().read_json()?)
    }

    /// DELETE with a JSON body, ignoring the response body. Off-spec (DELETE
    /// shouldn't carry a body) but some routes need it to name a target
    /// (e.g. revoking a specific share grant) — `force_send_body()` is ureq's
    /// documented escape hatch for exactly this.
    pub fn delete_json_void<B: Serialize>(&self, path: &str, body: &B) -> Result<()> {
        let _spin = nasiko_utils::term::start_status(format!("DELETE {path}"));
        let url = self.api_url(path);
        let mut req = self.agent.delete(&url).force_send_body();
        if let Some(ref t) = self.token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        let mut resp = req.send_json(body).context("cannot reach control plane")?;
        check_status(&mut resp, &url)?;
        Ok(())
    }

    // ─── Public endpoints (no /api prefix, no auth) ─────────────────────────

    pub fn get_public_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        let _spin = nasiko_utils::term::start_status(format!("GET {path}"));
        let url = self.raw_url(path);
        let mut resp = self
            .agent
            .get(&url)
            .call()
            .context("cannot reach control plane")?;
        check_status(&mut resp, &url)?;
        Ok(resp.body_mut().read_json()?)
    }

    /// Like `get_public_json`, but parses and returns the body regardless of
    /// HTTP status — for endpoints that intentionally signal degraded state
    /// via a non-2xx code (e.g. `/readiness` returns 503 exactly when the
    /// caller most needs to see the per-subsystem breakdown in the body,
    /// not have it discarded in favor of a raw HTTP-error message).
    pub fn get_public_json_any_status<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
    ) -> Result<T> {
        let _spin = nasiko_utils::term::start_status(format!("GET {path}"));
        let url = self.raw_url(path);
        let mut resp = self
            .agent
            .get(&url)
            .call()
            .context("cannot reach control plane")?;
        Ok(resp.body_mut().read_json()?)
    }

    pub fn health_check(url: &str) -> Result<()> {
        let agent = Agent::new_with_config(
            ureq::config::Config::builder()
                .http_status_as_error(false)
                .build(),
        );
        let url = format!("{}/health", url.trim_end_matches('/'));
        let resp = agent
            .get(&url)
            .call()
            .context("cannot reach control plane")?;
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
        let resp = self
            .post(&url)
            .send(&[] as &[u8])
            .context("initiate upload failed")?;
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

    pub fn search(
        &self,
        query: Option<&str>,
        artifact_type: Option<&str>,
        framework: Option<&str>,
    ) -> Result<Vec<Artifact>> {
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
        let mut resp = self
            .agent
            .get(&url)
            .call()
            .context("registry search failed")?;
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
    #[allow(clippy::too_many_arguments)]
    pub fn upload_agent(
        &self,
        zip_path: &std::path::Path,
        name: &str,
        version_tag: &str,
        ports: &[u16],
        env: &std::collections::HashMap<String, String>,
        writable: bool,
    ) -> anyhow::Result<UploadQueued> {
        let file_bytes = std::fs::read(zip_path)
            .with_context(|| format!("cannot read {}", zip_path.display()))?;

        let boundary = "NasikoCloudBoundary1234567890";
        let mut body: Vec<u8> = Vec::new();

        // agent_name (required)
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\n{name}\r\n"
            )
            .as_bytes(),
        );
        // version_tag
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"version_tag\"\r\n\r\n{version_tag}\r\n").as_bytes(),
        );
        // ports (comma-separated)
        if !ports.is_empty() {
            let ports_str = ports
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(",");
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
        // writable
        if writable {
            body.extend_from_slice(
                format!("--{boundary}\r\nContent-Disposition: form-data; name=\"writable\"\r\n\r\ntrue\r\n").as_bytes(),
            );
        }
        // file (the zip file)
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"upload.zip\"\r\nContent-Type: application/zip\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(&file_bytes);
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

        let url = self.api_url("/agents/upload");
        let mut req = self.agent.post(&url).header(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        );
        if let Some(ref t) = self.token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        let _spin = nasiko_utils::term::start_status(format!(
            "uploading {name} ({} KB)",
            body.len() / 1024
        ));
        let mut resp = req.send(&body).context("cannot reach control plane")?;
        drop(_spin);
        if resp.status().as_u16() >= 400 {
            let b = resp.body_mut().read_to_string().unwrap_or_default();
            bail!(
                "upload failed (HTTP {}): {}",
                resp.status().as_u16(),
                extract_error_detail(&b)
            );
        }
        Ok(resp.body_mut().read_json()?)
    }

    /// Upload a zip to `PUT /api/agents/{id}/update` (re-upload / server-side rebuild).
    pub fn update_agent(
        &self,
        agent_id: &str,
        zip_path: &std::path::Path,
        version: Option<&str>,
        changelog: Option<&str>,
    ) -> anyhow::Result<UpdateQueued> {
        let file_bytes = std::fs::read(zip_path)
            .with_context(|| format!("cannot read {}", zip_path.display()))?;

        let boundary = "NasikoCloudBoundary1234567890";
        let mut body: Vec<u8> = Vec::new();

        // `version` field: explicit semver or strategy keyword (auto/patch/minor/major).
        // Omit entirely to let the server default to auto-patch.
        if let Some(v) = version {
            body.extend_from_slice(
                format!("--{boundary}\r\nContent-Disposition: form-data; name=\"version\"\r\n\r\n{v}\r\n").as_bytes(),
            );
        }
        if let Some(c) = changelog {
            body.extend_from_slice(
                format!("--{boundary}\r\nContent-Disposition: form-data; name=\"changelog\"\r\n\r\n{c}\r\n").as_bytes(),
            );
        }
        // Field name on the update route is "source" (not "file" as on the upload route).
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"source\"; filename=\"upload.zip\"\r\nContent-Type: application/zip\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(&file_bytes);
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

        let url = self.api_url(&format!("/agents/{agent_id}/update"));
        let mut req = self.agent.put(&url).header(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        );
        if let Some(ref t) = self.token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        let _spin = nasiko_utils::term::start_status(format!(
            "uploading update ({} KB)",
            body.len() / 1024
        ));
        let mut resp = req.send(&body).context("cannot reach control plane")?;
        drop(_spin);
        if resp.status().as_u16() >= 400 {
            let b = resp.body_mut().read_to_string().unwrap_or_default();
            bail!(
                "update failed (HTTP {}): {}",
                resp.status().as_u16(),
                extract_error_detail(&b)
            );
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
        let mut spin = Some(nasiko_utils::term::start_status("waiting for build"));

        for line in reader.lines() {
            let Ok(line) = line else { break };
            let Some(data) = line.strip_prefix("data: ") else {
                continue;
            };
            let Ok(val) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            let status = val
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("unknown");
            if status == last_status {
                continue;
            }
            last_status = status.to_string();
            // Drop first so the spinner line is cleared before the transition prints.
            spin = None;
            match status {
                "queued" => spin = Some(nasiko_utils::term::start_status("queued")),
                "building" => spin = Some(nasiko_utils::term::start_status("building image")),
                "success" => {
                    println!("  build succeeded");
                    succeeded = true;
                }
                "failed" => {
                    println!("  build failed");
                    failed = true;
                }
                other => println!("  {other}"),
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

    /// Upload a zip file to `POST /api/mcp/connectors/upload` and return the
    /// queued connector/build ids. Mirrors [`upload_agent`](Client::upload_agent)'s
    /// multipart body construction exactly (same boundary style, same manual
    /// `Content-Disposition` framing) — the MCP-server-upload endpoint takes a
    /// different, smaller field set (`name`/`version_tag`/`env`/`source`, no
    /// `ports`) but is otherwise the same shape.
    pub fn upload_mcp_connector_zip(
        &self,
        zip_path: &std::path::Path,
        name: &str,
        version_tag: &str,
        env: &std::collections::HashMap<String, String>,
    ) -> anyhow::Result<McpUploadQueued> {
        let file_bytes = std::fs::read(zip_path)
            .with_context(|| format!("cannot read {}", zip_path.display()))?;

        let boundary = "NasikoCloudBoundary1234567890";
        let mut body: Vec<u8> = Vec::new();

        // name (required)
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\n{name}\r\n"
            )
            .as_bytes(),
        );
        // version_tag
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"version_tag\"\r\n\r\n{version_tag}\r\n").as_bytes(),
        );
        // env (JSON) — decrypted server-side and injected as container env vars
        // only at deploy time, per the uploaded server's own secrets (never the
        // gateway's connector credentials — see build.rs's own doc comment on
        // `build_secrets_env` for that distinction).
        if !env.is_empty() {
            let env_json = serde_json::to_string(env).unwrap_or_else(|_| "{}".into());
            body.extend_from_slice(
                format!("--{boundary}\r\nContent-Disposition: form-data; name=\"env\"\r\n\r\n{env_json}\r\n").as_bytes(),
            );
        }
        // source (the zip file) — field name must be "source" or "file", both
        // accepted by the handler (oss/server/src/mcp/handlers/upload.rs).
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"source\"; filename=\"upload.zip\"\r\nContent-Type: application/zip\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(&file_bytes);
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

        let url = self.api_url("/mcp/connectors/upload");
        let mut req = self.agent.post(&url).header(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        );
        if let Some(ref t) = self.token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        let _spin = nasiko_utils::term::start_status(format!(
            "uploading {name} ({} KB)",
            body.len() / 1024
        ));
        let mut resp = req.send(&body).context("cannot reach control plane")?;
        drop(_spin);
        if resp.status().as_u16() >= 400 {
            let b = resp.body_mut().read_to_string().unwrap_or_default();
            bail!(
                "upload failed (HTTP {}): {}",
                resp.status().as_u16(),
                extract_error_detail(&b)
            );
        }
        unwrap_data(resp.body_mut().read_json()?)
    }

    /// Polls `GET /api/mcp/connectors/{id}/build-status` every 2s until the
    /// build reaches a terminal state (`running` = success, `failed` =
    /// failure). Unlike [`poll_build_status`](Client::poll_build_status) (SSE),
    /// the MCP upload route is deliberately plain polling JSON in v1 — no
    /// streaming (see `docs/MCP_UPLOAD_ITERATION_PLAN.md` Step 10's "Deferred"
    /// note) — so this polls on a fixed interval instead of reading a stream.
    pub fn poll_mcp_build_status(&self, connector_id: &str) -> anyhow::Result<()> {
        let mut last_status = String::new();
        let mut spin = Some(nasiko_utils::term::start_status("waiting for build"));
        let mut succeeded = false;
        let mut fail_msg = String::new();

        loop {
            let raw: serde_json::Value =
                self.get_json(&format!("/mcp/connectors/{connector_id}/build-status"))?;
            let status: McpBuildStatus = unwrap_data(raw)?;
            let build_status = status.build_status.unwrap_or_else(|| "pending".to_string());
            if build_status != last_status {
                last_status = build_status.clone();
                spin = None; // Drop first so the spinner line clears before the transition prints.
                match build_status.as_str() {
                    "pending" => spin = Some(nasiko_utils::term::start_status("queued")),
                    "building" => spin = Some(nasiko_utils::term::start_status("building image")),
                    "running" => {
                        println!("  build succeeded — connector is live");
                        succeeded = true;
                    }
                    "failed" => {
                        fail_msg = status
                            .error_msg
                            .unwrap_or_else(|| "(no error message)".to_string());
                    }
                    other => println!("  {other}"),
                }
            }
            if succeeded || build_status == "failed" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        drop(spin);

        if !succeeded {
            bail!("build failed: {fail_msg}");
        }
        Ok(())
    }
}

// ─── MCP-server-upload API types ────────────────────────────────────────────

/// Response body of both `POST /api/mcp/connectors/upload` and
/// `POST /api/mcp/connectors/upload-github` (`oss/server/src/mcp/handlers/upload.rs`).
#[derive(Debug, Deserialize)]
pub struct McpUploadQueued {
    pub connector_id: String,
    pub build_id: String,
}

/// Response body of `GET /api/mcp/connectors/{id}/build-status`.
#[derive(Debug, Deserialize, Serialize)]
pub struct McpBuildStatus {
    #[serde(default)]
    pub build_status: Option<String>,
    #[serde(default)]
    pub error_msg: Option<String>,
    #[serde(default)]
    pub image_tag: Option<String>,
}

// ─── API types ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Tabled)]
pub struct AgentRecord {
    #[tabled(rename = "ID")]
    #[serde(alias = "agent_id")]
    pub id: String,
    #[tabled(rename = "NAME")]
    pub name: String,
    #[tabled(rename = "STATUS", display = "opt_dash")]
    #[serde(default)]
    pub status: Option<String>,
    #[tabled(rename = "VERSION", display = "opt_dash")]
    #[serde(default)]
    pub version: Option<String>,
    #[tabled(rename = "URL", display = "opt_dash")]
    #[serde(default)]
    pub url: Option<String>,
    /// JSON-RPC path from the agent's card (e.g. "/jsonrpc"), set by the server.
    #[tabled(skip)]
    #[serde(default)]
    pub transport_path: Option<String>,
    #[tabled(skip)]
    #[serde(default)]
    pub framework: Option<String>,
    #[tabled(skip)]
    #[serde(default)]
    pub created_at: Option<String>,
    #[tabled(skip)]
    #[serde(default)]
    pub description: Option<String>,
    /// Consumed by `agents ps` to split "Created by you" vs "Shared with
    /// you" (mirrors `nasiko mcp connector list`) — not shown as a column
    /// itself, since the section header already conveys it.
    #[tabled(skip)]
    #[serde(default)]
    pub owner_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UploadQueuedData {
    pub agent_name: String,
    pub status: String,
    #[serde(default)]
    pub build_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UploadQueued {
    pub data: UploadQueuedData,
    #[serde(default)]
    pub status_code: u16,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct DeploymentRecord {
    pub id: String,
    pub agent_id: String,
    #[serde(default)]
    pub agent_name: Option<String>,
    pub status: String,
    #[serde(default)]
    pub replicas: i32,
    #[serde(default)]
    pub service_url: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub crash_reason: Option<String>,
    #[serde(default)]
    pub crashed_at: Option<String>,
    #[serde(default)]
    pub restart_count: i32,
}

#[derive(Debug, Deserialize, Tabled, Default)]
pub struct UploadInfo {
    #[tabled(rename = "STATUS", display = "opt_dash")]
    #[serde(default)]
    pub upload_status: Option<String>,
    #[tabled(rename = "TYPE", display = "opt_dash")]
    #[serde(default)]
    pub upload_type: Option<String>,
}

#[derive(Debug, Deserialize, Tabled)]
pub struct UploadedAgent {
    #[tabled(rename = "AGENT ID", display = "opt_dash")]
    #[serde(default)]
    pub agent_id: Option<String>,
    #[tabled(rename = "NAME", display = "opt_dash")]
    #[serde(default)]
    pub agent_name: Option<String>,
    #[tabled(inline)]
    #[serde(default)]
    pub upload_info: Option<UploadInfo>,
    #[tabled(rename = "URL", display = "opt_dash")]
    #[serde(default)]
    pub url: Option<String>,
}

/// Response from `DELETE /agents/{id}` — full teardown: every container for
/// the agent is destroyed and the catalog row itself is deleted.
#[derive(Debug, Deserialize)]
pub struct DeletedAgent {
    #[serde(default)]
    pub containers_stopped: usize,
    #[serde(default)]
    pub runtime_errors: Vec<String>,
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

#[derive(Debug, Deserialize)]
pub struct UpdateQueued {
    pub build_id: String,
    pub agent_id: String,
    pub new_version: String,
    pub previous_version: String,
    pub status: String,
}

#[derive(Debug, Deserialize, Tabled)]
pub struct AgentVersion {
    #[tabled(rename = "VERSION")]
    pub version: String,
    #[tabled(rename = "STATUS")]
    pub status: String,
    #[tabled(rename = "ACTIVE")]
    pub is_active: bool,
    #[tabled(rename = "CAN ROLLBACK")]
    pub can_rollback: bool,
    #[tabled(rename = "PREV VERSION", display = "opt_dash")]
    #[serde(default)]
    pub previous_version: Option<String>,
    #[tabled(rename = "CHANGELOG", display = "opt_dash")]
    #[serde(default)]
    pub changelog: Option<String>,
    #[tabled(rename = "CREATED")]
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct DeploySpec {
    pub image: String,
    pub name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<u16>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub env: std::collections::HashMap<String, String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub writable: bool,
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

    // ─── get_json_optional: 404 → None (stale-binding recovery), else normal ────

    #[test]
    fn get_json_optional_none_on_404() {
        // A deleted/never-existed resource (e.g. a stale local agent binding after
        // a DB reset) must come back as None so callers can recover, not error.
        let mut srv = mockito::Server::new();
        let m = srv
            .mock("GET", "/api/agents/ghost")
            .with_status(404)
            .with_body("not found")
            .create();
        let client = Client::for_test(&srv.url(), None);
        let out: Option<serde_json::Value> = client.get_json_optional("/agents/ghost").unwrap();
        assert!(out.is_none());
        m.assert();
    }

    #[test]
    fn get_json_optional_some_on_200() {
        let mut srv = mockito::Server::new();
        srv.mock("GET", "/api/agents/live")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"live","version":"1.0.0"}"#)
            .create();
        let client = Client::for_test(&srv.url(), None);
        let out: Option<serde_json::Value> = client.get_json_optional("/agents/live").unwrap();
        assert_eq!(out.unwrap()["version"], "1.0.0");
    }

    #[test]
    fn get_json_optional_errors_on_500() {
        // Real failures must still surface — only 404 is treated as "absent".
        let mut srv = mockito::Server::new();
        srv.mock("GET", "/api/agents/boom")
            .with_status(500)
            .with_body("boom")
            .create();
        let client = Client::for_test(&srv.url(), None);
        let out: Result<Option<serde_json::Value>> = client.get_json_optional("/agents/boom");
        assert!(out.is_err());
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
        let p = search_query(
            Some("healthy eating"),
            Some("skill"),
            Some("a2a"),
            10,
            Some(0.3),
        );
        assert_eq!(
            p,
            "/v1/search?q=healthy%20eating&type=skill&framework=a2a&min_score=0.3&limit=10"
        );
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
        let url = blob_put_url(
            "https://cp.example.com",
            "https://cp.example.com/v2/repo/blobs/uploads/abc",
            "sha256:deadbeef",
        );
        assert_eq!(
            url,
            "https://cp.example.com/v2/repo/blobs/uploads/abc?digest=sha256:deadbeef"
        );
    }

    #[test]
    fn blob_put_url_absolute_location_with_existing_query() {
        // Regression: registries that return a Location already carrying a query string
        // (e.g. `?_state=xyz`) must get `&digest=...`, not a second `?digest=...`.
        let url = blob_put_url(
            "https://cp.example.com",
            "https://cp.example.com/v2/repo/blobs/uploads/abc?_state=xyz",
            "sha256:deadbeef",
        );
        assert_eq!(
            url,
            "https://cp.example.com/v2/repo/blobs/uploads/abc?_state=xyz&digest=sha256:deadbeef"
        );
    }

    #[test]
    fn blob_put_url_relative_location_no_query() {
        let url = blob_put_url(
            "https://cp.example.com",
            "/v2/repo/blobs/uploads/abc",
            "sha256:deadbeef",
        );
        assert_eq!(
            url,
            "https://cp.example.com/v2/repo/blobs/uploads/abc?digest=sha256:deadbeef"
        );
    }

    #[test]
    fn blob_put_url_relative_location_with_existing_query() {
        let url = blob_put_url(
            "https://cp.example.com",
            "/v2/repo/blobs/uploads/abc?_state=xyz",
            "sha256:deadbeef",
        );
        assert_eq!(
            url,
            "https://cp.example.com/v2/repo/blobs/uploads/abc?_state=xyz&digest=sha256:deadbeef"
        );
    }

    // ─── upload_mcp_connector_zip / poll_mcp_build_status (Step 14) ────────────

    fn write_temp_zip(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, b"not a real zip, just bytes for the multipart body").unwrap();
        path
    }

    #[test]
    fn upload_mcp_connector_zip_sends_multipart_and_returns_ids() {
        let zip_path = write_temp_zip("nasiko-cli-test-upload.zip");
        let mut srv = mockito::Server::new();
        srv.mock("POST", "/api/mcp/connectors/upload")
            .match_header("content-type", mockito::Matcher::Regex("multipart/form-data.*".into()))
            .with_status(202)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":{"connector_id":"c-1","build_id":"b-1"},"status_code":202,"message":"MCP server build queued"}"#)
            .create();
        let client = Client::for_test(&srv.url(), None);
        let env =
            std::collections::HashMap::from([("STRIPE_KEY".to_string(), "sk_test".to_string())]);
        let queued = client
            .upload_mcp_connector_zip(&zip_path, "my-server", "v1", &env)
            .unwrap();
        assert_eq!(queued.connector_id, "c-1");
        assert_eq!(queued.build_id, "b-1");
        let _ = std::fs::remove_file(&zip_path);
    }

    #[test]
    fn upload_mcp_connector_zip_errors_when_zip_path_missing() {
        let client = Client::for_test("http://127.0.0.1:1", None);
        let missing = std::env::temp_dir().join("nasiko-cli-test-does-not-exist.zip");
        let err = client
            .upload_mcp_connector_zip(
                &missing,
                "my-server",
                "v1",
                &std::collections::HashMap::new(),
            )
            .unwrap_err();
        assert!(err.to_string().contains("cannot read"), "got: {err}");
    }

    #[test]
    fn poll_mcp_build_status_returns_ok_once_running() {
        let mut srv = mockito::Server::new();
        srv.mock("GET", "/api/mcp/connectors/c-1/build-status")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":{"build_status":"running","image_tag":"my-image:v1"},"status_code":200,"message":"build status retrieved successfully"}"#)
            .create();
        let client = Client::for_test(&srv.url(), None);
        client.poll_mcp_build_status("c-1").unwrap();
    }

    #[test]
    fn poll_mcp_build_status_errors_with_message_on_failed() {
        let mut srv = mockito::Server::new();
        srv.mock("GET", "/api/mcp/connectors/c-1/build-status")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":{"build_status":"failed","error_msg":"no Dockerfile found"},"status_code":200,"message":"build status retrieved successfully"}"#)
            .create();
        let client = Client::for_test(&srv.url(), None);
        let err = client.poll_mcp_build_status("c-1").unwrap_err();
        assert!(
            err.to_string().contains("no Dockerfile found"),
            "got: {err}"
        );
    }
}
