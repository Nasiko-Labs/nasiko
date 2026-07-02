use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tempfile::TempDir;
use tokio::process::Command;
use tracing::{info, instrument, warn};
use uuid::Uuid;

use crate::config::GitHubConfig;
use crate::error::{Error, Result};
use crate::http::HttpClient;
use crate::models::{
    AccessToken, CloneArchive, GitHubRepo, GitHubTokenResponse, GitHubUser, OAuthStateClaims,
};

type HmacSha256 = Hmac<Sha256>;

const OAUTH_STATE_VERSION: &str = "v1";
const OAUTH_STATE_MAX_AGE_SECS: u64 = 600;

/// Core GitHub integration service.
///
/// Holds two [`HttpClient`] instances (one per host) and the [`GitHubConfig`].
/// No database access — all persistence is the caller's responsibility.
///
/// # Two-host design
///
/// GitHub OAuth uses two different base URLs:
/// - `https://github.com`        — token exchange (`/login/oauth/access_token`)
/// - `https://api.github.com`   — user + repo APIs
///
/// Both clients are constructed in [`GitHubService::new`] and can be
/// overridden for testing via [`GitHubService::with_base_urls`].
pub struct GitHubService {
    cfg: GitHubConfig,
    /// Client targeting `https://github.com`.
    github_client: HttpClient,
    /// Client targeting `https://api.github.com`.
    api_client: HttpClient,
}

impl GitHubService {
    /// Construct using production base URLs.
    pub fn new(cfg: GitHubConfig) -> Result<Self> {
        Self::with_base_urls(cfg, "https://github.com", "https://api.github.com")
    }

    /// Construct with overridable base URLs (used in tests with `mockito`).
    pub fn with_base_urls(
        cfg: GitHubConfig,
        github_base: &str,
        api_base: &str,
    ) -> Result<Self> {
        Ok(Self {
            github_client: HttpClient::new(github_base)?,
            api_client: HttpClient::new(api_base)?,
            cfg,
        })
    }

    // ── OAuth state ─────────────────────────────────────────────────────────

    /// Mint a tamper-proof OAuth `state` parameter.
    ///
    /// Format: `v1.<base64url(payload)>.<hex_hmac_sha256>`
    ///
    /// The payload is a `BTreeMap` serialized with `serde_json` — BTreeMap
    /// guarantees deterministic alphabetical key order, which is required so that
    /// [`verify_state`](Self::verify_state) can recompute an identical byte
    /// string during verification.
    ///
    /// # Security
    /// - Uses a 16-byte CSPRNG nonce (via UUID v4 bytes).
    /// - The signing key is `cfg.oauth_state_secret`; it is never logged.
    #[instrument(skip(self))]
    pub fn build_state(&self, user_id: &str) -> Result<String> {
        let iat = unix_now();
        let nonce = Uuid::new_v4().to_string();

        let mut payload: BTreeMap<&str, serde_json::Value> = BTreeMap::new();
        payload.insert("flow", serde_json::Value::String("connect".into()));
        payload.insert("iat", serde_json::Value::Number(iat.into()));
        payload.insert("nonce", serde_json::Value::String(nonce));
        payload.insert("user_id", serde_json::Value::String(user_id.into()));

        if let Some(ref cb) = self.cfg.central_callback_url {
            payload.insert("gateway_callback_url", serde_json::Value::String(cb.clone()));
        }

        let serialized = serde_json::to_string(&payload)?;
        let encoded = URL_SAFE_NO_PAD.encode(serialized.as_bytes());
        let signature = self.sign_payload(&encoded);

        Ok(format!("{OAUTH_STATE_VERSION}.{encoded}.{signature}"))
    }

    /// Authenticate and decode a GitHub OAuth `state` blob.
    ///
    /// Every failure returns [`Error::InvalidOAuthState`] to avoid leaking
    /// which specific check failed.
    ///
    /// # Security
    /// - Signature verification is constant-time (`Mac::verify_slice`).
    /// - States older than 600 seconds are rejected.
    #[instrument(skip(self))]
    pub fn verify_state(&self, state: &str) -> Result<OAuthStateClaims> {
        let invalid = |reason: &str| Error::InvalidOAuthState(reason.to_string());

        // Split into exactly three segments.
        let parts: Vec<&str> = state.splitn(3, '.').collect();
        if parts.len() != 3 {
            return Err(invalid("expected 3 segments separated by '.'"));
        }
        let (version, encoded_payload, hex_sig) = (parts[0], parts[1], parts[2]);

        if version != OAUTH_STATE_VERSION {
            return Err(invalid("unsupported state version"));
        }

        // Constant-time HMAC verification.
        let sig_bytes =
            hex::decode(hex_sig).map_err(|_| invalid("signature is not valid hex"))?;
        let mut mac = HmacSha256::new_from_slice(self.cfg.oauth_state_secret.as_bytes())
            .map_err(|_| invalid("invalid signing key"))?;
        mac.update(encoded_payload.as_bytes());
        mac.verify_slice(&sig_bytes)
            .map_err(|_| invalid("signature mismatch"))?;

        // Decode and parse the payload.
        let payload_bytes = URL_SAFE_NO_PAD
            .decode(encoded_payload)
            .map_err(|_| invalid("payload is not valid base64url"))?;
        let payload: BTreeMap<String, serde_json::Value> =
            serde_json::from_slice(&payload_bytes)
                .map_err(|_| invalid("payload is not valid JSON"))?;

        // Age check.
        let iat = payload
            .get("iat")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| invalid("missing or invalid 'iat' field"))?;
        let now = unix_now();
        if now.saturating_sub(iat) > OAUTH_STATE_MAX_AGE_SECS {
            return Err(invalid("state has expired"));
        }

