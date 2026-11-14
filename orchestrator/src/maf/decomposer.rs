use serde::{Deserialize, Serialize};

/// Client for the external "decompose one compound instruction into atomic
/// sub-queries" service (`MODEL_API_URL`/`MODEL_APIKEY`). Independent of
/// `llm.rs`/`planner.rs` — this is a bespoke `/v1/decompose` endpoint, not an
/// OpenAI-compatible chat API.
#[derive(Clone)]
pub struct DecomposerClient {
    http: reqwest::Client,
    url: String,
    api_key: Option<String>,
}

/// Why a decompose call failed, worded for the person creating the workflow. The service's
/// response body is logged, never carried: it can echo the submitted instruction back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecomposeError {
    /// The service refused this server's credential (HTTP 401 or 403).
    KeyRejected,
    /// Any other non-success status.
    Status(u16),
    /// The service could not be reached, or its reply could not be read.
    Unavailable,
}

impl std::fmt::Display for DecomposeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecomposeError::KeyRejected => f.write_str(
                "Breaking this instruction into steps isn't available: the step-splitting \
                 service rejected this server's MODEL_APIKEY. Ask your admin, or add the steps \
                 yourself.",
            ),
            DecomposeError::Status(code) => write!(
                f,
                "The step-splitting service didn't respond (HTTP {code}). Try again, or add the \
                 steps yourself."
            ),
            DecomposeError::Unavailable => f.write_str(
                "The step-splitting service didn't respond. Try again, or add the steps yourself.",
            ),
        }
    }
}

impl std::error::Error for DecomposeError {}

/// Classifies a non-success status from the service.
fn status_error(status: reqwest::StatusCode) -> DecomposeError {
    match status.as_u16() {
        401 | 403 => DecomposeError::KeyRejected,
        code => DecomposeError::Status(code),
    }
}

#[derive(Serialize)]
struct DecomposeRequest<'a> {
    query: &'a str,
}

#[derive(Deserialize)]
struct DecomposeResponse {
    sub_queries: Vec<String>,
}

impl DecomposerClient {
    pub fn new(http: reqwest::Client, url: String, api_key: Option<String>) -> Self {
        Self { http, url, api_key }
    }

    /// Splits one compound instruction into atomic sub-queries. Returns the
    /// instruction unchanged as a single-element vec if the service reports
    /// nothing to split (empty `sub_queries`).
    pub async fn decompose(&self, query: &str) -> Result<Vec<String>, DecomposeError> {
        // `query` is raw user input — logged at `debug`, not `info`, so it
        // isn't shipped to Loki on every workflow creation.
        tracing::info!(
            query_len = query.len(),
            "decomposer_client: decompose() start"
        );
        tracing::debug!(query, "decomposer_client: query text");
        let start = std::time::Instant::now();
        let mut req = self.http.post(&self.url);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req
            .json(&DecomposeRequest { query })
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "decomposer_client: request failed");
                DecomposeError::Unavailable
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            // The body can echo the instruction back, so it stays at `debug`.
            tracing::warn!(
                status = status.as_u16(),
                "decomposer_client: service refused"
            );
            tracing::debug!(body = %body, "decomposer_client: refusal body");
            return Err(status_error(status));
        }

        let parsed: DecomposeResponse = resp.json().await.map_err(|e| {
            tracing::warn!(error = %e, "decomposer_client: unreadable reply");
            DecomposeError::Unavailable
        })?;

        tracing::info!(
            elapsed_ms = start.elapsed().as_millis() as u64,
            sub_query_count = parsed.sub_queries.len(),
            "decomposer_client: decompose() done"
        );

        if parsed.sub_queries.is_empty() {
            return Ok(vec![query.to_string()]);
        }

        Ok(parsed.sub_queries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_credential_names_the_servers_key_and_the_way_round_it() {
        let error = status_error(reqwest::StatusCode::UNAUTHORIZED);
        assert_eq!(error, DecomposeError::KeyRejected);
        let text = error.to_string();
        assert!(text.contains("MODEL_APIKEY"), "{text}");
        assert!(text.contains("add the steps yourself"), "{text}");
    }

    #[test]
    fn any_other_status_is_reported_by_number_only() {
        let error = status_error(reqwest::StatusCode::BAD_GATEWAY);
        assert_eq!(
            error.to_string(),
            "The step-splitting service didn't respond (HTTP 502). Try again, or add the steps \
             yourself."
        );
    }
}
