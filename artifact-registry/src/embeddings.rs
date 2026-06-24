use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct EmbedRequest<'a> {
    input: &'a str,
    model: &'a str,
}

#[derive(Deserialize)]
struct EmbedResponse {
    data: Vec<EmbedData>,
}

#[derive(Deserialize)]
struct EmbedData {
    embedding: Vec<f32>,
}

/// Generate an embedding for `text` using an OpenAI-compatible `/embeddings` endpoint.
/// `base_url` is the API root (e.g. `https://api.openai.com/v1`), `model` the embedding model.
pub async fn generate(
    api_key: &str,
    base_url: &str,
    model: &str,
    text: &str,
) -> anyhow::Result<Vec<f32>> {
    let url = format!("{}/embeddings", base_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let resp: EmbedResponse = client
        .post(&url)
        .bearer_auth(api_key)
        .json(&EmbedRequest { input: text, model })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    resp.data
        .into_iter()
        .next()
        .map(|d| d.embedding)
        .ok_or_else(|| anyhow::anyhow!("empty embedding response"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live check against the configured embeddings provider. Skips unless OPENAI_API_KEY
    /// is set (e.g. via the repo `.env`), so CI without a key stays green.
    ///
    ///   OPENAI_API_KEY=sk-... cargo test -p nasiko-artifact-registry -- --nocapture live
    #[tokio::test]
    async fn live_embedding_has_expected_dimension() {
        let Ok(key) = std::env::var("OPENAI_API_KEY") else {
            eprintln!("SKIP: OPENAI_API_KEY not set — skipping live embedding test");
            return;
        };
        if key.starts_with("sk-REPLACE") {
            eprintln!("SKIP: OPENAI_API_KEY is still the placeholder — skipping live embedding test");
            return;
        }
        let base = std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into());
        let model = std::env::var("EMBEDDING_MODEL").unwrap_or_else(|_| "text-embedding-3-small".into());

        let v = generate(&key, &base, &model, "a skill for tracking daily nutrition and calories")
            .await
            .expect("embedding request failed — check the key/billing/model");

        // Default model is 1536-dim (matches the DB column). If you override EMBEDDING_MODEL,
        // this asserts it's non-empty rather than a fixed size.
        assert!(!v.is_empty(), "embedding was empty");
        if model == "text-embedding-3-small" {
            assert_eq!(v.len(), 1536, "text-embedding-3-small must be 1536-dim");
        }

        // Sanity: two related queries should be more similar than two unrelated ones.
        let food_a = generate(&key, &base, &model, "healthy eating and meal planning").await.unwrap();
        let food_b = generate(&key, &base, &model, "nutritious recipes and diet advice").await.unwrap();
        let taxes  = generate(&key, &base, &model, "filing income taxes and deductions").await.unwrap();

        let cos = |a: &[f32], b: &[f32]| -> f32 {
            let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
            let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
            let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
            dot / (na * nb)
        };
        let related = cos(&food_a, &food_b);
        let unrelated = cos(&food_a, &taxes);
        assert!(related > unrelated,
            "expected related food queries ({related:.3}) to beat unrelated tax query ({unrelated:.3})");
    }
}
