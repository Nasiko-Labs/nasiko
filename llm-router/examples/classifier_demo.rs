use nasiko_llm_router::config::GatewayConfig;
use nasiko_llm_router::routing::{ClassifyInput, build_request_classifier};

#[tokio::main]
async fn main() {
    let query = std::env::args()
        .skip(1)
        .collect::<Vec<_>>()
        .join(" ");

    if query.trim().is_empty() {
        eprintln!(
            "Usage: cargo run --release -p nasiko-llm-router --example classifier_demo -- \"<your request>\""
        );
        std::process::exit(1);
    }

    let cfg = GatewayConfig::default();
    let classifier = build_request_classifier(&cfg);
    let result = classifier
        .classify(&ClassifyInput {
            query: &query,
            context: None,
            codebase_size: None,
            cache_affinity: None,
        })
        .await
        .expect("classifier should produce a classification");

    println!(
        "{}",
        serde_json::to_string_pretty(&result.to_demo_json()).expect("serialize demo output")
    );
}
