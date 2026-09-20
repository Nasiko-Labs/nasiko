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
            { "id": "gemini/gemini-1.5-pro",                "provider": "gemini" },
            { "id": "groq/openai/gpt-oss-20b",              "provider": "groq" },
            { "id": "groq/openai/gpt-oss-120b",             "provider": "groq" },
            { "id": "mistral/mistral-small-latest",         "provider": "mistral" },
            { "id": "mistral/mistral-large-latest",         "provider": "mistral" },
            { "id": "ollama/llama3.2",                      "provider": "ollama" },
            { "id": "ollama/mistral",                       "provider": "ollama" },
            { "id": "azure/gpt-4o",                         "provider": "azure" },
            { "id": "azure/gpt-4o-mini",                    "provider": "azure" },
            { "id": "nvidia/nvidia/mistral-nemo-minitron-8b-8k-instruct", "provider": "nvidia" },
            { "id": "nvidia/nvidia/llama-3.1-nemotron-70b-instruct", "provider": "nvidia" }
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
        assert_eq!(data.len(), 14);
        assert_eq!(data[0]["id"], "openai/gpt-4o");
        assert_eq!(data[2]["provider"], "anthropic");
        assert_eq!(data[4]["provider"], "groq");
        assert_eq!(data[6]["provider"], "mistral");
        assert_eq!(data[8]["provider"], "ollama");
        assert_eq!(data[10]["provider"], "azure");
        assert_eq!(data[12]["provider"], "nvidia");
        assert!(
            data.iter()
                .all(|e| e["id"].is_string() && e["provider"].is_string())
        );
    }
}
