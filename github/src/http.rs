use reqwest::{Client, ClientBuilder, RequestBuilder};
use serde::de::DeserializeOwned;
use std::time::Duration;

use crate::error::{Error, Result};

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const USER_AGENT: &str = "Nasiko-Agent-Platform/1.0";

/// Thin async HTTP client shared across service modules.
///
/// Holds a base URL so callers pass only paths (`/user`, `/user/repos`).
/// Authentication is injected per-call because tokens are per-user.
pub(crate) struct HttpClient {
    inner: Client,
    base_url: String,
}

impl HttpClient {
    /// Create a client with the default 30-second timeout and Nasiko user-agent.
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let inner = ClientBuilder::new()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .user_agent(USER_AGENT)
            .build()
            .map_err(Error::Http)?;
        Ok(Self { inner, base_url: base_url.into() })
    }

    // ── raw request builder ────────────────────────────────────────────────

    pub fn get_req(&self, path: &str) -> RequestBuilder {
        self.inner.get(self.url(path))
    }

    // ── typed helpers ──────────────────────────────────────────────────────

    /// GET `path` with a Bearer token; deserialize response body into `T`.
    pub async fn get_authed<T: DeserializeOwned>(&self, path: &str, token: &str) -> Result<T> {
        let resp = self.inner.get(self.url(path)).bearer_auth(token).send().await.map_err(Error::Http)?;
        Self::parse(resp).await
    }

    /// GET `path` with a Bearer token and query params; deserialize into `T`.
    pub async fn get_authed_params<T: DeserializeOwned>(
        &self,
        path: &str,
        token: &str,
        params: &[(&str, &str)],
    ) -> Result<T> {
        let resp = self
            .inner
            .get(self.url(path))
            .bearer_auth(token)
            .query(params)
            .send()
            .await
            .map_err(Error::Http)?;
        Self::parse(resp).await
    }

    /// POST form-encoded data; deserialize response body into `T`.
    /// Used for the GitHub token exchange (no auth header).
    pub async fn post_form<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<T> {
        let resp = self
            .inner
            .post(self.url(path))
            .header("Accept", "application/json")
            .form(params)
            .send()
            .await
            .map_err(Error::Http)?;
        Self::parse(resp).await
    }

    /// Raw send — returns the `reqwest::Response` so callers can inspect
    /// status and body before mapping to a domain error.
    pub async fn send_raw(&self, req: RequestBuilder) -> Result<reqwest::Response> {
        req.send().await.map_err(Error::Http)
    }

    // ── private ────────────────────────────────────────────────────────────

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    /// Deserialize a 2xx response body into `T`.
    ///
    /// Non-2xx responses are mapped by status:
    /// - 401 / 403 → `Error::Auth`
    /// - 404       → `Error::NotFound`
    /// - other     → `Error::HttpStatus { status, body }`
    pub async fn parse<T: DeserializeOwned>(resp: reqwest::Response) -> Result<T> {
        let status = resp.status();
        if status.is_success() {
            return resp.json::<T>().await.map_err(Error::Http);
        }
        let body = resp.text().await.unwrap_or_default();
        Err(match status.as_u16() {
            401 | 403 => Error::Auth(body),
            404 => Error::NotFound(body),
            code => Error::HttpStatus { status: code, body },
        })
    }
}