        let user_id = payload
            .get("user_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| invalid("missing 'user_id' in state payload"))?
            .to_string();

        Ok(OAuthStateClaims { user_id, issued_at: iat })
    }

    // ── Authorization URL ────────────────────────────────────────────────────

    /// Build the GitHub consent-page URL the user should be redirected to.
    ///
    /// `redirect_uri` is the central callback URL when one is configured,
    /// otherwise `cfg.callback_url`.
    #[instrument(skip(self))]
    pub fn authorization_url(&self, user_id: &str) -> Result<String> {
        let state = self.build_state(user_id)?;
        let redirect_uri = self
            .cfg
            .central_callback_url
            .as_deref()
            .unwrap_or(&self.cfg.callback_url);

        // Use reqwest::Url to encode query values safely.
        let url = reqwest::Url::parse_with_params(
            "https://github.com/login/oauth/authorize",
            &[
                ("client_id", self.cfg.client_id.as_str()),
                ("redirect_uri", redirect_uri),
                ("scope", "repo,user:email"),
                ("state", state.as_str()),
            ],
        )
        .map_err(|e| Error::GitHubOAuth(format!("failed to build authorization URL: {e}")))?;

        info!(user_id, "generated GitHub authorization URL");
        Ok(url.to_string())
    }

    // ── Code exchange ────────────────────────────────────────────────────────

    /// Exchange an OAuth authorization code for an access token and user profile.
    ///
    /// Calls `POST https://github.com/login/oauth/access_token` then
    /// `GET https://api.github.com/user`.
    ///
    /// # Error handling
    /// GitHub may return HTTP 200 with an `{"error": "..."}` body on bad codes.
    /// This is detected and mapped to [`Error::GitHubOAuth`].
    ///
    /// # Security
    /// Never log `code` or the returned `access_token`.
    #[instrument(skip(self, code))]
    pub async fn exchange_code(&self, code: &str) -> Result<(AccessToken, GitHubUser)> {
        let mut form: Vec<(&str, &str)> = vec![
            ("client_id", &self.cfg.client_id),
            ("client_secret", &self.cfg.client_secret),
            ("code", code),
        ];
        if let Some(ref cb) = self.cfg.central_callback_url {
            form.push(("redirect_uri", cb.as_str()));
        }

        let token_resp: GitHubTokenResponse = self
            .github_client
            .post_form("/login/oauth/access_token", &form)
            .await?;

        let token = match token_resp {
            GitHubTokenResponse::Token(t) => t,
            GitHubTokenResponse::Error { error, error_description } => {
                return Err(Error::GitHubOAuth(
                    error_description.unwrap_or(error),
                ));
            }
        };

        let user: GitHubUser = self
            .api_client
            .get_authed("/user", &token.access_token)
            .await
            .map_err(|e| match e {
                // A 401/403 after a successful token exchange means the OAuth
                // flow itself is broken — surface as GitHubOAuth, not GitHubApi.
                Error::Auth(_) => Error::GitHubOAuth(
                    "GitHub rejected the newly issued token on /user".into(),
                ),
                Error::HttpStatus { status, body } => Error::GitHubApi { status, message: body },
                other => other,
            })?;

        info!(login = %user.login, "GitHub code exchange successful");
        Ok((token, user))
    }

    // ── Token validation ─────────────────────────────────────────────────────

    /// Test whether a stored access token is still valid.
    ///
    /// Returns `Ok(true)` on 200, `Ok(false)` on 401/403, `Err` on transport
    /// or 5xx failures.  Does not parse the response body.
    #[instrument(skip(self, token))]
    pub async fn verify_token(&self, token: &str) -> Result<bool> {
        let req = self.api_client.get_req("/user").bearer_auth(token);
        let resp = self.api_client.send_raw(req).await?;
        match resp.status().as_u16() {
            200 => Ok(true),
            401 | 403 => Ok(false),
            code => {
                let body = resp.text().await.unwrap_or_default();
                Err(Error::GitHubApi { status: code, message: body })
            }
        }
    }

    // ── Repository listing ───────────────────────────────────────────────────

    /// List the authenticated user's GitHub repositories (newest-updated first).
    ///
    /// Fetches a single page of up to 100 repos (`per_page=100`).  Full
    /// pagination via `Link` headers is not implemented; document this cap to
    /// callers who need coverage beyond 100 repositories.
    #[instrument(skip(self, token))]
    pub async fn list_repos(&self, token: &str) -> Result<Vec<GitHubRepo>> {
        let repos: Vec<GitHubRepo> = self
            .api_client
            .get_authed_params(
                "/user/repos",
                token,
                &[("sort", "updated"), ("direction", "desc"), ("per_page", "100")],
            )
            .await
            .map_err(|e| match e {
                // Preserve auth errors so the route layer maps them to 401, not 502.
                Error::Auth(_) | Error::Http(_) => e,
                // Promote explicit HTTP-status errors to the GitHubApi variant.
                Error::HttpStatus { status, ref body } => Error::GitHubApi {
                    status,
                    message: body.clone(),
                },
                other => other,
            })?;

        info!(count = repos.len(), "fetched GitHub repositories (capped at 100)");
        Ok(repos)
    }

    // ── Clone to archive ─────────────────────────────────────────────────────

