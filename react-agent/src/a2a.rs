use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A2A JSON-RPC client for calling remote agents via the protocol.
#[derive(Clone)]
pub struct A2aClient {
    http: reqwest::Client,
    default_timeout: std::time::Duration,
    request_metadata: Option<serde_json::Value>,
    extra_headers: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2aResponse {
    pub jsonrpc: String,
    pub id: String,
    pub result: Option<serde_json::Value>,
    pub error: Option<A2aJsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2aJsonRpcError {
    pub code: i32,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

impl Default for A2aClient {
    fn default() -> Self {
        Self::new()
    }
}

impl A2aClient {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::new(),
            default_timeout: std::time::Duration::from_secs(30),
            request_metadata: None,
            extra_headers: Vec::new(),
        }
    }

    pub fn with_http_client(http: reqwest::Client) -> Self {
        Self {
            http,
            default_timeout: std::time::Duration::from_secs(30),
            request_metadata: None,
            extra_headers: Vec::new(),
        }
    }

    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Set metadata to inject into `params.metadata` on all outbound A2A requests.
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.request_metadata = Some(metadata);
        self
    }

    /// Set extra HTTP headers on all outbound requests (e.g. traceparent for OTel).
    pub fn with_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.extra_headers = headers;
        self
    }

    /// Send a message to an A2A agent and block until task completion.
    pub async fn send_message(
        &self,
        endpoint: &str,
        message: &str,
        context_id: Option<&str>,
    ) -> Result<A2aResponse, A2aClientError> {
        let ctx = context_id
            .map(|s| s.to_string())
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        let mut body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": Uuid::new_v4().to_string(),
            "method": "SendMessage",
            "params": {
                "message": {
                    "messageId": Uuid::new_v4().to_string(),
                    "role": "ROLE_USER",
                    "parts": [{"text": message}],
                    "contextId": ctx
                }
            }
        });

        if let Some(ref metadata) = self.request_metadata
            && let Some(params) = body.get_mut("params") {
                params.as_object_mut().map(|p| p.insert("metadata".to_string(), metadata.clone()));
            }

        let mut req = self
            .http
            .post(endpoint)
            .header("A2A-Version", "1.0")
            .json(&body)
            .timeout(self.default_timeout);

        for (key, value) in &self.extra_headers {
            req = req.header(key, value);
        }

        let resp = req.send().await
            .map_err(|e| A2aClientError::Network(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(A2aClientError::Http(status.as_u16(), body));
        }

        let a2a_resp: A2aResponse = resp
            .json()
            .await
            .map_err(|e| A2aClientError::InvalidResponse(e.to_string()))?;

        if let Some(ref err) = a2a_resp.error {
            return Err(A2aClientError::A2aProtocol {
                code: err.code,
                message: err.message.clone(),
            });
        }

        Ok(a2a_resp)
    }

    /// Extract text content from an A2A response (artifacts or status message).
    pub fn extract_text(response: &A2aResponse) -> Option<String> {
        let result = response.result.as_ref()?;
        Self::extract_text_from_value(result)
    }

    pub fn extract_text_from_value(result: &serde_json::Value) -> Option<String> {
        // v1.0: result.task.artifacts[].parts[].text
        let task = result.get("task").unwrap_or(result);

        if let Some(artifacts) = task.get("artifacts").and_then(|a| a.as_array()) {
            let text = collect_text_parts(artifacts.iter().filter_map(|a| a.get("parts")));
            if !text.is_empty() {
                return Some(text);
            }
        }

        // v1.0: result.task.status.message.parts[].text
        if let Some(parts) = task
            .pointer("/status/message/parts")
            .and_then(|p| p.as_array())
        {
            let text: Vec<&str> = parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect();
            if !text.is_empty() {
                return Some(text.join("\n"));
            }
        }

        // v0.3 fallback: result.artifacts[].parts[].text
        if let Some(artifacts) = result.get("artifacts").and_then(|a| a.as_array()) {
            let text = collect_text_parts(artifacts.iter().filter_map(|a| a.get("parts")));
            if !text.is_empty() {
                return Some(text);
            }
        }

        if let Some(parts) = result
            .pointer("/status/message/parts")
            .and_then(|p| p.as_array())
        {
            let text: Vec<&str> = parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect();
            if !text.is_empty() {
                return Some(text.join("\n"));
            }
        }

        None
    }
}

fn collect_text_parts<'a>(parts_arrays: impl Iterator<Item = &'a serde_json::Value>) -> String {
    let mut texts = Vec::new();
    for parts_val in parts_arrays {
        if let Some(parts) = parts_val.as_array() {
            for part in parts {
                if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                    texts.push(t);
                }
            }
        }
    }
    texts.join("\n")
}

#[derive(Debug, thiserror::Error)]
pub enum A2aClientError {
    #[error("network error: {0}")]
    Network(String),

    #[error("HTTP {0}: {1}")]
    Http(u16, String),

    #[error("invalid response: {0}")]
    InvalidResponse(String),

    #[error("A2A protocol error ({code}): {message}")]
    A2aProtocol { code: i32, message: String },
}
