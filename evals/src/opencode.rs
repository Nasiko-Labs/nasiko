//! The replayed agent, opencode: its config, and reading its session export.
//!
//! opencode talks OpenAI-style chat completions to the eval gateway, which forwards to the
//! Nasiko router; the router serves whatever model is pinned on the eval identity, so the model
//! opencode names is just `router` (the same pattern `nasiko connect opencode` uses).

use serde::Deserialize;
use serde_json::{Value, json};

use crate::cost::TokenUsage;

/// Inputs of the agent's `opencode.json`.
pub struct AgentConfig {
    pub context_tokens: u32,
    pub output_tokens: u32,
    /// Agent steps before opencode tells the model to stop using tools.
    pub max_steps: u32,
}

/// The provider id and model id opencode is pointed at: `--model eval/router`.
pub const MODEL_ARG: &str = "eval/router";

/// The `opencode.json` for one attempt. Secrets are not in it: the base URL and the per-attempt
/// gateway secret come from env (`{env:…}` is resolved by opencode).
pub fn config_json(cfg: &AgentConfig) -> Value {
    json!({
        "$schema": "https://opencode.ai/config.json",
        "provider": {
            "eval": {
                "npm": "@ai-sdk/openai-compatible",
                "name": "Nasiko router (via eval gateway)",
                "options": {
                    "baseURL": "{env:OPENAI_BASE_URL}",
                    "apiKey": "unused-gateway-adds-key",
                    "headers": { "x-eval-secret": "{env:EVAL_GATEWAY_SECRET}" },
                    "timeout": 600000,
                    "chunkTimeout": 120000
                },
                "models": {
                    "router": {
                        "name": "Nasiko Router",
                        "tool_call": true,
                        "limit": { "context": cfg.context_tokens, "output": cfg.output_tokens }
                    }
                }
            }
        },
        "model": MODEL_ARG,
        "small_model": MODEL_ARG,
        "agent": { "build": { "steps": cfg.max_steps } },
        "permission": {
            "edit": "allow", "bash": "allow", "doom_loop": "allow",
            "webfetch": "deny", "websearch": "deny", "external_directory": "deny", "task": "deny"
        },
        "autoupdate": false,
        "share": "disabled",
        "snapshot": false
    })
}

/// Where [`token_config_json`] reads the router's base URL and the attempt's token from.
pub const ROUTER_URL_ENV: &str = "NASIKO_ROUTER_URL";
pub const ROUTER_TOKEN_ENV: &str = "NASIKO_ROUTER_TOKEN";

/// [`config_json`] for an attempt the server runs: opencode calls the router directly (through a
/// network that reaches nothing else) with the attempt's own token, which names the attempt and
/// the model, so no gateway adds a key or a tag.
pub fn token_config_json(cfg: &AgentConfig) -> Value {
    let mut value = config_json(cfg);
    let provider = &mut value["provider"]["eval"];
    provider["name"] = json!("Nasiko router");
    provider["options"] = json!({
        "baseURL": format!("{{env:{ROUTER_URL_ENV}}}"),
        "apiKey": format!("{{env:{ROUTER_TOKEN_ENV}}}"),
        "timeout": 600000,
        "chunkTimeout": 120000
    });
    value
}

/// What opencode's own record says about one run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentRecord {
    pub steps: u32,
    pub tool_calls: u32,
    pub usage: TokenUsage,
    pub errors: Vec<String>,
    /// Whether the last step ended because the step limit was reached.
    pub hit_step_limit: bool,
}

#[derive(Deserialize)]
struct Export {
    #[serde(default)]
    messages: Vec<ExportMessage>,
}

#[derive(Deserialize)]
struct ExportMessage {
    #[serde(default)]
    info: Value,
    #[serde(default)]
    parts: Vec<Value>,
}

/// Reads `opencode export <session>` output. Token counts come from the `step-finish` parts
/// (input already excludes cache reads and writes, output excludes reasoning).
pub fn parse_export(text: &str) -> Result<AgentRecord, serde_json::Error> {
    let export: Export = serde_json::from_str(text)?;
    let mut record = AgentRecord::default();
    for message in &export.messages {
        if let Some(err) = message.info.get("error") {
            record.errors.push(err.to_string());
        }
        for part in &message.parts {
            match part.get("type").and_then(Value::as_str) {
                Some("step-finish") => {
                    record.steps += 1;
                    record.usage.add(&step_tokens(part));
                    record.hit_step_limit =
                        part.get("reason").and_then(Value::as_str) == Some("max-steps");
                }
                Some("tool") => record.tool_calls += 1,
                _ => {}
            }
        }
    }
    Ok(record)
}

fn step_tokens(part: &Value) -> TokenUsage {
    let t = &part["tokens"];
    let n = |v: &Value| v.as_f64().unwrap_or(0.0) as u64;
    TokenUsage {
        input: n(&t["input"]),
        output: n(&t["output"]),
        reasoning: n(&t["reasoning"]),
        cache_read: n(&t["cache"]["read"]),
        cache_write: n(&t["cache"]["write"]),
    }
}

/// The session id from opencode's `--format json` event stream (any event carries it).
pub fn session_id_from_events(events: &str) -> Option<String> {
    events.lines().find_map(|l| {
        serde_json::from_str::<Value>(l)
            .ok()?
            .get("sessionID")?
            .as_str()
            .map(str::to_string)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_config_reads_url_and_key_from_env_and_sends_no_secret_header() {
        let c = token_config_json(&AgentConfig {
            context_tokens: 1000,
            output_tokens: 100,
            max_steps: 5,
        });
        let options = &c["provider"]["eval"]["options"];
        assert_eq!(options["baseURL"], "{env:NASIKO_ROUTER_URL}");
        assert_eq!(options["apiKey"], "{env:NASIKO_ROUTER_TOKEN}");
        assert!(options.get("headers").is_none());
        assert_eq!(c["agent"]["build"]["steps"], 5);
    }

    #[test]
    fn config_points_at_the_router_model_with_limits() {
        let c = config_json(&AgentConfig {
            context_tokens: 32768,
            output_tokens: 4096,
            max_steps: 100,
        });
        assert_eq!(c["model"], "eval/router");
        assert_eq!(
            c["provider"]["eval"]["models"]["router"]["limit"]["output"],
            4096
        );
        assert_eq!(c["agent"]["build"]["steps"], 100);
        assert_eq!(c["permission"]["webfetch"], "deny");
    }

    #[test]
    fn export_steps_and_tokens_are_summed() {
        let export = r#"{"info":{},"messages":[
          {"info":{"role":"user"},"parts":[{"type":"text","text":"hi"}]},
          {"info":{"role":"assistant"},"parts":[
            {"type":"tool","tool":"bash"},
            {"type":"step-finish","reason":"tool-calls","tokens":{"input":100,"output":10,"reasoning":5,"cache":{"read":50,"write":0}}},
            {"type":"step-finish","reason":"stop","tokens":{"input":20,"output":4,"reasoning":0,"cache":{"read":150,"write":0}}}
          ]}]}"#;
        let r = parse_export(export).expect("parses");
        assert_eq!((r.steps, r.tool_calls), (2, 1));
        assert_eq!(r.usage.input, 120);
        assert_eq!(r.usage.cache_read, 200);
        assert!(!r.hit_step_limit);
    }

    #[test]
    fn session_id_is_read_from_events() {
        let events = "not json\n{\"type\":\"step_start\",\"sessionID\":\"ses_1\"}\n";
        assert_eq!(session_id_from_events(events).as_deref(), Some("ses_1"));
    }
}
