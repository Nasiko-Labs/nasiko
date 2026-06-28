//! Canonical embeddings IR — OpenAI `/v1/embeddings` shape. Permissive (passthrough)
//! like the chat IR. Provider impls land in step 9.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::chat::Usage;

fn object_list() -> String {
    "list".to_string()
}
fn object_embedding() -> String {
    "embedding".to_string()
}

/// Inbound embeddings request. `model` is discarded like chat (C4). `input` may be a
/// string, an array of strings, or token-id arrays — kept as raw JSON.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EmbeddingsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub input: Value,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Embeddings response, normalized to OpenAI shape.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EmbeddingsResponse {
    #[serde(default = "object_list")]
    pub object: String,
    pub data: Vec<Embedding>,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Embedding {
    #[serde(default = "object_embedding")]
    pub object: String,
    /// Usually an array of floats; base64 is possible, so kept as raw JSON.
    pub embedding: Value,
    pub index: i64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_round_trips_with_passthrough() {
        let body = json!({
            "model": "text-embedding-3-small",
            "input": ["a", "b"],
            "encoding_format": "float",
            "dimensions": 256
        });
        let req: EmbeddingsRequest = serde_json::from_value(body).unwrap();
        assert_eq!(req.input, json!(["a", "b"]));
        assert!(req.extra.contains_key("encoding_format"));
        let back = serde_json::to_value(&req).unwrap();
        assert_eq!(back["dimensions"], 256);
    }

    #[test]
    fn response_serializes_to_openai_shape() {
        let resp = EmbeddingsResponse {
            object: "list".into(),
            data: vec![Embedding {
                object: "embedding".into(),
                embedding: json!([0.1, 0.2]),
                index: 0,
                extra: Map::new(),
            }],
            model: "text-embedding-3-small".into(),
            usage: Some(Usage {
                prompt_tokens: Some(3),
                completion_tokens: None,
                total_tokens: Some(3),
            }),
            extra: Map::new(),
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["object"], "list");
        assert_eq!(v["data"][0]["object"], "embedding");
        assert_eq!(v["data"][0]["embedding"][1], 0.2);
        assert_eq!(v["usage"]["total_tokens"], 3);
    }
}
