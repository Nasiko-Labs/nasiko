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
    pub async fn decompose(&self, query: &str) -> Result<Vec<String>, String> {
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
            .map_err(|e| format!("decomposer request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("decomposer HTTP {status}: {body}"));
        }

        let parsed: DecomposeResponse = resp
            .json()
            .await
            .map_err(|e| format!("decomposer response parse error: {e}"))?;

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
