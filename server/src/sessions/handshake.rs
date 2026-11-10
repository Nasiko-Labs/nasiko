//! Configuring a handshake-mode shim before the ACP connect.
//!
//! On a backend that cannot pass session settings as process env, the shim starts with none
//! (`handshake` mode) and takes them
//! from `POST /control/configure`. In `env` mode (Docker, Pod providers) the settings came with
//! the process environment and nothing here changes the turn: [`decide`] answers `Connect`, and
//! so does a pre-handshake shim, which has no `/control/status` (404).
//!
//! [`ControlClient`] is the one place that puts a sandbox's routing headers and the shim bearer
//! on a control request; the restart helper of a cancelled turn uses it too.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use nasiko_runtime::TransportTls;

use super::profile::{FILES_ENV, ProfileFile};

use crate::coding::turn::CodingTurnError;

/// Env keys the shim takes as the `workspace` block, so the harness child never sees the git token.
const REPO_KEY: &str = "WORKSPACE_REPO";
const REPO_TOKEN_KEY: &str = "WORKSPACE_REPO_TOKEN";
const CLONE_TIMEOUT_KEY: &str = "WORKSPACE_CLONE_TIMEOUT_SECS";

/// How long any one control request may take.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
/// A restart is synchronous: the shim's terminate grace plus the spawn.
const RESTART_TIMEOUT: Duration = Duration::from_secs(25);
/// How long a turn waits, beyond any clone, for the harness to run on the session's configuration.
const START_WAIT: Duration = Duration::from_secs(15);
/// The shim's clone budget when the configure body names none (the shim's own default).
const DEFAULT_CLONE_WAIT: Duration = Duration::from_secs(300);
const RESTART_POLL: Duration = Duration::from_millis(250);
/// How much of a refusal's body is kept in the error.
const BODY_EXCERPT_CHARS: usize = 200;

/// Why a control call failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum HandshakeError {
    /// The shim could not be reached (not listening yet): worth retrying.
    #[error("shim control endpoint unreachable: {0}")]
    Unreachable(String),
    /// The TLS handshake failed (an untrusted certificate, a missing client identity): retrying
    /// cannot help.
    #[error("shim TLS handshake failed: {0}")]
    Tls(String),
    /// Any other transport failure.
    #[error("shim control request failed: {0}")]
    Transport(reqwest::Error),
    /// The shim answered a call with a failure status.
    #[error("shim refused {method} {path}: {status} {body_excerpt}")]
    Rejected {
        method: reqwest::Method,
        path: String,
        status: u16,
        body_excerpt: String,
    },
    /// The status or restart body was not the documented shape.
    #[error("shim control reply unreadable: {0}")]
    Malformed(String),
    /// The sandbox's attach URL is not a WebSocket URL a control URL can be derived from.
    #[error("cannot derive a control URL from {0}")]
    BadUrl(String),
    /// The harness exited after the restart, or never came up on the new configuration in time.
    #[error("shim harness did not start after restart: {0}")]
    RestartFailed(String),
    /// The sandbox's TLS material (CA, client identity) could not be used for the control client.
    #[error("unusable sandbox TLS material: {0}")]
    BadTls(String),
    /// The turn's lease was lost before the shim was changed.
    #[error("the turn's lease was lost")]
    Superseded,
}

impl HandshakeError {
    /// Whether a fresh attempt could succeed: only a shim that is not listening yet.
    pub(crate) fn is_retryable(&self) -> bool {
        matches!(self, HandshakeError::Unreachable(_))
    }

    /// The request ran out of time (a restart is polled instead of failed on this).
    fn is_timeout(&self) -> bool {
        matches!(self, HandshakeError::Transport(e) if e.is_timeout())
    }

    /// A shim that predates handshake mode has no `/control/status`.
    fn is_status_not_found(&self) -> bool {
        matches!(self, HandshakeError::Rejected { method, path, status: 404, .. }
            if method == reqwest::Method::GET && path == "/control/status")
    }
}

impl From<HandshakeError> for CodingTurnError {
    fn from(e: HandshakeError) -> Self {
        match e {
            HandshakeError::Superseded => CodingTurnError::Superseded,
            other => CodingTurnError::Setup(other.to_string()),
        }
    }
}

