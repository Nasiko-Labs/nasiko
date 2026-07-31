//! `nasiko llm-config` — manage a per-user library of reusable LLM routing configs and attach
//! them to agents.
//!
//! Wraps `/api/llm-configs*` (the library CRUD + default) and
//! `GET`/`PATCH /api/agents/{id}/llm-config` (attach/detach + resolved view), plus
//! `GET /api/llm-router/providers` (the catalog of valid provider/model values).

use anyhow::Result;
use serde_json::{Value, json};

use crate::api::{Client, unwrap_data};
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
    let configs: Vec<Value> = unwrap_data(client.get_json("/llm-configs")?)?;

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

/// `nasiko llm-config update <name|id> [flags]` — edit fields of an existing config in place.
///
/// The server's `PATCH /llm-configs/{id}` is a *full replace* of the routing fields, so this
/// fetches the current config and overlays only the flags the caller passed — an unspecified
/// flag leaves that field untouched. `--pin`/`--no-pin` are a tri-state (neither ⇒ keep current);
/// `--fallback` (repeatable) replaces the list, `--clear-fallbacks` empties it, and
/// `--clear-pinned-model` resets the pinned model to null (pin falls back to the config's model).
#[allow(clippy::too_many_arguments)]
pub fn update(
    config: &str,
    name: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    fallback: Vec<String>,
    clear_fallbacks: bool,
    temperature: Option<f64>,
    max_tokens: Option<i64>,
    api_key_secret: Option<String>,
    secret_value: Option<String>,
    pin: bool,
    no_pin: bool,
    pinned_model: Option<String>,
    clear_pinned_model: bool,
) -> Result<()> {
    if pin && no_pin {
        anyhow::bail!("--pin and --no-pin are mutually exclusive");
    }
    if clear_pinned_model && pinned_model.is_some() {
        anyhow::bail!("--clear-pinned-model and --pinned-model are mutually exclusive");
    }
    let client = Client::from_active_cluster()?;
    let existing = fetch_config_by_ref(&client, config)?;
    let id = existing
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("config '{config}' is missing an id"))?
        .to_string();

    let pinned = if pin {
        true
    } else if no_pin {
        false
    } else {
        existing
            .get("pinned")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    };
    let body = build_update_body(
        &existing,
        name,
        provider,
        model,
        fallback,
        clear_fallbacks,
        temperature,
        max_tokens,
        api_key_secret,
        secret_value,
        pinned,
        pinned_model,
        clear_pinned_model,
    );
    let _: Value = client.patch_json(&format!("/llm-configs/{id}"), &body)?;

    let p = body.get("provider").and_then(|v| v.as_str()).unwrap_or("?");
    let m = body.get("model").and_then(|v| v.as_str()).unwrap_or("?");
    println!("Updated LLM config '{config}' → {p}/{m}");
    if body
        .get("pinned")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let pinned_to = body
            .get("pinned_model")
            .and_then(|v| v.as_str())
            .unwrap_or(m);
        println!("  pinned to {pinned_to} (smart router will not re-select)");
    }
    Ok(())
}

