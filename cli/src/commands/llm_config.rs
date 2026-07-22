//! `nasiko llm-config` — view and change the provider/model an agent routes through
//! the LLM router, plus browse the provider/model catalog.
//!
//! Wraps `GET`/`PATCH /api/agents/{id}/llm-config` (owner/superuser) and
//! `GET /api/llm-router/providers` (the catalog of valid provider/model values).

use anyhow::Result;
use serde_json::{Value, json};

use crate::api::Client;
use crate::commands::agents::resolve_agent_id;

/// `nasiko llm-config get <agent>` — show the agent's current routing config.
pub fn get(agent: &str, json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let agent_id = resolve_agent_id(agent)?;
    let resp: Value = client.get_json(&format!("/agents/{agent_id}/llm-config"))?;

    if json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        return Ok(());
    }

    let inbound = resp
        .get("inbound_format")
        .and_then(|v| v.as_str())
        .unwrap_or("openai");
    println!("LLM config for '{agent}':");
    match resp.get("llm_config") {
        Some(cfg) if cfg.is_object() => {
            print_field("provider", cfg.get("provider"));
            print_field("model", cfg.get("model"));
            let fallbacks = cfg
                .get("fallback_models")
                .and_then(|v| v.as_array())
                .filter(|a| !a.is_empty());
            match fallbacks {
                Some(a) => println!(
                    "  fallback_models    {}",
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                None => println!("  fallback_models    (none)"),
            }
            print_field("temperature", cfg.get("temperature"));
            print_field("max_tokens", cfg.get("max_tokens"));
            print_field("api_key_secret_name", cfg.get("api_key_secret_name"));
            print_field("pinned", cfg.get("pinned"));
            print_field("pinned_model", cfg.get("pinned_model"));
        }
        // llm_config is null ⇒ the agent uses backward-compat defaults (no override set).
        _ => println!("  (unset — using default routing)"),
    }
    println!("  inbound_format     {inbound}");
    Ok(())
}

/// `nasiko llm-config set <agent> --provider … --model …` — update routing config.
/// `provider` and `model` are required by the server; the rest are optional overrides.
#[allow(clippy::too_many_arguments)]
pub fn set(
    agent: &str,
    provider: &str,
    model: &str,
    fallback: Vec<String>,
    temperature: Option<f64>,
    max_tokens: Option<i64>,
    api_key_secret: Option<String>,
    inbound_format: Option<String>,
    pin: bool,
    pinned_model: Option<String>,
) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let agent_id = resolve_agent_id(agent)?;

    let body = json!({
        "provider": provider,
        "model": model,
        "fallback_models": fallback,
        "temperature": temperature,
        "max_tokens": max_tokens,
        "api_key_secret_name": api_key_secret,
        "inbound_format": inbound_format,
        "pinned": pin,
        "pinned_model": pinned_model,
    });

    let _: Value = client.patch_json(&format!("/agents/{agent_id}/llm-config"), &body)?;
    println!("Updated LLM config for '{agent}' → {provider}/{model}");
    if pin {
        let pinned = pinned_model.as_deref().unwrap_or(model);
        println!("  pinned to {pinned} (smart router will not re-select)");
    }
    Ok(())
}

/// `nasiko llm-config providers` — list the provider/model catalog (valid values for `set`).
pub fn providers(json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let resp: Vec<Value> = client.get_json("/llm-router/providers")?;

    if json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        return Ok(());
    }

    if resp.is_empty() {
        println!("No models configured in the catalog.");
        return Ok(());
    }

    println!("Available providers and models (prices in USD per 1M tokens):");
    for provider in &resp {
        let name = provider
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        println!("\n{name}");
        let empty = Vec::new();
        let models = provider
            .get("models")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty);
        for m in models {
            let id = m.get("model").and_then(|v| v.as_str()).unwrap_or("?");
            let input = m
                .get("input_price_per_1m")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let output = m
                .get("output_price_per_1m")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            println!("  {id:<34}  in ${input:<7} out ${output}");
        }
    }
    Ok(())
}

/// Print `  <label>  <value>`, showing `-` for null/missing so the layout stays stable.
fn print_field(label: &str, value: Option<&Value>) {
    let rendered = match value {
        None | Some(Value::Null) => "-".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
    };
    println!("  {label:<18} {rendered}");
}
