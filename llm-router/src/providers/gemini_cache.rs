//! Gemini explicit context caching: the agent's `cache_control` markers become a
//! `cachedContents` resource.
//!
//! Gemini caches a chosen prefix only through a separate resource. `POST /cachedContents` stores
//! the system instruction, the tools, the tool config and the leading turns; `generateContent`
//! then names it in `cachedContent` and sends only the remaining turns. Gemini refuses a request
//! that names a cached content and also sets a system instruction, tools or a tool config, so
//! those three always move into the cache with the turns.
//!
//! The router creates a cache only where it can pay: the agent marked a prefix, the prefix clears
//! the model's minimum size (estimated from its length), and no live cache holds it already. A
//! cache lives for the markers' TTL (5 minutes, or an hour when a marker says `"1h"`): storage is
//! billed per hour, so it is never kept longer than the agent asked. It is reused, within this
//! process, by every request with the same endpoint, key, model and prefix.
//!
//! Caching never fails a call. A prefix Gemini will not cache, or a cache it will not take back,
//! is remembered as refused for one TTL, and the request goes out whole, as it did before.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde_json::{Map, Value, json};

use super::ProviderError;
use crate::resolver::ResolvedConfig;

/// A marker's default lifetime, as on Anthropic.
const DEFAULT_TTL: Duration = Duration::from_secs(300);
/// A marker with `"ttl": "1h"`.
const LONG_TTL: Duration = Duration::from_secs(3600);
/// A cache is not reused this close to its expiry, so it cannot lapse between check and call.
const EXPIRY_MARGIN: Duration = Duration::from_secs(30);
/// Gemini's smallest cacheable prefix, in tokens: 1,024 on Flash models, 4,096 on the others.
const MIN_TOKENS_FLASH: usize = 1024;
const MIN_TOKENS_OTHER: usize = 4096;
/// The ratio behind the size estimate. A prefix the estimate lets through but Gemini judges too
/// small is refused once and then remembered.
const CHARS_PER_TOKEN: usize = 4;
/// The longest a cache creation may hold up the call it serves; past it the call goes uncached.
const CREATE_TIMEOUT: Duration = Duration::from_secs(15);
/// Past this many remembered prefixes, the expired ones are dropped on the next insert.
const PRUNE_ABOVE: usize = 1024;
/// The top-level request fields that move into the cache with the prefix turns.
const CACHED_FIELDS: [&str; 3] = ["systemInstruction", "tools", "toolConfig"];

/// Where the agent's markers end in a translated request, and how long to keep that prefix.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct CachePlan {
    /// The leading `contents` the cache holds. At least the last turn is always sent live,
    /// since a request needs one.
    pub prefix_len: usize,
    pub ttl: Duration,
}

impl CachePlan {
    /// A plan caching `prefix_len` turns for the longest TTL any of `markers` asks for.
    pub fn new<'a>(prefix_len: usize, markers: impl IntoIterator<Item = &'a Value>) -> Self {
        let long = markers.into_iter().any(|marker| marker["ttl"] == "1h");
        Self {
            prefix_len,
            ttl: if long { LONG_TTL } else { DEFAULT_TTL },
        }
    }
}

/// A request body that names a cached prefix, and how to stop using that cache.
pub(super) struct CachedRequest {
    pub body: Value,
    key: u64,
    ttl: Duration,
}

/// The request to send in place of `body`, naming a cache of its marked prefix — created now
/// when none is live — or `None` to send `body` as it is.
pub(super) async fn use_cached_prefix(
    http: &reqwest::Client,
    base: &str,
    cfg: &ResolvedConfig,
    body: &Value,
    plan: &CachePlan,
) -> Option<CachedRequest> {
    let (prefix, rest) = split(body, plan.prefix_len)?;
    let prefix_text = prefix.to_string();
    if prefix_text.len() / CHARS_PER_TOKEN < min_tokens(&cfg.model) {
        return None;
    }
    let key = prefix_key(base, cfg, &prefix_text);
    let name = match lookup(key) {
        Lookup::Live(name) => name,
        Lookup::Refused => return None,
        Lookup::Missing => match create(http, base, cfg, prefix, plan.ttl).await {
            Ok(name) => {
                let until = Instant::now() + plan.ttl.saturating_sub(EXPIRY_MARGIN);
                remember(key, Entry::Live(name.clone(), until));
                name
            }
            Err(error) => {
                tracing::warn!(
                    target: "nasiko::llm_router::gemini",
                    model = %cfg.model, %error,
                    "gemini: explicit cache not created; sending the request uncached"
                );
                remember(key, Entry::Refused(Instant::now() + plan.ttl));
                return None;
            }
        },
    };
    let mut body = rest;
    body["cachedContent"] = json!(name);
    Some(CachedRequest {
        body,
        key,
        ttl: plan.ttl,
    })
}

/// Stop using the cache `request` named, for one TTL: Gemini would not take it back.
pub(super) fn refuse(request: &CachedRequest) {
    remember(request.key, Entry::Refused(Instant::now() + request.ttl));
}

/// What the process knows about one prefix.
enum Entry {
    /// A cache by this resource name, usable until the instant.
    Live(String, Instant),
    /// Not cacheable until the instant.
    Refused(Instant),
}

enum Lookup {
    Live(String),
    Refused,
    Missing,
}

static CACHES: LazyLock<DashMap<u64, Entry>> = LazyLock::new(DashMap::new);

fn lookup(key: u64) -> Lookup {
    let now = Instant::now();
    match CACHES.get(&key).as_deref() {
        Some(Entry::Live(name, until)) if *until > now => Lookup::Live(name.clone()),
        Some(Entry::Refused(until)) if *until > now => Lookup::Refused,
        _ => Lookup::Missing,
    }
}

