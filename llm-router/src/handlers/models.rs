//! `GET /v1/models` — the static catalog a UI dropdown reads (RUST_PLAN §3). Public:
//! no agent identity needed, it carries no per-agent data.

use axum::Json;
use serde_json::{Value, json};

/// Return the supported provider/model catalog in the exact spec shape.
pub async fn models() -> Json<Value> {
    Json(json!({
        "object": "list",
        "data": [
            { "id": "openai/gpt-4o",                        "provider": "openai" },
            { "id": "openai/gpt-4o-mini",                   "provider": "openai" },
            { "id": "anthropic/claude-3-5-sonnet-20241022", "provider": "anthropic" },
            { "id": "gemini/gemini-1.5-pro",                "provider": "gemini" }
        ]
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn catalog_has_expected_shape() {
        let Json(v) = models().await;
        assert_eq!(v["object"], "list");
        let data = v["data"].as_array().unwrap();
        assert_eq!(data.len(), 4);
        assert_eq!(data[0]["id"], "openai/gpt-4o");
        assert_eq!(data[2]["provider"], "anthropic");
        // every entry has an id and a provider
        assert!(data.iter().all(|e| e["id"].is_string() && e["provider"].is_string()));
    }
}