/// Whether a rustls error sits anywhere in `e`'s source chain. A refused certificate surfaces
/// as an I/O error wrapping (possibly several layers of) I/O errors around rustls's, whose
/// `source()` skips the wrapped error, so the wrapped one is followed explicitly. A TLS failure
/// also reports `is_connect`, hence the check comes first.
/// `e` and every error below it, joined: reqwest's own `Display` drops its sources, which hold
/// the reason (an unknown issuer is a CA problem, a refused client certificate an allowlist one).
fn chain(e: &(dyn std::error::Error + 'static)) -> String {
    fn inner(err: &(dyn std::error::Error + 'static), out: &mut Vec<String>) {
        out.push(err.to_string());
        if let Some(next) = err.source() {
            inner(next, out);
        } else if let Some(wrapped) = err
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
        {
            inner(wrapped, out);
        }
    }
    let mut parts = Vec::new();
    inner(e, &mut parts);
    parts.join(": ")
}

fn is_tls_failure(e: &reqwest::Error) -> bool {
    fn holds_rustls(err: &(dyn std::error::Error + 'static)) -> bool {
        if err.is::<rustls::Error>() {
            return true;
        }
        let wrapped = err
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
            .is_some_and(|inner| holds_rustls(inner));
        wrapped || err.source().is_some_and(holds_rustls)
    }
    std::error::Error::source(e).is_some_and(holds_rustls)
}

fn classify(e: reqwest::Error) -> HandshakeError {
    if is_tls_failure(&e) {
        HandshakeError::Tls(chain(&e))
    } else if e.is_connect() {
        HandshakeError::Unreachable(chain(&e))
    } else {
        HandshakeError::Transport(e)
    }
}

/// `GET /control/status`.
#[derive(Debug, Deserialize)]
pub(crate) struct ShimStatus {
    pub mode: String,
    pub configured: bool,
    /// The hash of the configuration the shim has stored.
    pub env_hash: Option<String>,
    pub child: String,
    pub evicting: bool,
    /// The hash of the configuration the running child was started with (absent on older shims).
    #[serde(default)]
    pub running_env_hash: Option<String>,
    /// Counts child spawns (absent on older shims).
    #[serde(default)]
    pub child_generation: Option<u64>,
    /// The directory the shim serves (absent on older shims).
    #[serde(default)]
    pub workspace: Option<String>,
}

/// What a turn must do to the shim before connecting.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// Nothing: the shim already runs this session's configuration.
    Connect,
    /// Send the configuration; the harness starts on it.
    Configure,
    /// Send the new configuration, then replace the running harness so it takes effect.
    ConfigureAndRestart,
    /// The shim stores this configuration but its child runs an older one: replace the child.
    Restart,
    /// The shim stores this configuration and is still cloning or starting its first child.
    Wait,
}

/// What `status` calls for, given the hash of the env this turn wants.
pub(crate) fn decide(status: &ShimStatus, want_hash: &str) -> Action {
    if status.mode != "handshake" {
        return Action::Connect;
    }
    if !status.configured {
        return Action::Configure;
    }
    let running = status.child == "running";
    if status.env_hash.as_deref() == Some(want_hash) {
        if status.child == "waiting" {
            return Action::Wait;
        }
        let stale_child = status
            .running_env_hash
            .as_deref()
            .is_some_and(|h| h != want_hash);
        // A child that exited is respawned by a restart, from the env the shim already stores.
        let exited = status.child == "exited";
        return if (running && stale_child) || exited {
            Action::Restart
        } else {
            Action::Connect
        };
    }
    if running {
        Action::ConfigureAndRestart
    } else {
        Action::Configure
    }
}

/// The repository block of a configure body.
#[derive(Serialize)]
pub(crate) struct WorkspaceBlock {
    pub repo: String,
    pub token: Option<String>,
    pub timeout_secs: Option<u64>,
}

/// The body of `POST /control/configure`. Not `Debug`: it carries the shim bearer and the
/// repository token.
#[derive(Serialize)]
pub(crate) struct Configure {
    pub token: String,
    pub env: HashMap<String, String>,
    pub workspace: Option<WorkspaceBlock>,
    /// The profile's files, written by the shim before the harness starts.
    pub files: Vec<ProfileFile>,
    pub env_hash: String,
}

/// Hex SHA-256 over `key=value\n` lines sorted by key.
pub(crate) fn env_hash(env: &HashMap<String, String>) -> String {
    let mut keys: Vec<&String> = env.keys().collect();
    keys.sort();
    let mut hasher = Sha256::new();
    for key in keys {
        hasher.update(format!("{key}={}\n", env[key]));
    }
    hex::encode(hasher.finalize())
}

/// Build the configure body: the repository keys leave `env` for the workspace block, the
/// profile's files leave it for the `files` block, and the env is hashed. The hash is taken
/// with the files still in the env, so a changed file restarts the harness like a changed
/// variable does.
pub(crate) fn configure_body(bearer: &str, mut env: HashMap<String, String>) -> Configure {
    let repo = env.remove(REPO_KEY).filter(|r| !r.trim().is_empty());
    let token = env.remove(REPO_TOKEN_KEY);
    let timeout_secs = env
        .remove(CLONE_TIMEOUT_KEY)
        .and_then(|v| v.trim().parse().ok());
    let workspace = repo.map(|repo| WorkspaceBlock {
        repo,
        token,
        timeout_secs,
    });
    let env_hash = env_hash(&env);
    let files = env
        .remove(FILES_ENV)
        .map(|json| {
            serde_json::from_str::<Vec<ProfileFile>>(&json).unwrap_or_else(|e| {
                // The server wrote this itself (`HarnessProfile::apply_to`); a parse failure is a
                // bug, and the turn should still run, just without the files.
                tracing::error!(error = %e, "configure body: {FILES_ENV} is not a file list; dropping it");
                Vec::new()
            })
        })
        .unwrap_or_default();
    Configure {
        token: bearer.to_owned(),
        env_hash,
        env,
        workspace,
        files,
    }
}

/// The provider's routing headers without any `Authorization`, which only the shim bearer may be.
pub(crate) fn sanitize_routing(
    routing: Vec<(String, String)>,
    session_id: &str,
) -> Vec<(String, String)> {
    let before = routing.len();
    let kept: Vec<_> = routing
        .into_iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case("authorization"))
        .collect();
    if kept.len() != before {
        tracing::warn!(%session_id, "sandbox routing headers carried Authorization; dropped");
    }
    kept
}

/// The headers of the ACP upgrade and of every control call: the routing headers, then the shim
/// bearer.
pub(crate) fn upgrade_headers(routing: &[(String, String)], bearer: &str) -> Vec<(String, String)> {
    let mut headers = routing.to_vec();
    headers.push(("Authorization".to_owned(), format!("Bearer {bearer}")));
    headers
}

/// A control URL from the WebSocket URL the sandbox attached us to
/// (`ws://host:7000/acp` becomes `http://host:7000/control/<path>`).
pub(crate) fn control_url(ws_url: &str, path: &str) -> Option<String> {
    let (scheme, rest) = ws_url
        .strip_prefix("ws://")
        .map(|r| ("http://", r))
        .or_else(|| ws_url.strip_prefix("wss://").map(|r| ("https://", r)))?;
    let host = rest.split('/').next()?;
    Some(format!("{scheme}{host}/control/{path}"))
}

/// The HTTP client for one sandbox's control calls: the shared one, or, when the endpoint needs
/// TLS material, a client that also trusts its CA and presents its client identity.
pub(crate) fn control_http(
    shared: &reqwest::Client,
    tls: Option<&TransportTls>,
) -> Result<reqwest::Client, HandshakeError> {
    let Some(tls) = tls else {
        return Ok(shared.clone());
    };
    fn bad(e: &(dyn std::error::Error + 'static)) -> HandshakeError {
        HandshakeError::BadTls(chain(e))
    }
    // An empty CA is "the public roots only" (an endpoint that only wants a client identity).
    let wants_ca = !tls.ca_pem.trim().is_empty();
    let certs = if wants_ca {
        reqwest::Certificate::from_pem_bundle(tls.ca_pem.as_bytes()).map_err(|e| bad(&e))?
    } else {
        Vec::new()
    };
    if wants_ca && certs.is_empty() {
        return Err(HandshakeError::BadTls(
            "the bundle holds no certificate".into(),
        ));
    }
    // Pinned: with the workspace's features unified a native-tls backend can be the default, and
    // it rejects a PEM identity and a private CA.
    let mut builder = reqwest::Client::builder().use_rustls_tls();
    for cert in certs {
        builder = builder.add_root_certificate(cert);
    }
    match (&tls.client_cert_pem, &tls.client_key_pem) {
        (Some(cert), Some(key)) => {
            let identity = reqwest::Identity::from_pem(format!("{cert}\n{key}").as_bytes())
                .map_err(|e| bad(&e))?;
            builder = builder.identity(identity);
        }
        (None, None) => {}
        _ => {
            return Err(HandshakeError::BadTls(
                "a client certificate needs its key, and the reverse".into(),
            ));
        }
    }
    builder.build().map_err(|e| bad(&e))
}

/// The same material in the ACP client's own type.
pub(crate) fn acp_tls(tls: &TransportTls) -> nasiko_acp_client::TlsConfig {
    nasiko_acp_client::TlsConfig {
        ca_pem: tls.ca_pem.clone(),
        client_cert_pem: tls.client_cert_pem.clone(),
        client_key_pem: tls.client_key_pem.clone(),
    }
}

/// HTTP client for one sandbox's shim control endpoints.
pub(crate) struct ControlClient<'a> {
    pub http: &'a reqwest::Client,
    pub base_ws_url: &'a str,
    pub routing: &'a [(String, String)],
    pub bearer: &'a str,
}

impl ControlClient<'_> {
    /// The one place routing headers, the bearer and the timeout go on a control request.
    fn request(
        &self,
        method: reqwest::Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, HandshakeError> {
        let url = control_url(self.base_ws_url, path)
            .ok_or_else(|| HandshakeError::BadUrl(self.base_ws_url.to_owned()))?;
        let mut request = self.http.request(method, url).timeout(CONTROL_TIMEOUT);
        for (name, value) in upgrade_headers(self.routing, self.bearer) {
            request = request.header(name, value);
        }
        Ok(request)
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Configure>,
    ) -> Result<reqwest::Response, HandshakeError> {
        self.send_with(method, path, body, CONTROL_TIMEOUT).await
    }

    async fn send_with(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Configure>,
        timeout: Duration,
    ) -> Result<reqwest::Response, HandshakeError> {
        let mut request = self.request(method.clone(), path)?.timeout(timeout);
        if let Some(body) = body {
            request = request.json(body);
        }
        let resp = request.send().await.map_err(classify)?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        Err(HandshakeError::Rejected {
            method,
            path: format!("/control/{path}"),
            status,
            body_excerpt: body.chars().take(BODY_EXCERPT_CHARS).collect(),
        })
    }

    pub(crate) async fn status(&self) -> Result<ShimStatus, HandshakeError> {
        let resp = self.send(reqwest::Method::GET, "status", None).await?;
        resp.json()
            .await
            .map_err(|e| HandshakeError::Malformed(e.to_string()))
    }

    pub(crate) async fn configure(&self, body: &Configure) -> Result<(), HandshakeError> {
        self.send(reqwest::Method::POST, "configure", Some(body))
            .await?;
        Ok(())
    }

    /// Replace the harness child. The new shim answers once the child is spawned, with its
    /// `child_generation`; an older one answers without a body, which is `None`.
    pub(crate) async fn restart(&self) -> Result<Option<u64>, HandshakeError> {
        let resp = self
            .send_with(reqwest::Method::POST, "restart", None, RESTART_TIMEOUT)
            .await?;
        let text = resp.text().await.unwrap_or_default();
        Ok(serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["child_generation"].as_u64()))
    }
}

