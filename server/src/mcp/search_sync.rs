//! Keeps every replica's MCP tool-search index current.
//!
//! The index is in memory and rebuilt from `mcp_connector_tools` by whoever changed a connector.
//! [`install`] wraps the gateway's index so that every rebuild requested on this replica (a
//! connector registered, refreshed, deleted or built; the seed sync) also publishes
//! [`topic::MCP_TOOL_SEARCH`], after the local rebuild has finished and so after it stored any new
//! embeddings: the other replicas then read them back instead of each paying to embed the same
//! tools. On receipt a replica rebuilds its own index, debounced, so a burst of changes costs one
//! rebuild.
//!
//! No periodic rebuild: a rebuild reads every tool and embedding, and the one way a replica can
//! miss a change, its listener being down, already triggers a rebuild when it resubscribes
//! (the `invalidation` resync).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::PgPool;
use tokio::sync::mpsc;
use uuid::Uuid;

use nasiko_mcp_gateway::McpState;
use nasiko_mcp_gateway::permissions::EffectiveAccess;
use nasiko_mcp_gateway::search::{ToolMatch, ToolSearchIndex};

use crate::invalidation::{Invalidations, topic};

/// How long a replica waits after the first change it hears of before rebuilding, so the changes
/// that follow within it are covered by the same rebuild.
const REBUILD_DEBOUNCE: Duration = Duration::from_secs(2);

/// Wrap `mcp.search_index` so its rebuilds are announced, and rebuild it when another replica
/// announces one.
pub fn install(mcp: &mut McpState, db: PgPool, invalidations: &Invalidations) {
    let local = mcp.search_index.clone();
    let (wake, requests) = mpsc::channel(1);
    tokio::spawn(rebuild_on_request(
        local.clone(),
        db,
        requests,
        REBUILD_DEBOUNCE,
    ));
    invalidations.register(topic::MCP_TOOL_SEARCH, move |_| {
        // Full means a rebuild is already pending, which covers this change too.
        let _ = wake.try_send(());
    });
    mcp.search_index = Arc::new(AnnouncingSearchIndex {
        inner: local,
        invalidations: invalidations.clone(),
    });
}

/// Rebuilds `index` once per burst of requests (those within `debounce` of the first); ends when
/// the handler is dropped.
async fn rebuild_on_request(
    index: Arc<dyn ToolSearchIndex>,
    db: PgPool,
    mut requests: mpsc::Receiver<()>,
    debounce: Duration,
) {
    while requests.recv().await.is_some() {
        tokio::time::sleep(debounce).await;
        while requests.try_recv().is_ok() {}
        if let Err(e) = index.rebuild(&db).await {
            tracing::warn!(%e, "rebuilding the tool-search index after another replica's change failed");
        }
    }
}

/// The gateway's index, with every rebuild announced to the other replicas.
struct AnnouncingSearchIndex {
    inner: Arc<dyn ToolSearchIndex>,
    invalidations: Invalidations,
}

#[async_trait]
impl ToolSearchIndex for AnnouncingSearchIndex {
    async fn search_tools(
        &self,
        query: &str,
        accessible_connector_ids: &[Uuid],
        access: &EffectiveAccess<'_>,
        limit: usize,
    ) -> Vec<ToolMatch> {
        self.inner
            .search_tools(query, accessible_connector_ids, access, limit)
            .await
    }

    fn granted_tools(
        &self,
        connector_ids: &[Uuid],
        access: &EffectiveAccess<'_>,
    ) -> Vec<ToolMatch> {
        self.inner.granted_tools(connector_ids, access)
    }

    /// Announced whether or not the local rebuild succeeded: the tools changed either way.
    async fn rebuild(&self, db: &PgPool) -> nasiko_mcp_gateway::error::Result<()> {
        let rebuilt = self.inner.rebuild(db).await;
        self.invalidations.publish(topic::MCP_TOOL_SEARCH);
        rebuilt
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct CountingIndex {
        rebuilds: AtomicUsize,
    }

    #[async_trait]
    impl ToolSearchIndex for CountingIndex {
        async fn search_tools(
            &self,
            _: &str,
            _: &[Uuid],
            _: &EffectiveAccess<'_>,
            _: usize,
        ) -> Vec<ToolMatch> {
            Vec::new()
        }
        fn granted_tools(&self, _: &[Uuid], _: &EffectiveAccess<'_>) -> Vec<ToolMatch> {
            Vec::new()
        }
        async fn rebuild(&self, _: &PgPool) -> nasiko_mcp_gateway::error::Result<()> {
            self.rebuilds.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn lazy_pool() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/unused")
            .unwrap()
    }

    /// A burst of requests inside the debounce window costs one rebuild.
    #[tokio::test]
    async fn a_burst_of_changes_is_one_rebuild() {
        const DEBOUNCE: Duration = Duration::from_millis(500);
        let index = Arc::new(CountingIndex::default());
        let (wake, requests) = mpsc::channel(1);
        let task = tokio::spawn(rebuild_on_request(
            index.clone(),
            lazy_pool(),
            requests,
            DEBOUNCE,
        ));
        for _ in 0..5 {
            let _ = wake.try_send(());
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tokio::time::sleep(DEBOUNCE * 2).await;
        assert_eq!(index.rebuilds.load(Ordering::SeqCst), 1);

        // A later change is a rebuild of its own.
        let _ = wake.try_send(());
        tokio::time::sleep(DEBOUNCE * 2).await;
        assert_eq!(index.rebuilds.load(Ordering::SeqCst), 2);

        drop(wake);
        task.await.unwrap();
    }
}
