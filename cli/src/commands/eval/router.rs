//! Talking to the Nasiko server for evals: the `eval` coding-agent identity, pinning its model,
//! its router token, and the router's own usage and prices (`/api/evals/*`).
//!
//! Every eval model call goes through the Nasiko LLM router under the `eval` identity, tagged
//! with `x-nasiko-eval-attempt`, so every dollar in a report comes from the router.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use nasiko_evals::{PriceQuality, RouterUsage, TokenUsage};
use serde::Deserialize;
use serde_json::{Value, json};

use super::store::ModelEntry;
use crate::api::Client;
use crate::commands::coding_agent_router::{RoutingCredential, require_current_login};

/// The coding-agent integration the eval identity is registered as.
pub const EVAL_INTEGRATION: &str = "eval";
/// The header the router reads the attempt tag from (`oss/llm-router/src/eval_tag.rs`).
pub const ATTEMPT_HEADER: &str = "x-nasiko-eval-attempt";

pub struct RouterClient {
    api: Client,
    cluster_url: String,
}

#[derive(Deserialize)]
struct Envelope<T> {
    data: T,
}

#[derive(Deserialize)]
struct UsagePage {
    data: Vec<AttemptUsage>,
    #[serde(default)]
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
struct AttemptUsage {
    attempt: String,
    calls: i64,
    models: Vec<String>,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
    cost_usd: f64,
    worst_price_source: String,
    estimated_calls: i64,
}

#[derive(Deserialize)]
struct PricedTokens {
    cost_usd: f64,
    source: String,
    estimated: bool,
}

/// A price the router gave for some tokens.
pub struct Priced {
    pub cost_usd: f64,
    pub quality: PriceQuality,
    pub estimated: bool,
}

impl RouterClient {
    /// The cluster you are logged in to (`nasiko auth login`).
    pub fn from_login() -> Result<Self> {
        let (_, entry, _) = require_current_login()?;
        Ok(Self {
            cluster_url: entry.url.trim_end_matches('/').to_string(),
            api: Client::from_cluster_entry(&entry),
        })
    }

    /// `<cluster>/v1`, the router's OpenAI-compatible base URL.
    pub fn router_base(&self) -> String {
        format!("{}/v1", self.cluster_url)
    }

    /// Registers (or finds) your `eval` coding-agent identity and returns its id.
    pub fn ensure_eval_agent(&self) -> Result<String> {
        let reply: Value = self.api.post_json(
            "/agents/coding-integrations",
            &json!({ "integration_id": EVAL_INTEGRATION }),
        )?;
        let agent = reply.get("data").unwrap_or(&reply);
        agent
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .context("the server did not return the eval identity's id; is this cluster on a version with replay evals?")
    }

    /// Pins the eval identity to one model: an LLM config `eval-<short>` with the model pinned
    /// (created once, reused after), attached to the identity.
    pub fn pin_model(&self, agent_id: &str, short: &str, model: &ModelEntry) -> Result<()> {
        let name = format!("eval-{short}");
        let configs: Envelope<Vec<Value>> = self.api.get_json("/llm-configs")?;
        let existing = configs.data.iter().find(|c| {
            c.get("name").and_then(Value::as_str) == Some(name.as_str())
                && c.get("provider").and_then(Value::as_str) == Some(model.provider.as_str())
                && c.get("pinned_model").and_then(Value::as_str) == Some(model.model.as_str())
        });
        let id = match existing.and_then(|c| c.get("id")).and_then(Value::as_str) {
            Some(id) => id.to_string(),
            None => {
                let created: Envelope<Value> = self.api.post_json(
                    "/llm-configs",
                    &json!({
                        "name": name,
                        "provider": model.provider,
                        "model": model.model,
                        "pinned": true,
                        "pinned_model": model.model,
                    }),
                )?;
                created.data["id"]
                    .as_str()
                    .context("the new LLM config has no id")?
                    .to_string()
            }
        };
        let _: Value = self.api.patch_json(
            &format!("/agents/{agent_id}/llm-config"),
            &json!({ "llm_config_id": id }),
        )?;
        Ok(())
    }

    /// A fresh router token for the eval identity (valid about 10 minutes).
    pub fn mint_token(&self, agent_id: &str) -> Result<RoutingCredential> {
        let reply: Envelope<RoutingCredential> = self
            .api
            .post_json_quiet(&format!("/agents/{agent_id}/llm-token"), &json!({}))?;
        Ok(reply.data)
    }