fn remember(key: u64, entry: Entry) {
    if CACHES.len() > PRUNE_ABOVE {
        let now = Instant::now();
        CACHES.retain(|_, entry| match entry {
            Entry::Live(_, until) | Entry::Refused(until) => *until > now,
        });
    }
    CACHES.insert(key, entry);
}

/// Split a translated `generateContent` body into the cache body (the moved fields and the
/// first `prefix_len` turns, capped to keep the last turn live) and the request that remains.
/// `None` when the body has no turns.
fn split(body: &Value, prefix_len: usize) -> Option<(Value, Value)> {
    let contents = body["contents"].as_array()?;
    let prefix_len = prefix_len.min(contents.len().checked_sub(1)?);
    let mut rest = body.as_object()?.clone();
    let mut prefix = Map::new();
    for field in CACHED_FIELDS {
        if let Some(value) = rest.remove(field) {
            prefix.insert(field.to_string(), value);
        }
    }
    if prefix_len > 0 {
        prefix.insert("contents".into(), json!(contents[..prefix_len]));
    }
    rest.insert("contents".into(), json!(contents[prefix_len..]));
    Some((Value::Object(prefix), Value::Object(rest)))
}

fn min_tokens(model: &str) -> usize {
    if model.contains("flash") {
        MIN_TOKENS_FLASH
    } else {
        MIN_TOKENS_OTHER
    }
}

/// A cache belongs to one endpoint, key and model, so all three are part of what it is found by.
fn prefix_key(base: &str, cfg: &ResolvedConfig, prefix_text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    base.hash(&mut hasher);
    cfg.api_key.hash(&mut hasher);
    cfg.model.hash(&mut hasher);
    prefix_text.hash(&mut hasher);
    hasher.finish()
}

/// `POST /cachedContents`; the new cache's resource name.
async fn create(
    http: &reqwest::Client,
    base: &str,
    cfg: &ResolvedConfig,
    mut prefix: Value,
    ttl: Duration,
) -> Result<String, ProviderError> {
    prefix["model"] = json!(format!("models/{}", cfg.model));
    prefix["ttl"] = json!(format!("{}s", ttl.as_secs()));
    let resp = http
        .post(format!("{base}/cachedContents"))
        .header("x-goog-api-key", &cfg.api_key)
        .timeout(CREATE_TIMEOUT)
        .json(&prefix)
        .send()
        .await
        .map_err(|e| ProviderError::Transport(e.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(ProviderError::Status {
            status: status.as_u16(),
            message: resp.text().await.unwrap_or_default(),
            retryable: false,
        });
    }
    let created: Value = resp
        .json()
        .await
        .map_err(|e| ProviderError::Parse(e.to_string()))?;
    created["name"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| ProviderError::Parse("cachedContents response has no name".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> Value {
        json!({
            "systemInstruction": { "parts": [{ "text": "sys" }] },
            "tools": [{ "functionDeclarations": [{ "name": "t" }] }],
            "toolConfig": { "functionCallingConfig": { "mode": "AUTO" } },
            "generationConfig": { "temperature": 0.1 },
            "contents": [
                { "role": "user", "parts": [{ "text": "a" }] },
                { "role": "model", "parts": [{ "text": "b" }] },
                { "role": "user", "parts": [{ "text": "c" }] }
            ]
        })
    }

    #[test]
    fn split_moves_system_tools_and_the_prefix_turns_into_the_cache() {
        let (prefix, rest) = split(&body(), 2).unwrap();
        assert_eq!(prefix["systemInstruction"]["parts"][0]["text"], "sys");
        assert_eq!(prefix["tools"][0]["functionDeclarations"][0]["name"], "t");
        assert_eq!(
            prefix["toolConfig"]["functionCallingConfig"]["mode"],
            "AUTO"
        );
        assert_eq!(prefix["contents"].as_array().unwrap().len(), 2);
        for field in CACHED_FIELDS {
            assert!(rest.get(field).is_none(), "{field} stayed in the request");
        }
        assert_eq!(rest["generationConfig"]["temperature"], 0.1);
        assert_eq!(
            rest["contents"],
            json!([{ "role": "user", "parts": [{ "text": "c" }] }])
        );
    }

    #[test]
    fn split_always_keeps_the_last_turn_live() {
        let (prefix, rest) = split(&body(), 3).unwrap();
        assert_eq!(prefix["contents"].as_array().unwrap().len(), 2);
        assert_eq!(rest["contents"].as_array().unwrap().len(), 1);
        // No turns before the marker: only the moved fields are cached.
        let (prefix, rest) = split(&body(), 0).unwrap();
        assert!(prefix.get("contents").is_none());
        assert_eq!(rest["contents"].as_array().unwrap().len(), 3);
        assert!(split(&json!({ "contents": [] }), 0).is_none());
    }

    #[test]
    fn plan_keeps_the_longest_ttl_any_marker_asks_for() {
        let short = json!({ "type": "ephemeral" });
        let long = json!({ "type": "ephemeral", "ttl": "1h" });
        assert_eq!(CachePlan::new(1, [&short]).ttl, DEFAULT_TTL);
        assert_eq!(CachePlan::new(1, [&short, &long]).ttl, LONG_TTL);
    }

    #[test]
    fn flash_models_cache_smaller_prefixes() {
        assert_eq!(min_tokens("gemini-2.5-flash"), MIN_TOKENS_FLASH);
        assert_eq!(min_tokens("gemini-2.5-pro"), MIN_TOKENS_OTHER);
    }
}
