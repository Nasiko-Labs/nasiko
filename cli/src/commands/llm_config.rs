//! `nasiko llm-config` — manage a per-user library of reusable LLM routing configs and attach
//! them to agents.
//!
//! Wraps `/api/llm-configs*` (the library CRUD + default) and
//! `GET`/`PATCH /api/agents/{id}/llm-config` (attach/detach + resolved view), plus
//! `GET /api/llm-router/providers` (the catalog of valid provider/model values).

use anyhow::Result;
use serde_json::{Value, json};

use crate::api::Client;
use crate::commands::agents::resolve_agent_id;

/// `nasiko llm-config create --name … --provider … --model …` — add a reusable config.
#[allow(clippy::too_many_arguments)]
pub fn create(
    name: &str,
    provider: &str,
    model: &str,
    fallback: Vec<String>,
    temperature: Option<f64>,
    max_tokens: Option<i64>,
    api_key_secret: Option<String>,
    secret_value: Option<String>,
    pin: bool,
    pinned_model: Option<String>,
    default: bool,
) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let body = json!({
        "name": name,
        "provider": provider,
        "model": model,
        "fallback_models": fallback,
        "temperature": temperature,
        "max_tokens": max_tokens,
        "api_key_secret_name": api_key_secret,
        "secret_value": secret_value,
        "pinned": pin,
        "pinned_model": pinned_model,
        "is_default": default,
    });
    let _: Value = client.post_json("/llm-configs", &body)?;
    println!("Created LLM config '{name}' → {provider}/{model}");
    if default {
        println!("  marked as your default");
    }
    if pin {
        let pinned = pinned_model.as_deref().unwrap_or(model);
        println!("  pinned to {pinned} (smart router will not re-select)");
    }
    Ok(())
}

/// `nasiko llm-config list` — show the configs in your library.
pub fn list(json_out: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let configs: Vec<Value> = client.get_json("/llm-configs")?;

    if json_out {
        println!("{}", serde_json::to_string_pretty(&configs)?);
        return Ok(());
    }
    if configs.is_empty() {
        println!("No LLM configs in your library. Create one with `nasiko llm-config create`.");
        return Ok(());
    }
    println!("Your LLM configs:");
    for c in &configs {
        let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let provider = c.get("provider").and_then(|v| v.as_str()).unwrap_or("?");
        let model = c.get("model").and_then(|v| v.as_str()).unwrap_or("?");
        let is_default = c
            .get("is_default")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let marker = if is_default { " (default)" } else { "" };
        println!("  {name:<24} {provider}/{model}{marker}");
    }
    Ok(())
}

/// `nasiko llm-config set-default <name|id>` — mark one of your configs as your default.
pub fn set_default(config: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let id = resolve_config_id(&client, config)?;
    client.post_void(&format!("/llm-configs/{id}/default"))?;
    println!("'{config}' is now your default LLM config");
    Ok(())
}

/// `nasiko llm-config attach <agent> <name|id>` — attach one of your configs to your agent.
pub fn attach(agent: &str, config: &str, inbound_format: Option<String>) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let agent_id = resolve_agent_id(agent)?;
    let id = resolve_config_id(&client, config)?;
    let body = json!({ "llm_config_id": id, "inbound_format": inbound_format });
    let _: Value = client.patch_json(&format!("/agents/{agent_id}/llm-config"), &body)?;
    println!("Attached config '{config}' to agent '{agent}'");
    Ok(())
}

/// `nasiko llm-config detach <agent>` — detach the config (falls back to your default).
pub fn detach(agent: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let agent_id = resolve_agent_id(agent)?;
    // Explicit JSON null ⇒ detach (vs. an absent field, which leaves it unchanged).
    let body = json!({ "llm_config_id": Value::Null });
    let _: Value = client.patch_json(&format!("/agents/{agent_id}/llm-config"), &body)?;
    println!("Detached the config from agent '{agent}' (now uses your default, if any)");
    Ok(())
}

/// `nasiko llm-config get <agent>` — show the agent's resolved routing config.
pub fn get(agent: &str, json_out: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let agent_id = resolve_agent_id(agent)?;
    let resp: Value = client.get_json(&format!("/agents/{agent_id}/llm-config"))?;

    if json_out {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        return Ok(());
    }

    let inbound = resp
        .get("inbound_format")
        .and_then(|v| v.as_str())
        .unwrap_or("openai");
    let source = resp
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or("none");
    println!("LLM config for '{agent}' (source: {source}):");
    match resp.get("llm_config") {
        Some(cfg) if cfg.is_object() => {
            print_field("name", cfg.get("name"));
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
        // llm_config is null ⇒ no attached config and no owner default (platform defaults apply).
        _ => println!("  (none — using platform default routing)"),
    }
    println!("  inbound_format     {inbound}");
    Ok(())
}

/// `nasiko llm-config providers` — list the provider/model catalog (valid values for `create`).
pub fn providers(json_out: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let resp: Vec<Value> = client.get_json("/llm-router/providers")?;

    if json_out {
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

/// Resolve a config reference (name or UUID) to its id by matching against the caller's library.
fn resolve_config_id(client: &Client, config: &str) -> Result<String> {
    let configs: Vec<Value> = client.get_json("/llm-configs")?;
    for c in &configs {
        let id = c.get("id").and_then(|v| v.as_str());
        let name = c.get("name").and_then(|v| v.as_str());
        if let Some(id) = id
            && (Some(config) == name || Some(config) == Some(id))
        {
            return Ok(id.to_string());
        }
    }
    anyhow::bail!("no LLM config named '{config}' in your library")
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