/// What [`prepare`] learned about the shim.
pub(crate) struct Prepared {
    /// The backend is draining this sandbox.
    pub evicting: bool,
    /// The directory the shim reported serving; `None` from a shim that does not say.
    pub workspace: Option<String>,
}

/// Bring the shim to this session's configuration, as its status calls for. The lease is checked
/// before anything on the shim is changed.
pub(crate) async fn prepare(
    client: &ControlClient<'_>,
    body: &Configure,
    lease_lost: &AtomicBool,
) -> Result<Prepared, HandshakeError> {
    let status = match client.status().await {
        Ok(status) => status,
        Err(e) if e.is_status_not_found() => {
            tracing::debug!("shim has no /control/status; treating it as env mode");
            return Ok(Prepared {
                evicting: false,
                workspace: None,
            });
        }
        Err(e) => return Err(e),
    };
    let evicting = status.evicting;
    let action = decide(&status, &body.env_hash);
    if action != Action::Connect && lease_lost.load(Ordering::SeqCst) {
        return Err(HandshakeError::Superseded);
    }
    // A configure or restart spawns the next child; the poll expects the generation after this one.
    let expect = status.child_generation.map(|g| g + 1);
    let first_start = status.child == "waiting";
    let first_start_budget = clone_wait(body) + START_WAIT;
    match action {
        Action::Connect => {}
        Action::Configure => {
            client.configure(body).await?;
            // The shim answers at once; its first start (clone, spawn) or respawn runs after.
            let budget = if first_start {
                first_start_budget
            } else {
                START_WAIT
            };
            wait_for_child(client, &body.env_hash, expect, budget, lease_lost).await?;
        }
        Action::Wait => {
            wait_for_child(
                client,
                &body.env_hash,
                expect,
                first_start_budget,
                lease_lost,
            )
            .await?;
        }
        Action::ConfigureAndRestart => {
            client.configure(body).await?;
            restart_and_wait(client, &body.env_hash, expect, lease_lost).await?;
        }
        Action::Restart => restart_and_wait(client, &body.env_hash, expect, lease_lost).await?,
    }
    Ok(Prepared {
        evicting,
        workspace: status.workspace,
    })
}