    /// Shallow-clone a repository and return an in-memory `tar.gz` archive.
    ///
    /// The `.git` directory is stripped from the archive.  The caller receives
    /// raw bytes and the suggested S3 key; uploading to object storage is the
    /// caller's responsibility.
    ///
    /// # Security (Int-P0)
    /// The token is injected as an HTTP Basic credential via `-c http.extraHeader`
    /// rather than being embedded in the clone URL.  This prevents it from
    /// appearing in:
    /// - the git reflog
    /// - `~/.git-credentials`
    /// - audit logs that record remote URLs
    ///
    /// **Auth format:** `Authorization: Basic base64("x-access-token:TOKEN")`.
    /// GitHub's git smart HTTP transport requires Basic auth, not Bearer.
    /// Sending Bearer causes a 401 challenge that triggers git's credential
    /// system — which fails with "terminal prompts disabled" when no helper or
    /// askpass is available.  Using Basic auth ensures the first request succeeds
    /// (200) and the credential system is never invoked.
    ///
    /// The base64-encoded credentials appear briefly in process argv; stderr is
    /// scrubbed of both the raw token and the encoded form before any error is
    /// returned.
    ///
    /// # Limits
    /// - Timeout: `cfg.clone_timeout_secs` (default 300 s).
    /// - Size cap: `cfg.clone_max_size_bytes` (default 500 MB).
    #[instrument(skip(self, token), fields(repo = %repo_full_name, branch = %branch))]
    pub async fn clone_to_archive(
        &self,
        token: &str,
        repo_full_name: &str,
        branch: &str,
    ) -> Result<CloneArchive> {
        validate_repo_full_name(repo_full_name)?;
        validate_branch_name(branch)?;

        let tmp = TempDir::new().map_err(Error::Io)?;
        let target = tmp.path().join("repo");

        let clone_url = format!("https://github.com/{repo_full_name}.git");

        // ── Authentication strategy ──────────────────────────────────────────
        //
        // GitHub exposes TWO distinct services that require different auth formats:
        //
        //   REST API  (api.github.com)      → accepts `Authorization: Bearer TOKEN`
        //   git smart HTTP (github.com/*.git) → expects `Authorization: Basic base64("user:TOKEN")`
        //
        // The git smart HTTP protocol is NOT a REST API.  When git contacts
        // `https://github.com/owner/repo.git/info/refs`, GitHub responds with
        // `401 WWW-Authenticate: Basic realm="GitHub"` if credentials are wrong or
        // absent.  Sending `Authorization: Bearer TOKEN` on this endpoint does NOT
        // satisfy the Basic challenge — GitHub still returns 401.
        //
        // That 401 triggers git's credential resolution system.  With no helper,
        // no ASKPASS, and GIT_TERMINAL_PROMPT=0, git has no path to credentials:
        //
        //   fatal: could not read Username for 'https://github.com':
        //          terminal prompts disabled
        //
        // Fix: use Basic auth with "x-access-token" as the username (the
        // conventional placeholder for PAT auth; any non-empty string works).
        // GitHub's git endpoint accepts this on the FIRST request → 200, so git
        // never triggers credential resolution at all.
        //
        // Security properties preserved:
        // - Token is NOT embedded in the clone URL (no reflog / ps / audit leakage).
        // - Token is in argv briefly as part of `-c http.extraHeader=…` (same
        //   exposure window as before, unavoidable without an external askpass file).
        // - BOTH the raw token AND its base64-encoded form are scrubbed from any
        //   error text before surfacing in Error::GitClone.
        let b64_credentials = {
            use base64::engine::general_purpose::STANDARD;
            STANDARD.encode(format!("x-access-token:{token}").as_bytes())
        };
        let auth_header = format!("http.extraHeader=Authorization: Basic {b64_credentials}");

        // ── Defence against credential helper interference ───────────────────
        //
        // 1. `-c credential.helper=` (empty) — disables osxkeychain, libsecret,
        //    GCM, etc. — belt.
        // 2. `env_remove("GIT_ASKPASS")` — IDEs (VS Code, IntelliJ) inject
        //    GIT_ASKPASS pointing to a Node.js/GUI helper on a socket that our
        //    subprocess cannot reach; stripping it prevents ECONNREFUSED errors —
        //    suspenders.
        // 3. `-c` BEFORE `clone` — global git options must precede the subcommand
        //    so the HTTP transport sees them on the first request.
        // 4. `GIT_CONFIG_NOSYSTEM=1` — skip /etc/gitconfig which may carry its
        //    own credential.helper.
        // 5. `GIT_TERMINAL_PROMPT=0` — hard fail rather than hang on any prompt
        //    that slips through.
        // `kill_on_drop(true)` ensures that when the timeout fires and the
        // future is dropped, Tokio kills the child process immediately rather
        // than leaving it running (writing to the now-deleted TempDir and
        // consuming CPU until it happens to notice the path is gone).
        let result = tokio::time::timeout(
            Duration::from_secs(self.cfg.clone_timeout_secs),
            Command::new("git")
                .kill_on_drop(true)
                .arg("-c").arg("credential.helper=")
                .arg("-c").arg(&auth_header)
                .arg("clone")
                .arg("--depth").arg("1")
                .arg("--single-branch")
                .arg("--branch").arg(branch)
                .arg(&clone_url)
                .arg(&target)
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env_remove("GIT_ASKPASS")
                .env_remove("GIT_SSH_COMMAND")
                .output(),
        )
        .await;

        let output = match result {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return Err(Error::GitClone(format!("failed to spawn git: {e}"))),
            Err(_) => {
                return Err(Error::GitClone(format!(
                    "clone timed out after {}s",
                    self.cfg.clone_timeout_secs
                )));
            }
        };

        if !output.status.success() {
            let raw_stderr = String::from_utf8_lossy(&output.stderr).to_string();
            // Scrub both the raw token and its base64-encoded form from error text.
            let scrubbed = scrub_token(&scrub_token(&raw_stderr, token), &b64_credentials);
            return Err(Error::GitClone(scrubbed));
        }

        // Strip the .git directory.
        let git_dir = target.join(".git");
        if git_dir.exists() {
            tokio::fs::remove_dir_all(&git_dir).await.map_err(Error::Io)?;
        }

        // Both `dir_size_bytes` and `pack_tar_gz` perform synchronous recursive
        // filesystem I/O.  Running them on the Tokio executor thread would block
        // it for the full duration of a walk + tar/gzip pass (potentially seconds
        // on repos approaching the 500 MB cap).  Offload both to the blocking
        // thread pool.
        let cap = self.cfg.clone_max_size_bytes;
        let size_path = target.clone();
        let size = tokio::task::spawn_blocking(move || dir_size_bytes(&size_path))
            .await
            .map_err(|e| Error::Io(std::io::Error::other(e)))?;

        if size > cap {
            return Err(Error::GitClone(format!(
                "repo exceeds size cap: {} MB (max {} MB)",
                size / 1024 / 1024,
                cap / 1024 / 1024,
            )));
        }

        let tar_path = target.clone();
        let archive_bytes = tokio::task::spawn_blocking(move || pack_tar_gz(&tar_path))
            .await
            .map_err(|e| Error::Io(std::io::Error::other(e)))?  // JoinError → Io
            ?; // inner Result<Vec<u8>, Error>
        let s3_key = format!("github/{repo_full_name}/{branch}.tar.gz");

        info!(
            repo = repo_full_name,
            branch,
            archive_bytes = archive_bytes.len(),
            "repository cloned and archived"
        );

        Ok(CloneArchive { archive_bytes, s3_key })
    }

