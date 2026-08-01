//! `nasiko model-registry` — view and set the platform's tier→model mappings that the
//! smart router resolves against (`model_registry` table). Tiers: 1 = strongest … 3 = smallest.
//!
//! Wraps `GET /api/model-registry` (any authenticated user) and
//! `PUT /api/model-registry` (superuser-only; the server enforces the role).

use anyhow::Result;
use serde_json::{Value, json};

use crate::api::{Client, unwrap_data};

/// `nasiko model-registry ls` — list every configured (provider, tier) → model mapping.
pub fn ls(json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let raw: Value = client.get_json("/model-registry")?;
    let rows: Vec<Value> = unwrap_data(raw)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("No tier→model mappings configured (smart router uses built-in defaults).");
        return Ok(());
    }

    println!("{:<12} {:<6} MODEL", "PROVIDER", "TIER");
    for r in &rows {
        let provider = r.get("provider").and_then(|v| v.as_str()).unwrap_or("?");
        let tier = r.get("tier").and_then(|v| v.as_i64()).unwrap_or(0);
        let model = r.get("model").and_then(|v| v.as_str()).unwrap_or("?");
        println!("{provider:<12} {tier:<6} {model}");
    }
    Ok(())
}

/// `nasiko model-registry set --provider … --tier <1-3> --model …` — upsert one mapping
/// (superuser-only; the server returns 403 otherwise).
pub fn set(provider: &str, tier: i16, model: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let body = json!({ "provider": provider, "tier": tier, "model": model });
    let _: Value = client.put_json("/model-registry", &body)?;
    println!("Set {provider} tier {tier} → {model}");
    Ok(())
}
