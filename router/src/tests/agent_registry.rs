use super::*;

#[test]
fn new_registry_has_empty_cache() {
    let registry = AgentRegistry::new(3600);
    // Cache starts empty — no way to get a hit without populating it
    let guard = registry.cache.try_read().unwrap();
    assert!(guard.is_none());
}

#[tokio::test]
async fn invalidate_clears_cache() {
    let registry = AgentRegistry::new(3600);
    // Manually populate the cache
    {
        let mut guard = registry.cache.write().await;
        *guard = Some(CachedAgents {
            agents: vec![],
            built_at: Instant::now(),
        });
    }
    // Verify it's populated
    {
        let guard = registry.cache.read().await;
        assert!(guard.is_some());
    }
    // Invalidate and verify it's gone
    registry.invalidate().await;
    let guard = registry.cache.read().await;
    assert!(guard.is_none());
}

#[tokio::test]
async fn cache_hit_within_ttl() {
    let registry = AgentRegistry::new(3600);
    {
        let mut guard = registry.cache.write().await;
        *guard = Some(CachedAgents {
            agents: vec![],
            built_at: Instant::now(),
        });
    }
    // Within TTL — should be a cache hit (elapsed << 3600s)
    let guard = registry.cache.read().await;
    let cached = guard.as_ref().unwrap();
    assert!(cached.built_at.elapsed() < Duration::from_secs(3600));
}
