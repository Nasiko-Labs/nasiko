//! One-off backfill: embed every agent that doesn't have a stored embedding
//! yet. Run with:
//!   DATABASE_URL=... OPENAI_API_KEY=... cargo run -p nasiko-orchestrator --example backfill_embeddings
//! Not wired into any binary or CI path — delete after use.

use nasiko_orchestrator::{AgentCard, embed_and_store_agent};
use sqlx::Row;

#[tokio::main]
async fn main() {
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL required");
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY required");
    let base_url =
        std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com".into());
    let model =
        std::env::var("EMBEDDING_MODEL").unwrap_or_else(|_| "text-embedding-3-small".into());

    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect(&db_url)
        .await
        .expect("connect to database");

    let rows = sqlx::query(
        "SELECT id, name, description, tags FROM agents WHERE embedding IS NULL ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .expect("fetch agents");

    println!("found {} agent(s) without a stored embedding", rows.len());

    let mut ok = 0;
    let mut failed = 0;
    for row in rows {
        let id: uuid::Uuid = row.get("id");
        let name: String = row.get("name");
        let description: Option<String> = row.get("description");
        let tags: Vec<String> = row.get("tags");

        let agent = AgentCard {
            id,
            name: name.clone(),
            description: description.unwrap_or_default(),
            skills: vec![],
            tags,
            url: None,
            embedding: None,
            embedding_content_hash: None,
        };

        match embed_and_store_agent(&pool, &agent, &api_key, &base_url, &model).await {
            Ok(_) => {
                println!("embedded: {name} ({id})");
                ok += 1;
            }
            Err(e) => {
                eprintln!("FAILED: {name} ({id}): {e}");
                failed += 1;
            }
        }
    }

    println!("done — {ok} embedded, {failed} failed");
}
