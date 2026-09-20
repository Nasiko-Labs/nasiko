//! Dev/test helper: mint a Nasiko agent-identity JWT.
//!
//! ```sh
//! AGENT_JWT_SECRET=dev-secret \
//!   cargo run -p nasiko-llm-router --example mint_token -- <agent_id> [owner_id] [ttl_seconds]
//! ```
//! Prints the token to stdout. Use it as the agent's `OPENAI_API_KEY` for smoke tests.

use jsonwebtoken::Algorithm;
use nasiko_llm_router::auth::{DEFAULT_TTL_SECONDS, mint_agent_token, parse_algorithm};

fn main() {
    let mut args = std::env::args().skip(1);

    let Some(agent_id) = args.next() else {
        eprintln!("usage: mint_token <agent_id> [owner_id] [ttl_seconds]");
        std::process::exit(2);
    };
    let owner_id = args.next().unwrap_or_default();
    let ttl = args
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_TTL_SECONDS);

    let secret = std::env::var("AGENT_JWT_SECRET").unwrap_or_default();
    if secret.is_empty() {
        eprintln!("AGENT_JWT_SECRET must be set");
        std::process::exit(2);
    }
    let algorithm: Algorithm =
        parse_algorithm(&std::env::var("AGENT_JWT_ALGORITHM").unwrap_or_else(|_| "HS256".into()));

    match mint_agent_token(&agent_id, &owner_id, &secret, ttl, algorithm) {
        Ok(token) => println!("{token}"),
        Err(e) => {
            eprintln!("mint failed: {e}");
            std::process::exit(1);
        }
    }
}