    // ── Input validation (lightweight, no network) ───────────────────────────

    /// Validate clone inputs and return the suggested S3 key without performing
    /// any network or filesystem operations.
    ///
    /// Used by route handlers that need to surface 422 on bad inputs before
    /// the MinIO upload path is available.  Once the upload path is wired,
    /// callers should switch directly to [`clone_to_archive`](Self::clone_to_archive).
    pub fn validate_clone_request(repo_full_name: &str, branch: &str) -> Result<String> {
        validate_repo_full_name(repo_full_name)?;
        validate_branch_name(branch)?;
        Ok(format!("github/{repo_full_name}/{branch}.tar.gz"))
    }

    // ── Private helpers ──────────────────────────────────────────────────────

    /// Compute HMAC-SHA256 over `encoded_payload` and return the hex digest.
    fn sign_payload(&self, encoded_payload: &str) -> String {
        let mut mac = HmacSha256::new_from_slice(self.cfg.oauth_state_secret.as_bytes())
            .expect("HMAC accepts any key length");
        mac.update(encoded_payload.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }
}

// ── Module-level helpers ─────────────────────────────────────────────────────

/// Current Unix timestamp in seconds.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Recursively sum the size of all files under `dir` in bytes.
fn dir_size_bytes(dir: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                total += dir_size_bytes(&path);
            } else if let Ok(meta) = entry.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

/// Build a `tar.gz` archive of `dir` in memory.
///
/// The archive root is `.` so that extraction produces the repository files
/// directly (no enclosing directory wrapper).
fn pack_tar_gz(dir: &Path) -> Result<Vec<u8>> {
    use flate2::{Compression, write::GzEncoder};
    use tar::Builder;

    let mut buf = Vec::new();
    let enc = GzEncoder::new(&mut buf, Compression::default());
    let mut archive = Builder::new(enc);

    archive.append_dir_all(".", dir).map_err(Error::Io)?;

    let enc = archive.into_inner().map_err(Error::Io)?;
    enc.finish().map_err(Error::Io)?;

    Ok(buf)
}

/// Replace all occurrences of `token` inside `text` with `[REDACTED]`.
///
/// Prevents leaked credentials in `Error::GitClone` messages and tracing spans.
fn scrub_token(text: &str, token: &str) -> String {
    if token.is_empty() {
        return text.to_string();
    }
    text.replace(token, "[REDACTED]")
}

/// Reject `repo_full_name` values that are not exactly `owner/repo` shaped.
///
/// Allowed characters: `[A-Za-z0-9._-]` in each segment, separated by a
/// single `/`.  This is defense-in-depth even though `Command` does not use a
/// shell — it prevents path traversal and arg-injection surprises.
fn validate_repo_full_name(name: &str) -> Result<()> {
    let parts: Vec<&str> = name.splitn(2, '/').collect();
    if parts.len() != 2 {
        return Err(Error::GitClone(format!(
            "invalid repo_full_name {name:?}: expected 'owner/repo'"
        )));
    }
    let valid_segment = |s: &&str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    if !parts.iter().all(valid_segment) {
        return Err(Error::GitClone(format!(
            "invalid repo_full_name {name:?}: segments must match [A-Za-z0-9._-]+"
        )));
    }
    Ok(())
}

/// Reject branch names that contain shell-unsafe or git-unsafe characters.
fn validate_branch_name(branch: &str) -> Result<()> {
    if branch.is_empty() {
        return Err(Error::GitClone("branch name must not be empty".into()));
    }
    if branch
        .chars()
        .any(|c| matches!(c, ' ' | '\t' | '\n' | '\\' | '^' | '~' | ':' | '?' | '*') || c == '\x00')
    {
        return Err(Error::GitClone(format!(
            "branch name {branch:?} contains invalid characters"
        )));
    }
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> GitHubConfig {
        GitHubConfig {
            client_id: "test-client-id".into(),
            client_secret: "test-client-secret".into(),
            callback_url: "https://example.com/api/github/callback".into(),
            oauth_state_secret: "test-oauth-secret-key-32bytes!!".into(),
            central_callback_url: None,
            clone_timeout_secs: 300,
            clone_max_size_bytes: 500 * 1024 * 1024,
        }
    }

    fn test_svc(mock_github_url: &str, mock_api_url: &str) -> GitHubService {
        GitHubService::with_base_urls(test_config(), mock_github_url, mock_api_url).unwrap()
    }

    fn standalone_svc() -> GitHubService {
        GitHubService::with_base_urls(test_config(), "https://github.com", "https://api.github.com").unwrap()
    }

    // ── OAuth state crypto ─────────────────────────────────────────────────

    #[test]
    fn state_round_trip() {
        let svc = standalone_svc();
        let state = svc.build_state("user-abc").unwrap();
        let claims = svc.verify_state(&state).unwrap();
        assert_eq!(claims.user_id, "user-abc");
    }

    #[test]
    fn state_tampered_signature_rejected() {
        let svc = standalone_svc();
        let state = svc.build_state("user-abc").unwrap();
        // Flip the last character of the hex signature.
        let tampered = {
            let mut s = state.clone();
            let last = s.pop().unwrap();
            s.push(if last == 'a' { 'b' } else { 'a' });
            s
        };
        assert!(svc.verify_state(&tampered).is_err());
    }

    #[test]
    fn state_wrong_version_rejected() {
        let svc = standalone_svc();
        let state = svc.build_state("user-abc").unwrap();
        // Replace leading "v1." with "v2.".
        let tampered = state.replacen("v1.", "v2.", 1);
        assert!(svc.verify_state(&tampered).is_err());
    }

    #[test]
    fn state_expired_rejected() {
        use std::collections::BTreeMap;
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

        let svc = standalone_svc();
        // Build a state with iat far in the past.
        let mut payload: BTreeMap<&str, serde_json::Value> = BTreeMap::new();
        payload.insert("flow", serde_json::Value::String("connect".into()));
        payload.insert("iat", serde_json::Value::Number((unix_now() - 601).into()));
        payload.insert("nonce", serde_json::Value::String("test-nonce".into()));
        payload.insert("user_id", serde_json::Value::String("user-abc".into()));

        let serialized = serde_json::to_string(&payload).unwrap();
        let encoded = URL_SAFE_NO_PAD.encode(serialized.as_bytes());
        let sig = svc.sign_payload(&encoded);
        let expired_state = format!("{OAUTH_STATE_VERSION}.{encoded}.{sig}");

        assert!(svc.verify_state(&expired_state).is_err());
    }

    #[test]
    fn authorization_url_contains_client_id() {
        let svc = standalone_svc();
        let url = svc.authorization_url("user-abc").unwrap();
        assert!(url.contains("client_id=test-client-id"), "URL should contain client_id");
        assert!(url.starts_with("https://github.com/login/oauth/authorize"), "Wrong base URL");
    }

    // ── Scrub helper ───────────────────────────────────────────────────────

    #[test]
    fn scrub_token_replaces_all_occurrences() {
        let scrubbed = scrub_token(
            "fatal: could not read Username for 'https://mytoken@github.com': terminal prompts disabled\nmytoken",
            "mytoken",
        );
        assert!(!scrubbed.contains("mytoken"), "token must be scrubbed from error text");
        assert!(scrubbed.contains("[REDACTED]"));
    }

    #[test]
    fn scrub_empty_token_is_noop() {
        let text = "no token here";
        assert_eq!(scrub_token(text, ""), text);
    }

    // ── Validation helpers ─────────────────────────────────────────────────

    #[test]
    fn validate_repo_full_name_accepts_valid() {
        assert!(validate_repo_full_name("owner/repo").is_ok());
        assert!(validate_repo_full_name("my-org/my.repo_1").is_ok());
    }

    #[test]
    fn validate_repo_full_name_rejects_invalid() {
        assert!(validate_repo_full_name("no-slash").is_err());
        assert!(validate_repo_full_name("/leading-slash").is_err());
        assert!(validate_repo_full_name("owner/repo/extra").is_err()); // splitn(2) captures "repo/extra" as segment
        assert!(validate_repo_full_name("owner/../etc").is_err());
    }

    #[test]
    fn validate_branch_name_accepts_valid() {
        assert!(validate_branch_name("main").is_ok());
        assert!(validate_branch_name("feature/my-branch").is_ok());
        assert!(validate_branch_name("v1.2.3").is_ok());
    }

    #[test]
    fn validate_branch_name_rejects_invalid() {
        assert!(validate_branch_name("").is_err());
        assert!(validate_branch_name("bad branch").is_err());
        assert!(validate_branch_name("bad~branch").is_err());
    }

    // ── HTTP tests (mockito) ───────────────────────────────────────────────

    #[tokio::test]
    async fn verify_token_returns_true_on_200() {
        let mut server = mockito::Server::new_async().await;
        let mock = server.mock("GET", "/user").with_status(200).with_body("{}").create_async().await;

        let svc = test_svc("https://github.com", &server.url());
        assert!(svc.verify_token("valid-token").await.unwrap());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn verify_token_returns_false_on_401() {
        let mut server = mockito::Server::new_async().await;
        let mock = server.mock("GET", "/user").with_status(401).with_body("{}").create_async().await;

        let svc = test_svc("https://github.com", &server.url());
        assert!(!svc.verify_token("expired-token").await.unwrap());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn list_repos_deserializes_response() {
        let body = r#"[
            {"id":1,"name":"repo1","full_name":"owner/repo1","description":null,"private":false,
             "clone_url":"https://github.com/owner/repo1.git","ssh_url":"git@github.com:owner/repo1.git",
             "html_url":"https://github.com/owner/repo1","default_branch":"main","updated_at":"2024-01-01T00:00:00Z"},
            {"id":2,"name":"repo2","full_name":"owner/repo2","description":"A repo","private":true,
             "clone_url":"https://github.com/owner/repo2.git","ssh_url":"git@github.com:owner/repo2.git",
             "html_url":"https://github.com/owner/repo2","default_branch":"develop","updated_at":"2024-01-02T00:00:00Z"}
        ]"#;

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", mockito::Matcher::Regex(r"^/user/repos".to_string()))
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("sort".into(), "updated".into()),
                mockito::Matcher::UrlEncoded("direction".into(), "desc".into()),
                mockito::Matcher::UrlEncoded("per_page".into(), "100".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await;

        let svc = test_svc("https://github.com", &server.url());
        let repos = svc.list_repos("some-token").await.unwrap();
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].name, "repo1");
        assert_eq!(repos[1].name, "repo2");
        assert!(repos[1].private);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn exchange_code_github_error_body_mapped() {
        // GitHub returns 200 with an error payload on bad codes.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/login/oauth/access_token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":"bad_verification_code","error_description":"The code passed is incorrect or expired."}"#)
            .create_async()
            .await;

        let svc = test_svc(&server.url(), "https://api.github.com");
        let err = svc.exchange_code("invalid-code").await.unwrap_err();
        assert!(
            matches!(err, Error::GitHubOAuth(_)),
            "expected GitHubOAuth, got {err:?}"
        );
        mock.assert_async().await;
    }

    // ── OAuth state — additional edge cases ────────────────────────────────

    #[test]
    fn state_expired_error_message_says_expired() {
        // The existing state_expired_rejected test asserts is_err().
        // This test verifies the *reason* is expiry, not a signature failure —
        // i.e., a correctly-signed-but-old state is rejected at the age check.
        let svc = standalone_svc();
        let old_iat = unix_now().saturating_sub(700); // 700s ago, 100s past the 600s window

        let mut payload: BTreeMap<&str, serde_json::Value> = BTreeMap::new();
        payload.insert("flow", serde_json::Value::String("connect".into()));
        payload.insert("iat", serde_json::Value::Number(old_iat.into()));
        payload.insert("nonce", serde_json::Value::String("old-nonce".into()));
        payload.insert("user_id", serde_json::Value::String("user-abc".into()));

        let serialized = serde_json::to_string(&payload).unwrap();
        let encoded = URL_SAFE_NO_PAD.encode(serialized.as_bytes());
        // Use the private sign_payload — accessible from this child module.
        let sig = svc.sign_payload(&encoded);
        let state = format!("{OAUTH_STATE_VERSION}.{encoded}.{sig}");

        let err = svc.verify_state(&state).unwrap_err();
        assert!(
            err.to_string().contains("expired"),
            "expected 'expired' in error message, got: {err}"
        );
    }

    #[test]
    fn state_invalid_base64_in_payload_rejected() {
        let svc = standalone_svc();
        // The payload segment is not valid base64url.
        let bad = "v1.!!!this_is_not_base64!!.deadbeef00000000000000000000000000000000000000000000000000000000";
        let err = svc.verify_state(bad).unwrap_err();
        assert!(err.to_string().contains("invalid OAuth state"), "{err}");
    }

    #[test]
    fn state_too_few_segments_rejected() {
        let svc = standalone_svc();
        assert!(svc.verify_state("v1.onlytwo").is_err(), "two segments should fail");
        assert!(svc.verify_state("noseparatorsatall").is_err(), "no separators should fail");
    }

    #[test]
    fn state_missing_user_id_field_rejected() {
        let svc = standalone_svc();
        // Valid sig + valid JSON, but no `user_id` field.
        let mut payload: BTreeMap<&str, serde_json::Value> = BTreeMap::new();
        payload.insert("flow", serde_json::Value::String("connect".into()));
        payload.insert("iat", serde_json::Value::Number(unix_now().into()));
        payload.insert("nonce", serde_json::Value::String("n".into()));
        // deliberately omit user_id

        let serialized = serde_json::to_string(&payload).unwrap();
        let encoded = URL_SAFE_NO_PAD.encode(serialized.as_bytes());
        let sig = svc.sign_payload(&encoded);
        let state = format!("{OAUTH_STATE_VERSION}.{encoded}.{sig}");

        let err = svc.verify_state(&state).unwrap_err();
        assert!(err.to_string().contains("invalid OAuth state"), "{err}");
    }

    #[test]
    fn state_missing_iat_field_rejected() {
        let svc = standalone_svc();
        let mut payload: BTreeMap<&str, serde_json::Value> = BTreeMap::new();
        payload.insert("flow", serde_json::Value::String("connect".into()));
        // deliberately omit iat
        payload.insert("nonce", serde_json::Value::String("n".into()));
        payload.insert("user_id", serde_json::Value::String("u".into()));

        let serialized = serde_json::to_string(&payload).unwrap();
        let encoded = URL_SAFE_NO_PAD.encode(serialized.as_bytes());
        let sig = svc.sign_payload(&encoded);
        let state = format!("{OAUTH_STATE_VERSION}.{encoded}.{sig}");

        let err = svc.verify_state(&state).unwrap_err();
        assert!(err.to_string().contains("invalid OAuth state"), "{err}");
    }

    #[test]
    fn state_non_json_payload_rejected() {
        let svc = standalone_svc();
        // Encode a string that decodes successfully from base64 but is not JSON.
        let encoded = URL_SAFE_NO_PAD.encode(b"not-json-at-all");
        let sig = svc.sign_payload(&encoded);
        let state = format!("{OAUTH_STATE_VERSION}.{encoded}.{sig}");

        let err = svc.verify_state(&state).unwrap_err();
        assert!(err.to_string().contains("invalid OAuth state"), "{err}");
    }

    #[test]
    fn state_json_array_payload_rejected() {
        let svc = standalone_svc();
        // A JSON array is valid JSON but not an object (BTreeMap deserialization fails).
        let encoded = URL_SAFE_NO_PAD.encode(b"[1,2,3]");
        let sig = svc.sign_payload(&encoded);
        let state = format!("{OAUTH_STATE_VERSION}.{encoded}.{sig}");

        let err = svc.verify_state(&state).unwrap_err();
        assert!(err.to_string().contains("invalid OAuth state"), "{err}");
    }

    #[test]
    fn state_payload_keys_are_sorted_for_determinism() {
        // Serialization must use BTreeMap (sorted keys) so that verify_state
        // can recompute the exact same byte string for the HMAC check.
        // This test confirms that two states built from the same logical payload
        // produce identical encoded payloads (deterministic ordering).
        let svc = standalone_svc();
        // We verify this indirectly: verify_state succeeds only if the
        // signing and verification routines agree on the serialized form.
        // A HashMap-based serializer could produce different orderings.
        for _ in 0..10 {
            let state = svc.build_state("user-sorted").unwrap();
            let claims = svc.verify_state(&state).unwrap();
            assert_eq!(claims.user_id, "user-sorted");
        }
    }

    // ── Authorization URL — additional checks ──────────────────────────────

    #[test]
    fn authorization_url_contains_all_required_query_params() {
        let svc = standalone_svc();
        let url = svc.authorization_url("user-1").unwrap();

        // Parse the URL so we can inspect params reliably.
        let parsed = reqwest::Url::parse(&url).unwrap();
        let params: std::collections::HashMap<_, _> = parsed.query_pairs().collect();

        assert!(params.contains_key("client_id"), "missing client_id");
        assert!(params.contains_key("redirect_uri"), "missing redirect_uri");
        assert!(params.contains_key("scope"), "missing scope");
        assert!(params.contains_key("state"), "missing state");
    }

    #[test]
    fn authorization_url_scope_includes_repo_and_user_email() {
        let svc = standalone_svc();
        let url = svc.authorization_url("user-1").unwrap();
        let parsed = reqwest::Url::parse(&url).unwrap();
        let scope = parsed
            .query_pairs()
            .find(|(k, _)| k == "scope")
            .map(|(_, v)| v.to_string())
            .unwrap_or_default();

        assert!(scope.contains("repo"), "scope must include 'repo', got: {scope}");
        assert!(scope.contains("user"), "scope must include 'user', got: {scope}");
    }

    #[test]
    fn authorization_url_redirect_uri_uses_callback_url() {
        let svc = standalone_svc();
        let url = svc.authorization_url("user-1").unwrap();
        let parsed = reqwest::Url::parse(&url).unwrap();
        let redirect_uri = parsed
            .query_pairs()
            .find(|(k, _)| k == "redirect_uri")
            .map(|(_, v)| v.to_string())
            .unwrap_or_default();

        assert!(
            redirect_uri.contains("example.com"),
            "redirect_uri should reference the configured callback URL, got: {redirect_uri}"
        );
    }

    #[test]
    fn authorization_url_uses_central_callback_url_when_configured() {
        let cfg = GitHubConfig {
            central_callback_url: Some("https://central.example.com/github/callback".into()),
            ..test_config()
        };
        let svc = GitHubService::with_base_urls(
            cfg,
            "https://github.com",
            "https://api.github.com",
        )
        .unwrap();

        let url = svc.authorization_url("user-1").unwrap();
        let parsed = reqwest::Url::parse(&url).unwrap();
        let redirect_uri = parsed
            .query_pairs()
            .find(|(k, _)| k == "redirect_uri")
            .map(|(_, v)| v.to_string())
            .unwrap_or_default();

        assert!(
            redirect_uri.contains("central.example.com"),
            "redirect_uri must be the central callback URL, got: {redirect_uri}"
        );
    }

    // ── Token verification — edge cases ───────────────────────────────────

    #[tokio::test]
    async fn verify_token_returns_false_on_403() {
        let mut server = mockito::Server::new_async().await;
        let mock = server.mock("GET", "/user").with_status(403).with_body("{}").create_async().await;

        let svc = test_svc("https://github.com", &server.url());
        let result = svc.verify_token("insufficient-scope-token").await.unwrap();
        assert!(!result, "403 Forbidden must be treated as an invalid/insufficient token");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn verify_token_returns_err_on_server_error() {
        let mut server = mockito::Server::new_async().await;
        let mock = server.mock("GET", "/user").with_status(500).with_body("Internal Server Error").create_async().await;

        let svc = test_svc("https://github.com", &server.url());
        let result = svc.verify_token("any-token").await;
        // 5xx must propagate as an Err, not as Ok(false).
        assert!(result.is_err(), "server error should be Err, not Ok(false)");
        mock.assert_async().await;
    }

    // ── Repository listing — edge cases ───────────────────────────────────

    #[tokio::test]
    async fn list_repos_returns_empty_vec_for_empty_array() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", mockito::Matcher::Regex(r"^/user/repos".to_string()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("[]")
            .create_async()
            .await;

        let svc = test_svc("https://github.com", &server.url());
        let repos = svc.list_repos("token").await.unwrap();
        assert!(repos.is_empty(), "empty array response must yield empty Vec");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn list_repos_preserves_auth_error_on_401() {
        // A 401 from GitHub must surface as Error::Auth so the route layer
        // returns HTTP 401 rather than 502.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", mockito::Matcher::Regex(r"^/user/repos".to_string()))
            .with_status(401)
            .with_body(r#"{"message":"Bad credentials"}"#)
            .create_async()
            .await;

        let svc = test_svc("https://github.com", &server.url());
        let err = svc.list_repos("bad-token").await.unwrap_err();
        assert!(
            matches!(err, Error::Auth(_)),
            "401 from GitHub repos must be Error::Auth, got: {err:?}"
        );
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn list_repos_returns_github_api_error_on_500() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", mockito::Matcher::Regex(r"^/user/repos".to_string()))
            .with_status(500)
            .with_body("server blew up")
            .create_async()
            .await;

        let svc = test_svc("https://github.com", &server.url());
        let err = svc.list_repos("token").await.unwrap_err();
        assert!(
            matches!(err, Error::GitHubApi { status: 500, .. }),
            "500 must be Error::GitHubApi {{status:500}}, got: {err:?}"
        );
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn list_repos_returns_err_on_malformed_json() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", mockito::Matcher::Regex(r"^/user/repos".to_string()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("not-json{{{")
            .create_async()
            .await;

        let svc = test_svc("https://github.com", &server.url());
        let result = svc.list_repos("token").await;
        assert!(result.is_err(), "malformed JSON must be Err");
        mock.assert_async().await;
    }

    // ── Rate limiting ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_repos_surfaces_rate_limit_as_github_api_error() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", mockito::Matcher::Regex(r"^/user/repos".to_string()))
            .with_status(429)
            .with_header("retry-after", "60")
            .with_body(r#"{"message":"API rate limit exceeded"}"#)
            .create_async()
            .await;

        let svc = test_svc("https://github.com", &server.url());
        let err = svc.list_repos("token").await.unwrap_err();
        assert!(
            matches!(err, Error::GitHubApi { status: 429, .. }),
            "429 must be Error::GitHubApi {{status:429}}, got: {err:?}"
        );
        mock.assert_async().await;
    }

    // ── exchange_code — additional ─────────────────────────────────────────

    #[tokio::test]
    async fn exchange_code_maps_api_rejection_of_new_token_to_oauth_error() {
        // The token exchange succeeds (GitHub issues a token), but GET /user
        // with the new token returns 401 — the whole flow is broken.
        let mut gh_server = mockito::Server::new_async().await;
        let mut api_server = mockito::Server::new_async().await;

        let _m1 = gh_server
            .mock("POST", "/login/oauth/access_token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"gho_test","token_type":"bearer"}"#)
            .create_async()
            .await;
        let _m2 = api_server
            .mock("GET", "/user")
            .with_status(401)
            .with_body("{}")
            .create_async()
            .await;

        let svc = test_svc(&gh_server.url(), &api_server.url());
        let err = svc.exchange_code("code").await.unwrap_err();
        assert!(
            matches!(err, Error::GitHubOAuth(_)),
            "token rejected by /user must surface as GitHubOAuth, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn exchange_code_network_failure_propagates_as_http_error() {
        // Point the github client at a port nothing is listening on.
        let svc = test_svc("http://127.0.0.1:1", "http://127.0.0.1:1");
        let err = svc.exchange_code("code").await.unwrap_err();
        assert!(matches!(err, Error::Http(_)), "connection refused must be Error::Http");
    }

    #[tokio::test]
    async fn exchange_code_malformed_success_response_is_err() {
        // GitHub returns 200 but the body is not the expected JSON shape.
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/login/oauth/access_token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("not json at all")
            .create_async()
            .await;

        let svc = test_svc(&server.url(), "http://127.0.0.1:1");
        let result = svc.exchange_code("code").await;
        assert!(result.is_err(), "malformed token response must be Err");
    }

    #[tokio::test]
    async fn exchange_code_success() {
        let token_body = r#"{"access_token":"gho_abc123","token_type":"bearer","scope":"repo,user:email"}"#;
        let user_body = r#"{"id":42,"login":"octocat","name":"The Octocat","email":"octocat@github.com","avatar_url":"https://avatars.githubusercontent.com/u/583231"}"#;

        let mut gh_server = mockito::Server::new_async().await;
        let mut api_server = mockito::Server::new_async().await;

        let m1 = gh_server
            .mock("POST", "/login/oauth/access_token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(token_body)
            .create_async()
            .await;
        let m2 = api_server
            .mock("GET", "/user")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(user_body)
            .create_async()
            .await;

        let svc = test_svc(&gh_server.url(), &api_server.url());
        let (token, user) = svc.exchange_code("good-code").await.unwrap();
        assert_eq!(token.access_token, "gho_abc123");
        assert_eq!(user.login, "octocat");
        m1.assert_async().await;
        m2.assert_async().await;
    }
}
