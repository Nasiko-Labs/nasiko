//! Decision cache — remembers the model chosen for a `(conv_id, agent_id)` so the next
//! turns of the same agent's loop read the decision instead of re-classifying.
//!
//! Read at Level 2 of the precedence chain; written after Level 3 (classification)
//! succeeds. **It is a latency optimisation, not a correctness dependency:** if the
//! backing store is unavailable the read returns `None` and the router simply
//! re-classifies (Level 3) or falls through — never an error. That distinction is
//! enforced in the impls, which must swallow backend failures.
//!
//! [`NoopCache`] is the fail-open default (every read misses); [`RedisCache`] is the
//! process-wide, horizontally-shared store keyed on `(conv_id, agent_id)`.

use async_trait::async_trait;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

use super::classifier::{RequestType, Tier};

/// A cached routing decision.
#[derive(Debug, Clone)]
pub struct CachedDecision {
    /// The provider-native model id that was selected.
    pub model: String,
    /// The tier it came from (informational; `None` for decisions not produced by
    /// classification).
    pub tier: Option<Tier>,
    /// The request type the query was classified as — carried so a later turn's feedback can
    /// be credited to the right learned cell. `None` for decisions not produced by
    /// classification.
    pub request_type: Option<RequestType>,
    /// The classifier's complexity (1–5) for the classified turn. Telemetry and future use
    /// only — learning stays keyed on `(tier, request_type)`.
    pub complexity: Option<u8>,
    /// The classifier's confidence for the classified turn (telemetry only).
    pub confidence: Option<f32>,
}

/// Process-wide store for routing decisions, keyed on `(conv_id, agent_id)`.
///
/// Implementations MUST NOT surface backend errors: a failed `get` returns `None`
/// (treated as a miss) and a failed `put` is dropped. Correctness never depends on the
/// cache being up.
#[async_trait]
pub trait DecisionCache: Send + Sync {
    /// The cached decision for this conversation+agent, or `None` on a miss (including
    /// when the backend is unavailable).
    async fn get(&self, conv_id: &str, agent_id: &str) -> Option<CachedDecision>;

    /// Store a decision. Best-effort — failures are swallowed.
    async fn put(&self, conv_id: &str, agent_id: &str, decision: &CachedDecision);
}

/// A cache that stores nothing — every read misses. The fail-open stand-in whenever no
/// Redis is configured (`REDIS_URL` unset).
pub struct NoopCache;

#[async_trait]
impl DecisionCache for NoopCache {
    async fn get(&self, _conv_id: &str, _agent_id: &str) -> Option<CachedDecision> {
        None
    }
    async fn put(&self, _conv_id: &str, _agent_id: &str, _decision: &CachedDecision) {}
}

/// On-the-wire form of a [`CachedDecision`] — `tier` stored as its SMALLINT level so the
/// value survives independently of the `Tier` enum's Rust representation.
#[derive(Serialize, Deserialize)]
struct WireDecision {
    model: String,
    tier: Option<i16>,
    /// Persisted as the request-type string; absent on older cached entries.
    #[serde(default)]
    request_type: Option<String>,
    /// Classifier complexity; absent on entries cached before the classifier was pluggable.
    #[serde(default)]
    complexity: Option<u8>,
    /// Classifier confidence; absent on older entries.
    #[serde(default)]
    confidence: Option<f32>,
}

/// Redis-backed decision cache, keyed on `(conv_id, agent_id)`, with a TTL per entry.
///
/// Every operation opens a multiplexed async connection and **swallows all failures**
/// (connection, command, or malformed value) — a `get` degrades to a miss and a `put` is
/// dropped, per the "latency, not correctness" contract. A Redis outage therefore makes
/// the router re-classify or fall through, never error.
pub struct RedisCache {
    client: redis::Client,
    ttl_secs: u64,
}

impl RedisCache {
    pub fn new(client: redis::Client, ttl_secs: u64) -> Self {
        Self { client, ttl_secs }
    }

    fn key(conv_id: &str, agent_id: &str) -> String {
        format!("nasiko:router:decision:{conv_id}:{agent_id}")
    }
}