    /// The router's totals for every attempt tag that starts with `<run>:`.
    pub fn run_usage(&self, run: &str) -> Result<BTreeMap<String, RouterUsage>> {
        let mut out = BTreeMap::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut path = format!("/evals/usage?run={}&limit=1000", encode(run));
            if let Some(c) = &cursor {
                path.push_str(&format!("&cursor={}", encode(c)));
            }
            let page: UsagePage = self.api.get_json(&path)?;
            for a in page.data {
                out.insert(a.attempt.clone(), router_usage(&a)?);
            }
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => return Ok(out),
            }
        }
    }

    /// Prices token counts with the router's own price book.
    pub fn price(&self, provider: Option<&str>, model: &str, usage: &TokenUsage) -> Result<Priced> {
        let reply: Envelope<PricedTokens> = self.api.post_json(
            "/evals/price",
            &json!({
                "provider": provider,
                "model": model,
                "input_tokens": usage.input,
                "output_tokens": usage.output,
                "cache_read_tokens": usage.cache_read,
                "cache_creation_tokens": usage.cache_write,
            }),
        )?;
        Ok(Priced {
            cost_usd: reply.data.cost_usd,
            quality: price_quality(&reply.data.source)?,
            estimated: reply.data.estimated,
        })
    }
}

fn router_usage(a: &AttemptUsage) -> Result<RouterUsage> {
    let n = |v: i64| u64::try_from(v).unwrap_or(0);
    Ok(RouterUsage {
        calls: u32::try_from(a.calls).unwrap_or(u32::MAX),
        usage: TokenUsage {
            input: n(a.input_tokens),
            output: n(a.output_tokens),
            cache_read: n(a.cache_read_tokens),
            cache_write: n(a.cache_creation_tokens),
            reasoning: 0,
        },
        cost_usd: a.cost_usd,
        models: a.models.clone(),
        worst_price: price_quality(&a.worst_price_source)?,
        estimated_calls: u32::try_from(a.estimated_calls).unwrap_or(u32::MAX),
    })
}

fn price_quality(source: &str) -> Result<PriceQuality> {
    Ok(match source {
        "Exact" => PriceQuality::Exact,
        "Family" => PriceQuality::Family,
        "Static" => PriceQuality::Static,
        "Ratio" => PriceQuality::Ratio,
        "Default" => PriceQuality::Default,
        other => bail!("unknown price source {other:?} from the router"),
    })
}

/// Percent-encodes a query value (tags are `[A-Za-z0-9:._/-]`, so only `:` and `/` matter).
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_rows_map_to_router_usage() {
        let a = AttemptUsage {
            attempt: "run1:a".into(),
            calls: 3,
            models: vec!["flint".into()],
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 7,
            cache_creation_tokens: 2,
            cost_usd: 0.0,
            worst_price_source: "Exact".into(),
            estimated_calls: 0,
        };
        let u = router_usage(&a).expect("maps");
        assert_eq!(
            (u.calls, u.usage.cache_read, u.usage.cache_write),
            (3, 7, 2)
        );
        assert_eq!(u.worst_price, PriceQuality::Exact);
        assert!(price_quality("Nope").is_err());
    }

    #[test]
    fn query_values_are_encoded() {
        assert_eq!(encode("run-1:a/b"), "run-1%3Aa%2Fb");
    }

    #[test]
    fn usage_pages_are_followed() {
        let mut server = mockito::Server::new();
        let first = server
            .mock("GET", "/api/evals/usage?run=r1&limit=1000")
            .with_body(r#"{"data":[{"attempt":"r1:a","calls":1,"failed_calls":0,"models":["m"],"input_tokens":1,"output_tokens":1,"cache_read_tokens":0,"cache_creation_tokens":0,"cost_usd":0.5,"worst_price_source":"Exact","estimated_calls":0}],"has_more":true,"next_cursor":"r1:a"}"#)
            .create();
        let second = server
            .mock("GET", "/api/evals/usage?run=r1&limit=1000&cursor=r1%3Aa")
            .with_body(r#"{"data":[{"attempt":"r1:b","calls":2,"failed_calls":0,"models":["m"],"input_tokens":1,"output_tokens":1,"cache_read_tokens":0,"cache_creation_tokens":0,"cost_usd":0.25,"worst_price_source":"Default","estimated_calls":2}],"has_more":false,"next_cursor":null}"#)
            .create();
        let client = RouterClient {
            api: Client::for_test(&server.url(), Some("t")),
            cluster_url: server.url(),
        };
        let usage = client.run_usage("r1").expect("pages");
        first.assert();
        second.assert();
        assert_eq!(usage.len(), 2);
        assert_eq!(usage["r1:b"].worst_price, PriceQuality::Default);
    }
}
