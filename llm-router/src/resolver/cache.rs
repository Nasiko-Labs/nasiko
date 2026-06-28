//! In-process TTL cache for per-agent `llm_config` lookups.
//!
//! Config edits take effect within `LLM_CONFIG_CACHE_TTL` with no redeploy. Only
//! *successful* agent fetches are cached (the inner `Option<LLMConfig>` distinguishes
//! "row exists, no llm_config" from "row exists with config"); a **missing agent row**
//! is an error and is never cached.

use std::time::{Duration, Instant};

use dashmap::DashMap;
use uuid::Uuid;

use super::LLMConfig;

/// Concurrency-safe TTL cache keyed by agent UUID.
pub struct ConfigCache {
    ttl: Duration,
    entries: DashMap<Uuid, Entry>,
}

struct Entry {
    expires_at: Instant,
    config: Option<LLMConfig>,
}

impl ConfigCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: DashMap::new(),
        }
    }

    /// `Some(config_state)` on a fresh hit (where `config_state` is the cached
    /// `Option<LLMConfig>`); `None` on a miss or an expired entry. Expired entries are
    /// left in place and overwritten by the next [`put`](Self::put).
    pub fn get(&self, agent_id: Uuid) -> Option<Option<LLMConfig>> {
        self.entries.get(&agent_id).and_then(|e| {
            if e.expires_at > Instant::now() {
                Some(e.config.clone())
            } else {
                None
            }
        })
    }

    /// Cache the result of a successful agent fetch.
    pub fn put(&self, agent_id: Uuid, config: Option<LLMConfig>) {
        self.entries.insert(
            agent_id,
            Entry {
                expires_at: Instant::now() + self.ttl,
                config,
            },
        );
    }

    /// Drop all cached entries (test/ops hook; also used after an `llm_config` edit).
    pub fn clear(&self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGENT: &str = "11111111-1111-1111-1111-111111111111";

    fn agent() -> Uuid {
        Uuid::parse_str(AGENT).unwrap()
    }

    #[test]
    fn miss_then_hit_then_clear() {
        let c = ConfigCache::new(Duration::from_secs(60));
        assert!(c.get(agent()).is_none());
        c.put(agent(), None);
        assert!(matches!(c.get(agent()), Some(None)));
        c.clear();
        assert!(c.get(agent()).is_none());
    }

    #[test]
    fn entry_expires_after_ttl() {
        let c = ConfigCache::new(Duration::from_millis(20));
        c.put(agent(), None);
        assert!(matches!(c.get(agent()), Some(None)));
        std::thread::sleep(Duration::from_millis(35));
        assert!(c.get(agent()).is_none());
    }
}