#[async_trait]
impl DecisionCache for RedisCache {
    async fn get(&self, conv_id: &str, agent_id: &str) -> Option<CachedDecision> {
        let key = Self::key(conv_id, agent_id);
        let mut conn = match self.client.get_multiplexed_async_connection().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    target: "nasiko::llm_router::decision_cache",
                    error = %e, %key,
                    "redis decision cache GET — connection failed; degrading to a cache miss (fail-open)"
                );
                return None;
            }
        };
        let raw: Option<String> = conn.get(&key).await.ok()?;
        match raw {
            None => {
                tracing::debug!(
                    target: "nasiko::llm_router::decision_cache",
                    %key, "redis decision cache GET — miss (no entry)"
                );
                None
            }
            Some(raw) => match serde_json::from_str::<WireDecision>(&raw) {
                Ok(wire) => {
                    tracing::info!(
                        target: "nasiko::llm_router::decision_cache",
                        %key, model = %wire.model, tier = ?wire.tier, request_type = ?wire.request_type,
                        "redis decision cache GET — hit"
                    );
                    Some(CachedDecision {
                        model: wire.model,
                        tier: wire.tier.and_then(Tier::from_level),
                        request_type: wire
                            .request_type
                            .as_deref()
                            .and_then(RequestType::from_wire),
                        complexity: wire.complexity,
                        confidence: wire.confidence,
                    })
                }
                Err(e) => {
                    tracing::warn!(
                        target: "nasiko::llm_router::decision_cache",
                        error = %e, %key,
                        "redis decision cache GET — malformed cached value; treating as miss"
                    );
                    None
                }
            },
        }
    }

    async fn put(&self, conv_id: &str, agent_id: &str, decision: &CachedDecision) {
        let key = Self::key(conv_id, agent_id);
        let Ok(mut conn) = self.client.get_multiplexed_async_connection().await else {
            tracing::warn!(
                target: "nasiko::llm_router::decision_cache",
                %key, "redis decision cache PUT — connection failed; decision not cached (fail-open)"
            );
            return;
        };
        let wire = WireDecision {
            model: decision.model.clone(),
            tier: decision.tier.map(Tier::as_level),
            request_type: decision.request_type.map(|rt| rt.as_str().to_string()),
            complexity: decision.complexity,
            confidence: decision.confidence,
        };
        let Ok(payload) = serde_json::to_string(&wire) else {
            return;
        };
        // SET key payload EX ttl — best-effort; a failure just means the next turn re-derives.
        let res: Result<(), _> = conn.set_ex(&key, payload, self.ttl_secs).await;
        match res {
            Ok(()) => tracing::info!(
                target: "nasiko::llm_router::decision_cache",
                %key, model = %decision.model, tier = ?decision.tier, ttl_secs = self.ttl_secs,
                "redis decision cache PUT — stored sticky decision"
            ),
            Err(e) => tracing::warn!(
                target: "nasiko::llm_router::decision_cache",
                error = %e, %key,
                "redis decision cache PUT — write failed; decision not cached (fail-open)"
            ),
        }
    }
}

/// In-process sticky decision cache (`ROUTER_DECISION_L1_CAPACITY > 0`): gives a single
/// router instance conversation stickiness without Redis, and spares a Redis round trip when
/// both are configured. Entries expire after the decision TTL; when full, expired entries are
/// evicted first and, if that is not enough, the whole map is cleared (bounded memory with
/// the simplest correct policy — a miss only means the next turn re-derives its model).
pub struct InMemoryDecisionCache {
    map: dashmap::DashMap<(String, String), (CachedDecision, std::time::Instant)>,
    ttl: std::time::Duration,
    capacity: usize,
}

