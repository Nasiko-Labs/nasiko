use serde::{Deserialize, Serialize};

const EMBEDDING_MODEL: &str = "text-embedding-3-small";
const EMBEDDING_URL: &str = "https://api.openai.com/v1/embeddings";

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

pub async fn generate(api_key: &str, text: &str) -> anyhow::Result<Vec<f32>> {
    let client = reqwest::Client::new();
    let resp: EmbedResponse = client
        .post(EMBEDDING_URL)
        .bearer_auth(api_key)
        .json(&EmbedRequest { input: text, model: EMBEDDING_MODEL })
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
