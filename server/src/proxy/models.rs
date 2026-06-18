use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// JSONRPC 2.0 request envelope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    pub params: serde_json::Value,
}

/// JSONRPC 2.0 response envelope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Proxy call metadata for audit logging
#[derive(Debug, Clone, Serialize)]
pub struct ProxyCallLog {
    pub caller_id: Uuid,
    pub target_agent_id: Uuid,
    pub method: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub latency_ms: u64,
    pub status: u16,
    pub error: Option<String>,
}