/// Build the `PATCH /llm-configs/{id}` body by overlaying the caller's flags over the existing
/// config. Pure (no I/O) so the merge semantics — "unspecified flag keeps the current value" —
/// are unit-tested. `pinned` arrives already resolved from the `--pin`/`--no-pin` tri-state.
#[allow(clippy::too_many_arguments)]
fn build_update_body(
    existing: &Value,
    name: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    fallback: Vec<String>,
    clear_fallbacks: bool,
    temperature: Option<f64>,
    max_tokens: Option<i64>,
    api_key_secret: Option<String>,
    secret_value: Option<String>,
    pinned: bool,
    pinned_model: Option<String>,
    clear_pinned_model: bool,
) -> Value {
    let keep_str = |field: &str| {
        existing
            .get(field)
            .and_then(|v| v.as_str())
            .map(String::from)
    };
    // Explicit clear wins; otherwise a provided value, else keep the current one.
    let pinned_model = if clear_pinned_model {
        None
    } else {
        pinned_model.or_else(|| keep_str("pinned_model"))
    };
    let fallbacks: Vec<String> = if clear_fallbacks {
        Vec::new()
    } else if !fallback.is_empty() {
        fallback
    } else {
        existing
            .get("fallback_models")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    json!({
        // Server COALESCEs a null name to the current one, so an omitted --name is a no-op rename.
        "name": name,
        "provider": provider.or_else(|| keep_str("provider")),
        "model": model.or_else(|| keep_str("model")),
        "fallback_models": fallbacks,
        "temperature": temperature.or_else(|| existing.get("temperature").and_then(|v| v.as_f64())),
        "max_tokens": max_tokens.or_else(|| existing.get("max_tokens").and_then(|v| v.as_i64())),
        "api_key_secret_name": api_key_secret.or_else(|| keep_str("api_key_secret_name")),
        "secret_value": secret_value,
        "pinned": pinned,
        "pinned_model": pinned_model,
    })
}

/// `nasiko llm-config delete <name|id>` — soft-delete a config from your library. The server
/// refuses (409) if it's still attached to any agent, so detach it there first.
pub fn delete(config: &str, force: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let id = resolve_config_id(&client, config)?;
    if !force {
        let confirm = dialoguer::Confirm::new()
            .with_prompt(format!("Delete LLM config '{config}'?"))
            .default(false)
            .interact()?;
        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }
    client.delete(&format!("/llm-configs/{id}"))?;
    println!("Deleted LLM config '{config}'");
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
    let resp: Value =
        unwrap_data(client.get_json(&format!("/agents/{agent_id}/llm-config"))?)?;

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
    let resp: Vec<Value> = unwrap_data(client.get_json("/llm-router/providers")?)?;

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

/// Fetch one config (full JSON) by name or UUID from the caller's library.
fn fetch_config_by_ref(client: &Client, config: &str) -> Result<Value> {
    let configs: Vec<Value> = unwrap_data(client.get_json("/llm-configs")?)?;
    for c in &configs {
        let id = c.get("id").and_then(|v| v.as_str());
        let name = c.get("name").and_then(|v| v.as_str());
        if let Some(id) = id
            && (Some(config) == name || Some(config) == Some(id))
        {
            return Ok(c.clone());
        }
    }
    anyhow::bail!("no LLM config named '{config}' in your library")
}

/// Resolve a config reference (name or UUID) to its id by matching against the caller's library.
fn resolve_config_id(client: &Client, config: &str) -> Result<String> {
    let cfg = fetch_config_by_ref(client, config)?;
    cfg.get("id")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| anyhow::anyhow!("config '{config}' is missing an id"))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        json!({
            "id": "cfg-1",
            "name": "openai-default",
            "provider": "openai",
            "model": "gpt-4o",
            "fallback_models": ["gpt-4o-mini"],
            "temperature": 0.7,
            "max_tokens": 4096,
            "api_key_secret_name": "MY_KEY",
            "pinned": false,
            "pinned_model": null,
        })
    }

    /// Thin wrapper so each test names only the flags it exercises. Order mirrors
    /// `build_update_body`; unnamed params default to "no change".
    #[allow(clippy::too_many_arguments)]
    fn build(
        existing: &Value,
        name: Option<&str>,
        provider: Option<&str>,
        model: Option<&str>,
        fallback: Vec<String>,
        clear_fallbacks: bool,
        pinned: bool,
        pinned_model: Option<&str>,
        clear_pinned_model: bool,
    ) -> Value {
        build_update_body(
            existing,
            name.map(String::from),
            provider.map(String::from),
            model.map(String::from),
            fallback,
            clear_fallbacks,
            None,
            None,
            None,
            None,
            pinned,
            pinned_model.map(String::from),
            clear_pinned_model,
        )
    }

    #[test]
    fn update_with_no_flags_preserves_all_routing_fields() {
        let body = build(
            &sample(),
            None,
            None,
            None,
            vec![],
            false,
            false,
            None,
            false,
        );
        assert_eq!(body["provider"], "openai");
        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["fallback_models"], json!(["gpt-4o-mini"]));
        assert_eq!(body["temperature"], 0.7);
        assert_eq!(body["max_tokens"], 4096);
        assert_eq!(body["api_key_secret_name"], "MY_KEY");
        assert_eq!(body["pinned"], false);
        // Omitted --name serializes to null so the server COALESCEs the current name.
        assert!(body["name"].is_null());
    }

    #[test]
    fn update_overrides_only_provided_fields() {
        let body = build(
            &sample(),
            Some("renamed"),
            None,
            Some("gpt-4o-mini"),
            vec![],
            false,
            false,
            None,
            false,
        );
        assert_eq!(body["name"], "renamed");
        assert_eq!(body["model"], "gpt-4o-mini");
        // Untouched fields keep their existing values.
        assert_eq!(body["provider"], "openai");
        assert_eq!(body["fallback_models"], json!(["gpt-4o-mini"]));
        assert_eq!(body["temperature"], 0.7);
    }

    #[test]
    fn update_pin_sets_pinned_true_keeping_other_fields() {
        let body = build(
            &sample(),
            None,
            None,
            None,
            vec![],
            false,
            true,
            None,
            false,
        );
        assert_eq!(body["pinned"], true);
        assert_eq!(body["model"], "gpt-4o");
    }

    #[test]
    fn update_replaces_and_clears_fallbacks() {
        let replaced = build(
            &sample(),
            None,
            None,
            None,
            vec!["gpt-4o".into(), "gpt-4o-mini".into()],
            false,
            false,
            None,
            false,
        );
        assert_eq!(
            replaced["fallback_models"],
            json!(["gpt-4o", "gpt-4o-mini"])
        );

        let cleared = build(
            &sample(),
            None,
            None,
            None,
            vec![],
            true,
            false,
            None,
            false,
        );
        assert_eq!(cleared["fallback_models"], json!([]));
    }

    #[test]
    fn update_pinned_model_set_kept_and_cleared() {
        // A config already pinned to an explicit model.
        let mut existing = sample();
        existing["pinned"] = json!(true);
        existing["pinned_model"] = json!("gpt-4o");

        // Set a new pinned model.
        let set = build(
            &existing,
            None,
            None,
            None,
            vec![],
            false,
            true,
            Some("gpt-4o-mini"),
            false,
        );
        assert_eq!(set["pinned_model"], "gpt-4o-mini");

        // No flag ⇒ keep the current pinned model.
        let kept = build(
            &existing,
            None,
            None,
            None,
            vec![],
            false,
            true,
            None,
            false,
        );
        assert_eq!(kept["pinned_model"], "gpt-4o");

        // --clear-pinned-model ⇒ null (pin falls back to the config's model server-side).
        let cleared = build(&existing, None, None, None, vec![], false, true, None, true);
        assert!(cleared["pinned_model"].is_null());
        assert_eq!(cleared["pinned"], true);
    }
}