/// How long the shim may spend cloning the session's repository before it starts the harness.
fn clone_wait(body: &Configure) -> Duration {
    match &body.workspace {
        Some(w) => w
            .timeout_secs
            .map_or(DEFAULT_CLONE_WAIT, Duration::from_secs),
        None => Duration::ZERO,
    }
}

/// Restart, then wait for the child to run the wanted configuration. A restart that times out is
/// not a failure: the shim may still be replacing the child, so the status poll decides.
async fn restart_and_wait(
    client: &ControlClient<'_>,
    want: &str,
    expect: Option<u64>,
    lease_lost: &AtomicBool,
) -> Result<(), HandshakeError> {
    // A restart that timed out may still complete: expect the generation after the one seen.
    let generation = match client.restart().await {
        Ok(generation) => generation.or(expect),
        Err(e) if e.is_timeout() => expect,
        Err(e) => return Err(e),
    };
    wait_for_child(client, want, generation, START_WAIT, lease_lost).await
}

/// Poll status until the harness is running on `want`, or fail if it exited or `budget` passes.
/// `generation` is the child generation the awaited child will have, when known.
async fn wait_for_child(
    client: &ControlClient<'_>,
    want: &str,
    generation: Option<u64>,
    budget: Duration,
    lease_lost: &AtomicBool,
) -> Result<(), HandshakeError> {
    let deadline = Instant::now() + budget;
    loop {
        if lease_lost.load(Ordering::SeqCst) {
            return Err(HandshakeError::Superseded);
        }
        let status = client.status().await?;
        let advanced =
            matches!((generation, status.child_generation), (Some(g), Some(now)) if now >= g);
        // `exited` is the old child until the generation advances (the shim clears it when it
        // spawns the next); a shim reporting no generation cannot tell, so it is a failure.
        if status.child == "exited" && (advanced || status.child_generation.is_none()) {
            return Err(HandshakeError::RestartFailed("the harness exited".into()));
        }
        if status.child == "running" {
            let on_new_config = status.running_env_hash.as_deref() == Some(want);
            // An older shim reports neither field: a running child is all there is to see.
            let unreported = status.running_env_hash.is_none() && status.child_generation.is_none();
            if on_new_config || advanced || unreported {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err(HandshakeError::RestartFailed(format!(
                "no harness on the session's configuration after {}s",
                budget.as_secs()
            )));
        }
        tokio::time::sleep(RESTART_POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::http::{HeaderMap, Method, StatusCode, Uri};
    use axum::response::IntoResponse;
    use axum::routing::any;
    use std::sync::{Arc, Mutex};

    fn status(mode: &str, configured: bool, hash: Option<&str>, child: &str) -> ShimStatus {
        ShimStatus {
            mode: mode.into(),
            configured,
            env_hash: hash.map(Into::into),
            child: child.into(),
            evicting: false,
            running_env_hash: None,
            child_generation: None,
            workspace: None,
        }
    }

    #[test]
    fn the_shim_workspace_decodes_when_reported_and_defaults_to_none() {
        let base = serde_json::json!({
            "mode": "handshake", "configured": true, "child": "running", "evicting": false});
        let mut with = base.clone();
        with["workspace"] = "/workspace/s-x".into();
        let reported: ShimStatus = serde_json::from_value(with).unwrap();
        assert_eq!(reported.workspace.as_deref(), Some("/workspace/s-x"));
        let silent: ShimStatus = serde_json::from_value(base).unwrap();
        assert_eq!(silent.workspace, None);
    }

    fn http() -> reqwest::Client {
        reqwest::Client::builder()
            .use_rustls_tls()
            .no_proxy()
            .build()
            .unwrap()
    }

    #[test]
    fn env_hash_ignores_insertion_order_and_changes_with_values() {
        let a = env_hash(&HashMap::from([
            ("A".into(), "1".into()),
            ("B".into(), "2".into()),
        ]));
        let b = env_hash(&HashMap::from([
            ("B".into(), "2".into()),
            ("A".into(), "1".into()),
        ]));
        assert_eq!(a, b);
        assert_ne!(
            a,
            env_hash(&HashMap::from([
                ("A".into(), "x".into()),
                ("B".into(), "2".into())
            ]))
        );
    }

    /// A renewed sandbox token changes the configuration, so a shim still on the old one is
    /// reconfigured (and its harness restarted) rather than connected to as it is.
    #[test]
    fn a_new_sandbox_token_changes_the_configure_hash() {
        let hash_for = |token: &str| {
            let env = crate::sessions::identity::session_env(token, "", "");
            configure_body("bearer", env).env_hash
        };
        assert_ne!(hash_for("old-token"), hash_for("new-token"));
    }

    #[test]
    fn decision_table() {
        use Action::*;
        assert_eq!(decide(&status("env", false, None, "running"), "h"), Connect);
        assert_eq!(
            decide(&status("handshake", false, None, "waiting"), "h"),
            Configure
        );
        assert_eq!(
            decide(&status("handshake", true, Some("h"), "running"), "h"),
            Connect
        );
        assert_eq!(
            decide(&status("handshake", true, Some("old"), "running"), "h"),
            ConfigureAndRestart
        );
        assert_eq!(
            decide(&status("handshake", true, Some("old"), "exited"), "h"),
            Configure
        );
    }

    #[test]
    fn a_child_on_an_older_configuration_is_restarted_without_a_configure() {
        let mut s = status("handshake", true, Some("h"), "running");
        s.running_env_hash = Some("old".into());
        assert_eq!(decide(&s, "h"), Action::Restart);
        s.running_env_hash = Some("h".into());
        assert_eq!(decide(&s, "h"), Action::Connect);
    }

    #[test]
    fn an_exited_child_on_the_stored_configuration_is_restarted() {
        let s = status("handshake", true, Some("h"), "exited");
        assert_eq!(decide(&s, "h"), Action::Restart);
    }

    fn tls_of(ca: &str, cert: Option<&str>, key: Option<&str>) -> TransportTls {
        TransportTls {
            ca_pem: ca.to_owned(),
            client_cert_pem: cert.map(str::to_owned),
            client_key_pem: key.map(str::to_owned),
        }
    }

    /// A throwaway CA with a server and a client certificate it issued.
    struct Pki {
        ca_pem: String,
        server_chain: Vec<rustls_pki_types::CertificateDer<'static>>,
        server_key: rustls_pki_types::PrivateKeyDer<'static>,
        client_cert_pem: String,
        client_key_pem: String,
    }

    fn pki() -> Pki {
        use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca = CertifiedIssuer::self_signed(ca_params, KeyPair::generate().unwrap()).unwrap();
        let server_key = KeyPair::generate().unwrap();
        let server = CertificateParams::new(vec!["127.0.0.1".to_owned()])
            .unwrap()
            .signed_by(&server_key, &ca)
            .unwrap();
        let client_key = KeyPair::generate().unwrap();
        let client = CertificateParams::new(vec!["client.test".to_owned()])
            .unwrap()
            .signed_by(&client_key, &ca)
            .unwrap();
        Pki {
            ca_pem: ca.pem(),
            server_chain: vec![server.der().clone()],
            server_key: rustls_pki_types::PrivateKeyDer::try_from(server_key.serialize_der())
                .unwrap(),
            client_cert_pem: client.pem(),
            client_key_pem: client_key.serialize_pem(),
        }
    }

    /// An HTTPS stand-in for the shim's `/control/status` (requiring a client certificate when
    /// asked); returns the `wss://` URL the sandbox would attach us to.
    async fn tls_shim(pki: &Pki, require_client: bool) -> String {
        use rustls_pki_types::pem::PemObject as _;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = rustls::ServerConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .unwrap();
        let builder = if require_client {
            let mut roots = rustls::RootCertStore::empty();
            for cert in rustls_pki_types::CertificateDer::pem_slice_iter(pki.ca_pem.as_bytes()) {
                roots.add(cert.unwrap()).unwrap();
            }
            let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
                Arc::new(roots),
                provider,
            )
            .build()
            .unwrap();
            builder.with_client_cert_verifier(verifier)
        } else {
            builder.with_no_client_auth()
        };
        let config = builder
            .with_single_cert(pki.server_chain.clone(), pki.server_key.clone_key())
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("wss://{}/acp", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(tcp).await else {
                        return;
                    };
                    let mut buf = [0u8; 4096];
                    let _ = tls.read(&mut buf).await;
                    let body = shim_status("env", false, "", "", "running").to_string();
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = tls.write_all(reply.as_bytes()).await;
                    let _ = tls.shutdown().await;
                });
            }
        });
        url
    }

    #[test]
    fn a_tls_config_gives_a_per_attach_client_and_a_bad_one_is_refused() {
        let shared = http();
        assert!(control_http(&shared, None).is_ok());
        let bad = tls_of("not a certificate", None, None);
        assert!(matches!(
            control_http(&shared, Some(&bad)),
            Err(HandshakeError::BadTls(_))
        ));
        let pki = pki();
        let good = tls_of(&pki.ca_pem, None, None);
        assert!(control_http(&shared, Some(&good)).is_ok());
        let half = tls_of(&pki.ca_pem, Some(&pki.client_cert_pem), None);
        assert!(matches!(
            control_http(&shared, Some(&half)),
            Err(HandshakeError::BadTls(_))
        ));
        let full = tls_of(
            &pki.ca_pem,
            Some(&pki.client_cert_pem),
            Some(&pki.client_key_pem),
        );
        assert!(
            control_http(&shared, Some(&full)).is_ok(),
            "{:?}",
            control_http(&shared, Some(&full)).err()
        );
        // No CA, only an identity: the public roots alone.
        let identity_only = tls_of("", Some(&pki.client_cert_pem), Some(&pki.client_key_pem));
        assert!(control_http(&shared, Some(&identity_only)).is_ok());
    }

    #[tokio::test]
    async fn the_control_client_needs_the_ca_and_the_identity_an_endpoint_requires() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let pki = pki();
        let url = tls_shim(&pki, true).await;
        let call = |tls: Option<TransportTls>| {
            let url = url.clone();
            async move {
                let http = control_http(&http(), tls.as_ref()).unwrap();
                let client = ControlClient {
                    http: &http,
                    base_ws_url: &url,
                    routing: &[],
                    bearer: "tok",
                };
                client.status().await
            }
        };
        // An untrusted certificate is a TLS failure, which is not retried.
        let err = call(None).await.unwrap_err();
        assert!(matches!(err, HandshakeError::Tls(_)), "{err:?}");
        assert!(err.to_string().contains("UnknownIssuer"), "{err}");
        assert!(!err.is_retryable());
        // Trusted CA but no identity: the endpoint refuses.
        assert!(call(Some(tls_of(&pki.ca_pem, None, None))).await.is_err());
        // With the identity the call goes through.
        let full = tls_of(
            &pki.ca_pem,
            Some(&pki.client_cert_pem),
            Some(&pki.client_key_pem),
        );
        assert_eq!(call(Some(full)).await.unwrap().mode, "env");
    }

    #[test]
    fn upgrade_headers_put_routing_first_then_bearer() {
        let h = upgrade_headers(&[("ate-target-actor".into(), "ns/a".into())], "tok");
        assert_eq!(
            h,
            vec![
                ("ate-target-actor".to_string(), "ns/a".to_string()),
                ("Authorization".to_string(), "Bearer tok".to_string())
            ]
        );
        assert_eq!(upgrade_headers(&[], "tok").len(), 1);
    }

    #[test]
    fn a_routing_authorization_is_dropped() {
        let kept = sanitize_routing(
            vec![
                ("authorization".into(), "x".into()),
                ("ate-target-actor".into(), "ns/a".into()),
            ],
            "s",
        );
        assert_eq!(
            kept,
            vec![("ate-target-actor".to_string(), "ns/a".to_string())]
        );
    }

    #[test]
    fn configure_body_moves_repo_keys_into_the_workspace_block() {
        let env = HashMap::from([
            ("HARNESS_CMD".to_string(), "opencode acp".to_string()),
            ("WORKSPACE_REPO".to_string(), "https://x/y.git".to_string()),
            ("WORKSPACE_REPO_TOKEN".to_string(), "s3cret".to_string()),
            ("WORKSPACE_CLONE_TIMEOUT_SECS".to_string(), "30".to_string()),
            ("OPENAI_API_KEY".to_string(), "t".to_string()),
        ]);
        let body = configure_body("bearer", env);
        assert_eq!(body.token, "bearer");
        for key in [
            "WORKSPACE_REPO",
            "WORKSPACE_REPO_TOKEN",
            "WORKSPACE_CLONE_TIMEOUT_SECS",
        ] {
            assert!(!body.env.contains_key(key), "{key}");
        }
        assert_eq!(body.env["OPENAI_API_KEY"], "t");
        let ws = body.workspace.as_ref().unwrap();
        assert_eq!(ws.repo, "https://x/y.git");
        assert_eq!(ws.token.as_deref(), Some("s3cret"));
        assert_eq!(ws.timeout_secs, Some(30));
        assert_eq!(body.env_hash, env_hash(&body.env));
        assert!(body.files.is_empty());
    }

    #[test]
    fn configure_body_moves_the_profile_files_out_of_the_env_but_hashes_them() {
        let files = vec![ProfileFile {
            path: "/workspace/home/.codex/config.toml".into(),
            content: "model = \"a\"\n".into(),
            mode: None,
        }];
        let env = |content: &str| {
            HashMap::from([
                ("HARNESS_CMD".to_string(), "codex-acp".to_string()),
                (
                    FILES_ENV.to_string(),
                    serde_json::to_string(&[ProfileFile {
                        content: content.into(),
                        ..files[0].clone()
                    }])
                    .unwrap(),
                ),
            ])
        };
        let body = configure_body("bearer", env("model = \"a\"\n"));
        assert!(!body.env.contains_key(FILES_ENV));
        assert_eq!(body.env["HARNESS_CMD"], "codex-acp");
        assert_eq!(body.files, files);
        // The hash covers the files: a changed file is a changed configuration.
        assert_ne!(body.env_hash, env_hash(&body.env));
        let changed = configure_body("bearer", env("model = \"b\"\n"));
        assert_ne!(changed.env_hash, body.env_hash);
        assert_eq!(
            configure_body("bearer", env("model = \"a\"\n")).env_hash,
            body.env_hash
        );
    }

    #[test]
    fn an_unparseable_files_carrier_is_dropped() {
        let env = HashMap::from([
            ("HARNESS_CMD".to_string(), "x".to_string()),
            (FILES_ENV.to_string(), "not json".to_string()),
        ]);
        let body = configure_body("bearer", env);
        assert!(body.files.is_empty());
        assert!(!body.env.contains_key(FILES_ENV));
    }

    #[test]
    fn control_url_maps_ws_to_http() {
        assert_eq!(
            control_url("ws://10.0.0.5:7000/acp", "restart").as_deref(),
            Some("http://10.0.0.5:7000/control/restart")
        );
        assert_eq!(
            control_url("wss://shim.internal/acp", "status").as_deref(),
            Some("https://shim.internal/control/status")
        );
        assert_eq!(control_url("http://x/acp", "status"), None);
    }

    type Seen = Arc<Mutex<Vec<(String, String, Option<String>, Option<String>)>>>;

    /// A shim that applies a restart only after a short delay.
    #[derive(Clone)]
    struct Fake {
        seen: Seen,
        status: Arc<Mutex<serde_json::Value>>,
        /// What the child is after a configure's delayed start: `running` or `exited`.
        after_configure: &'static str,
    }

    async fn fake(
        State(f): State<Fake>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: axum::body::Bytes,
    ) -> axum::response::Response {
        let get = |n: &str| {
            headers
                .get(n)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        f.seen.lock().unwrap().push((
            method.to_string(),
            uri.path().to_owned(),
            get("ate-target-actor"),
            get("authorization"),
        ));
        match uri.path() {
            "/control/status" => axum::Json(f.status.lock().unwrap().clone()).into_response(),
            "/control/configure" => {
                let sent: serde_json::Value = serde_json::from_slice(&body).unwrap();
                {
                    let mut st = f.status.lock().unwrap();
                    st["configured"] = true.into();
                    st["env_hash"] = sent["env_hash"].clone();
                }
                // Like the shim: answer at once, start the child afterwards.
                let (status, after) = (f.status.clone(), f.after_configure);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    let mut st = status.lock().unwrap();
                    st["child"] = after.into();
                    st["running_env_hash"] = st["env_hash"].clone();
                    let next = st["child_generation"].as_u64().unwrap_or(0) + 1;
                    st["child_generation"] = next.into();
                });
                StatusCode::NO_CONTENT.into_response()
            }
            "/control/restart" => {
                let status = f.status.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    let mut st = status.lock().unwrap();
                    st["running_env_hash"] = st["env_hash"].clone();
                    st["child_generation"] = 2.into();
                });
                axum::Json(serde_json::json!({"child_generation": 2})).into_response()
            }
            _ => StatusCode::NOT_FOUND.into_response(),
        }
    }

    async fn serve(app: axum::Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ws = format!("ws://{}/acp", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        ws
    }

    /// Run `prepare` against the fake: its result, the calls it made (status polls collapsed) and
    /// how long it took.
    async fn run_prepare_with(
        status: serde_json::Value,
        body: &Configure,
        after_configure: &'static str,
    ) -> (Result<Prepared, HandshakeError>, Vec<String>, Duration) {
        let seen: Seen = Seen::default();
        let ws = serve(axum::Router::new().fallback(any(fake)).with_state(Fake {
            seen: seen.clone(),
            status: Arc::new(Mutex::new(status)),
            after_configure,
        }))
        .await;
        let http = http();
        let routing = vec![("ate-target-actor".to_owned(), "ns/a".to_owned())];
        let client = ControlClient {
            http: &http,
            base_ws_url: &ws,
            routing: &routing,
            bearer: "tok",
        };
        let started = Instant::now();
        let result = prepare(&client, body, &AtomicBool::new(false)).await;
        let elapsed = started.elapsed();
        let seen = seen.lock().unwrap();
        for (_, _, actor, auth) in seen.iter() {
            assert_eq!(actor.as_deref(), Some("ns/a"));
            assert_eq!(auth.as_deref(), Some("Bearer tok"));
        }
        let mut calls: Vec<String> = seen.iter().map(|(m, p, ..)| format!("{m} {p}")).collect();
        calls.dedup();
        (result, calls, elapsed)
    }

    async fn run_prepare(status: serde_json::Value, body: &Configure) -> (Vec<String>, Duration) {
        let (result, calls, elapsed) = run_prepare_with(status, body, "running").await;
        result.unwrap();
        (calls, elapsed)
    }

    fn body() -> Configure {
        configure_body(
            "tok",
            HashMap::from([("HARNESS_CMD".to_owned(), "x".to_owned())]),
        )
    }

    fn shim_status(
        mode: &str,
        configured: bool,
        hash: &str,
        running: &str,
        child: &str,
    ) -> serde_json::Value {
        serde_json::json!({"mode": mode, "configured": configured,
            "env_hash": if hash.is_empty() { serde_json::Value::Null } else { hash.into() },
            "running_env_hash": if running.is_empty() { serde_json::Value::Null } else { running.into() },
            "child_generation": 1, "child": child, "evicting": false})
    }

    #[tokio::test]
    async fn prepare_returns_the_workspace_the_shim_reports() {
        let mut status = shim_status("handshake", true, "h", "h", "running");
        status["workspace"] = "/workspace/s-x".into();
        let configure = Configure {
            env_hash: "h".into(),
            ..body()
        };
        let (result, ..) = run_prepare_with(status, &configure, "running").await;
        assert_eq!(result.unwrap().workspace.as_deref(), Some("/workspace/s-x"));
    }

    #[tokio::test]
    async fn prepare_waits_for_the_first_start_after_configure_without_a_restart() {
        let (calls, elapsed) =
            run_prepare(shim_status("handshake", false, "", "", "waiting"), &body()).await;
        assert_eq!(
            calls,
            [
                "GET /control/status",
                "POST /control/configure",
                "GET /control/status"
            ]
        );
        assert!(elapsed >= Duration::from_millis(400), "{elapsed:?}");
    }

    #[tokio::test]
    async fn a_configure_on_an_exited_child_waits_for_the_respawn() {
        let (calls, elapsed) =
            run_prepare(shim_status("handshake", true, "old", "", "exited"), &body()).await;
        assert_eq!(
            calls,
            [
                "GET /control/status",
                "POST /control/configure",
                "GET /control/status"
            ]
        );
        assert!(elapsed >= Duration::from_millis(400), "{elapsed:?}");
    }

    #[test]
    fn a_configured_shim_still_starting_is_waited_on() {
        let s = status("handshake", true, Some("h"), "waiting");
        assert_eq!(decide(&s, "h"), Action::Wait);
    }

    #[tokio::test]
    async fn a_harness_that_exits_on_its_first_start_fails_the_prepare() {
        let (result, ..) = run_prepare_with(
            shim_status("handshake", false, "", "", "waiting"),
            &body(),
            "exited",
        )
        .await;
        assert!(matches!(result, Err(HandshakeError::RestartFailed(_))));
    }

    #[tokio::test]
    async fn prepare_waits_for_a_restart_to_take_effect() {
        let (calls, elapsed) = run_prepare(
            shim_status("handshake", true, "old", "old", "running"),
            &body(),
        )
        .await;
        assert_eq!(
            calls,
            [
                "GET /control/status",
                "POST /control/configure",
                "POST /control/restart",
                "GET /control/status"
            ]
        );
        assert!(elapsed >= Duration::from_millis(400), "{elapsed:?}");
    }

    #[tokio::test]
    async fn prepare_restarts_a_child_on_an_older_configuration_without_a_configure() {
        let want = body().env_hash;
        let (calls, elapsed) = run_prepare(
            shim_status("handshake", true, &want, "old", "running"),
            &body(),
        )
        .await;
        assert_eq!(
            calls,
            [
                "GET /control/status",
                "POST /control/restart",
                "GET /control/status"
            ]
        );
        assert!(elapsed >= Duration::from_millis(400), "{elapsed:?}");
    }

    #[tokio::test]
    async fn prepare_leaves_an_env_mode_shim_alone() {
        let (calls, _) = run_prepare(shim_status("env", false, "", "", "running"), &body()).await;
        assert_eq!(calls, ["GET /control/status"]);
    }

    #[tokio::test]
    async fn a_pre_handshake_shim_is_connected_to_as_is() {
        let ws = serve(axum::Router::new().fallback(|| async { StatusCode::NOT_FOUND })).await;
        let http = http();
        let client = ControlClient {
            http: &http,
            base_ws_url: &ws,
            routing: &[],
            bearer: "tok",
        };
        let prepared = prepare(&client, &body(), &AtomicBool::new(false))
            .await
            .unwrap();
        assert!(prepared.workspace.is_none());
    }

    #[tokio::test]
    async fn a_lost_lease_stops_prepare_before_it_changes_the_shim() {
        let ws = serve(axum::Router::new().fallback(any(fake)).with_state(Fake {
            seen: Seen::default(),
            status: Arc::new(Mutex::new(shim_status(
                "handshake",
                false,
                "",
                "",
                "waiting",
            ))),
            after_configure: "running",
        }))
        .await;
        let http = http();
        let client = ControlClient {
            http: &http,
            base_ws_url: &ws,
            routing: &[],
            bearer: "tok",
        };
        let err = prepare(&client, &body(), &AtomicBool::new(true))
            .await
            .err();
        assert!(matches!(err, Some(HandshakeError::Superseded)));
    }

    #[tokio::test]
    async fn a_refused_connection_is_retryable_and_a_401_is_not() {
        let http = http();
        let dead = {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            format!("ws://{}/acp", l.local_addr().unwrap())
        };
        let client = ControlClient {
            http: &http,
            base_ws_url: &dead,
            routing: &[],
            bearer: "tok",
        };
        assert!(client.status().await.unwrap_err().is_retryable());

        let ws = serve(
            axum::Router::new().fallback(|| async { (StatusCode::UNAUTHORIZED, "bad bearer") }),
        )
        .await;
        let client = ControlClient {
            http: &http,
            base_ws_url: &ws,
            routing: &[],
            bearer: "tok",
        };
        let err = client.configure(&body()).await.unwrap_err();
        assert!(!err.is_retryable());
        assert_eq!(
            err.to_string(),
            "shim refused POST /control/configure: 401 bad bearer"
        );
    }
}