impl InMemoryDecisionCache {
    pub fn new(capacity: usize, ttl: std::time::Duration) -> Self {
        Self {
            map: dashmap::DashMap::new(),
            ttl,
            capacity: capacity.max(1),
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[async_trait]
impl DecisionCache for InMemoryDecisionCache {
    async fn get(&self, conv_id: &str, agent_id: &str) -> Option<CachedDecision> {
        let key = (conv_id.to_string(), agent_id.to_string());
        let fresh = self
            .map
            .get(&key)
            .map(|e| (e.0.clone(), e.1.elapsed() < self.ttl));
        match fresh {
            Some((decision, true)) => Some(decision),
            Some((_, false)) => {
                self.map.remove(&key);
                None
            }
            None => None,
        }
    }

    async fn put(&self, conv_id: &str, agent_id: &str, decision: &CachedDecision) {
        if self.map.len() >= self.capacity {
            let ttl = self.ttl;
            self.map.retain(|_, (_, at)| at.elapsed() < ttl);
            if self.map.len() >= self.capacity {
                self.map.clear();
            }
        }
        self.map.insert(
            (conv_id.to_string(), agent_id.to_string()),
            (decision.clone(), std::time::Instant::now()),
        );
    }
}

/// An L1 cache in front of an L2 cache: reads try L1 then L2 (an L2 hit back-fills L1);
/// writes go to both.
pub struct TieredDecisionCache {
    l1: std::sync::Arc<dyn DecisionCache>,
    l2: std::sync::Arc<dyn DecisionCache>,
}

impl TieredDecisionCache {
    pub fn new(l1: std::sync::Arc<dyn DecisionCache>, l2: std::sync::Arc<dyn DecisionCache>) -> Self {
        Self { l1, l2 }
    }
}

#[async_trait]
impl DecisionCache for TieredDecisionCache {
    async fn get(&self, conv_id: &str, agent_id: &str) -> Option<CachedDecision> {
        if let Some(hit) = self.l1.get(conv_id, agent_id).await {
            return Some(hit);
        }
        let hit = self.l2.get(conv_id, agent_id).await?;
        self.l1.put(conv_id, agent_id, &hit).await;
        Some(hit)
    }

    async fn put(&self, conv_id: &str, agent_id: &str, decision: &CachedDecision) {
        self.l1.put(conv_id, agent_id, decision).await;
        self.l2.put(conv_id, agent_id, decision).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decision(model: &str) -> CachedDecision {
        CachedDecision {
            model: model.into(),
            tier: Some(Tier::Tier2),
            request_type: Some(RequestType::CodeGeneration),
            complexity: Some(3),
            confidence: Some(0.9),
        }
    }

    #[test]
    fn wire_decision_without_complexity_still_deserializes_and_round_trips() {
        let old: WireDecision =
            serde_json::from_str(r#"{"model":"m","tier":1,"request_type":"writing"}"#).unwrap();
        assert_eq!((old.complexity, old.confidence), (None, None));
        let wire = WireDecision {
            model: "m".into(),
            tier: Some(2),
            request_type: Some("writing".into()),
            complexity: Some(4),
            confidence: Some(0.75),
        };
        let back: WireDecision = serde_json::from_str(&serde_json::to_string(&wire).unwrap()).unwrap();
        assert_eq!((back.complexity, back.confidence), (Some(4), Some(0.75)));
    }

    #[tokio::test]
    async fn l1_cache_serves_sticky_decision_without_redis() {
        let l1 = std::sync::Arc::new(InMemoryDecisionCache::new(2, std::time::Duration::from_secs(60)));
        let tiered = TieredDecisionCache::new(l1.clone(), std::sync::Arc::new(NoopCache));
        tiered.put("c1", "a", &decision("m1")).await;
        assert_eq!(tiered.get("c1", "a").await.map(|d| d.model), Some("m1".into()));
        assert!(tiered.get("c2", "a").await.is_none());
        // Bounded: a third distinct key never grows the map past capacity.
        tiered.put("c2", "a", &decision("m2")).await;
        tiered.put("c3", "a", &decision("m3")).await;
        assert!(l1.len() <= 2);
        // Expiry: a zero TTL makes every entry stale.
        let expiring = InMemoryDecisionCache::new(8, std::time::Duration::ZERO);
        expiring.put("c", "a", &decision("m")).await;
        assert!(expiring.get("c", "a").await.is_none());
    }

    #[tokio::test]
    async fn noop_always_misses() {
        let c = NoopCache;
        c.put(
            "conv",
            "agent",
            &CachedDecision {
                model: "m".into(),
                tier: Some(Tier::Tier1),
                request_type: Some(RequestType::General),
                complexity: None,
                confidence: None,
            },
        )
        .await;
        assert!(c.get("conv", "agent").await.is_none());
    }

    #[test]
    fn key_is_namespaced_by_conv_and_agent() {
        assert_eq!(
            RedisCache::key("conv-1", "agent-9"),
            "nasiko:router:decision:conv-1:agent-9"
        );
    }

    #[test]
    fn wire_decision_round_trips_tier_as_level() {
        let wire = WireDecision {
            model: "m".into(),
            tier: Some(3),
            request_type: Some("factual_lookup".into()),
            complexity: None,
            confidence: None,
        };
        let json = serde_json::to_string(&wire).unwrap();
        let back: WireDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(back.model, "m");
        assert_eq!(back.tier.and_then(Tier::from_level), Some(Tier::Tier3));
        assert_eq!(
            back.request_type
                .as_deref()
                .and_then(RequestType::from_wire),
            Some(RequestType::FactualLookup)
        );
    }

    #[test]
    fn wire_decision_without_request_type_deserializes() {
        // An entry cached before request_type existed must still load (as a miss of the
        // learning target, not a parse failure).
        let back: WireDecision = serde_json::from_str(r#"{"model":"m","tier":2}"#).unwrap();
        assert_eq!(back.model, "m");
        assert_eq!(back.request_type, None);
    }

    #[tokio::test]
    async fn redis_cache_fails_open_when_unreachable() {
        // Port 1 refuses immediately — get degrades to a miss, put is a no-op, neither panics.
        let client = redis::Client::open("redis://127.0.0.1:1/").unwrap();
        let c = RedisCache::new(client, 60);
        c.put(
            "conv",
            "agent",
            &CachedDecision {
                model: "m".into(),
                tier: Some(Tier::Tier2),
                request_type: Some(RequestType::Writing),
                complexity: Some(2),
                confidence: Some(0.8),
            },
        )
        .await;
        assert!(c.get("conv", "agent").await.is_none());
    }
}
