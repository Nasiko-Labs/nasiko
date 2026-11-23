//! Sandbox-minutes metering: the sandbox half of the hours meter.
//!
//! Sandboxes exist only while serving a turn, so the unit is minutes of `Running` time. They are
//! recorded in `agent_instance_sessions` with `kind = 'sandbox'`, opened and closed by the same
//! rules as container runs (see the parent module): a failed `list()` never closes anything, a
//! sandbox that disappears or is no longer `Running` has its open row closed at `last_seen_at`.
//! Sandboxes the platform cannot attribute to an agent are skipped.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use nasiko_runtime::{SandboxInfo, SandboxKey, SandboxProvider, SandboxState};
use sqlx::PgPool;
use uuid::Uuid;

use super::{ReconcileStats, close_unobserved};

/// Who a sandbox's time is billed to, before the database resolves it to an agent.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Attribution {
    /// The sandbox belongs to this agent directly.
    Agent(Uuid),
    /// The sandbox belongs to the harness agent of this session.
    Session(Uuid),
}

/// The billing target a sandbox key names.
fn attribution(key: &SandboxKey) -> Attribution {
    match key {
        SandboxKey::Agent { agent_id } | SandboxKey::AgentContext { agent_id, .. } => {
            Attribution::Agent(*agent_id)
        }
        SandboxKey::Session { session_id } => Attribution::Session(*session_id),
    }
}

/// One Running sandbox, resolved to the agent it bills to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Observed {
    agent_id: Uuid,
    agent_name: String,
    instance_key: String,
    sandbox_key: String,
    runtime: String,
    started_at: Option<DateTime<Utc>>,
}

/// The agent (id, name snapshot) each attribution resolved to.
type AgentByKey = HashMap<Attribution, (Uuid, String)>;

/// The agent (id, name snapshot) of the session whose `sandbox_ref` is the map key.
type AgentByRef = HashMap<String, (Uuid, String)>;

/// What the database resolved this tick's Running sandboxes to.
#[derive(Debug, Default)]
struct Resolved {
    /// By key: agent sandboxes, and the fallback for sessions with no `sandbox_ref` match.
    by_key: AgentByKey,
    /// By `sandbox_ref`: the authoritative attribution of a session sandbox.
    by_ref: AgentByRef,
}

/// `chat_sessions.sandbox_ref` of a listed sandbox: `"<provider>:<backend_ref>"`.
fn sandbox_ref(info: &SandboxInfo) -> String {
    info.handle.sandbox_ref()
}

/// Keep the Running sandboxes that resolve to an agent. Anything else (starting, suspended,
/// absent, unattributable) is simply not observed, which is what closes its open row.
fn observe(infos: &[SandboxInfo], resolved: &Resolved) -> Vec<Observed> {
    let mut seen = HashSet::new();
    infos
        .iter()
        .filter(|i| i.state == SandboxState::Running)
        .filter(|i| seen.insert((i.handle.backend_ref.clone(), i.started_at)))
        .filter_map(|i| {
            // A session's stored ref is authoritative: its key may be a hash of a non-UUID
            // contextId, which cannot be reversed.
            let (agent_id, agent_name) = resolved
                .by_ref
                .get(&sandbox_ref(i))
                .or_else(|| resolved.by_key.get(&attribution(&i.handle.key)))?;
            Some(Observed {
                agent_id: *agent_id,
                agent_name: agent_name.clone(),
                instance_key: i.handle.backend_ref.clone(),
                sandbox_key: i.handle.key.name(),
                runtime: i.handle.provider.to_string(),
                started_at: i.started_at,
            })
        })
        .collect()
}

/// One observation pass over the sandbox provider. A `list()` error propagates untouched and
/// closes nothing.
pub async fn reconcile(
    db: &PgPool,
    provider: &dyn SandboxProvider,
) -> anyhow::Result<ReconcileStats> {
    let infos = provider.list().await?;
    let now = Utc::now();
    let running: Vec<&SandboxInfo> = infos
        .iter()
        .filter(|i| i.state == SandboxState::Running)
        .collect();
    let resolved = resolve(db, &running).await?;
    let observed = observe(&infos, &resolved);

    let mut stats = ReconcileStats {
        observed: observed.len(),
        ..Default::default()
    };
    // Close before opening: a sandbox that restarted has a new start time, and the unique index
    // allows one open row per sandbox.
    let keys: Vec<String> = observed.iter().map(|o| o.instance_key.clone()).collect();
    let starts: Vec<Option<DateTime<Utc>>> = observed.iter().map(|o| o.started_at).collect();
    stats.closed = close_unobserved(db, "sandbox", &keys, &starts).await?;
    stats.upserted = upsert(db, &observed, now).await?;
    Ok(stats)
}

/// Resolve every running sandbox's attribution to an agent: one query for agent keys and one for
/// session sandboxes, matched on their stored `sandbox_ref`. A session sandbox no row claims
/// falls back to its key as a session id (UUID-shaped contextIds only) and is logged at debug.
async fn resolve(db: &PgPool, running: &[&SandboxInfo]) -> anyhow::Result<Resolved> {
    let mut agents: Vec<Uuid> = Vec::new();
    let mut refs: Vec<String> = Vec::new();
    for info in running {
        match attribution(&info.handle.key) {
            Attribution::Agent(id) => agents.push(id),
            Attribution::Session(_) => refs.push(sandbox_ref(info)),
        }
    }
    let mut out = Resolved::default();
    if !agents.is_empty() {
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT id, COALESCE(display_name, name) FROM agents WHERE id = ANY($1)",
        )
        .bind(&agents)
        .fetch_all(db)
        .await?;
        out.by_key.extend(
            rows.into_iter()
                .map(|(id, n)| (Attribution::Agent(id), (id, n))),
        );
        for id in agents {
            // A deleted agent's sandbox still bills; keep a stable fallback name.
            out.by_key
                .entry(Attribution::Agent(id))
                .or_insert_with(|| (id, id.to_string()));
        }
    }
    if !refs.is_empty() {
        let rows: Vec<(String, Uuid, String)> = sqlx::query_as(
            r#"SELECT s.sandbox_ref, a.id, COALESCE(a.display_name, a.name)
               FROM chat_sessions s JOIN agents a ON a.id = s.agent_id
               WHERE s.sandbox_ref = ANY($1)"#,
        )
        .bind(&refs)
        .fetch_all(db)
        .await?;
        // No `deleted_at` filter: a soft-deleted session's sandbox still runs, and bills, until it
        // is released (as a deleted agent's does). Refs are unique per session.
        out.by_ref
            .extend(rows.into_iter().map(|(r, id, n)| (r, (id, n))));
        let unmatched: Vec<&SandboxInfo> = running
            .iter()
            .copied()
            .filter(|i| matches!(attribution(&i.handle.key), Attribution::Session(_)))
            .filter(|i| !out.by_ref.contains_key(&sandbox_ref(i)))
            .collect();
        resolve_by_session_id(db, &unmatched, &mut out).await?;
    }
    Ok(out)
}

/// Fallback for session sandboxes no `sandbox_ref` matched: the key's UUID as the contextId.
async fn resolve_by_session_id(
    db: &PgPool,
    unmatched: &[&SandboxInfo],
    out: &mut Resolved,
) -> anyhow::Result<()> {
    let ids: Vec<String> = unmatched
        .iter()
        .filter_map(|i| match &i.handle.key {
            SandboxKey::Session { session_id } => Some(session_id.to_string()),
            _ => None,
        })
        .collect();
    for info in unmatched {
        tracing::debug!(sandbox_ref = %sandbox_ref(info), "sandbox has no session row by ref");
    }
    if ids.is_empty() {
        return Ok(());
    }
    let rows: Vec<(String, Uuid, String)> = sqlx::query_as(
        r#"SELECT s.session_id, a.id, COALESCE(a.display_name, a.name)
           FROM chat_sessions s JOIN agents a ON a.id = s.agent_id
           WHERE s.session_id = ANY($1)"#,
    )
    .bind(&ids)
    .fetch_all(db)
    .await?;
    for (session_id, agent_id, name) in rows {
        if let Ok(id) = Uuid::parse_str(&session_id) {
            out.by_key
                .insert(Attribution::Session(id), (agent_id, name));
        }
    }
    Ok(())
}

/// Open or bump the session of every observed sandbox. Returns rows touched.
async fn upsert(db: &PgPool, observed: &[Observed], now: DateTime<Utc>) -> anyhow::Result<usize> {
    let mut touched = 0;
    for o in observed {
        touched += match o.started_at {
            Some(started_at) => upsert_known_start(db, o, started_at, now).await?,
            None => upsert_first_seen(db, o, now).await?,
        };
    }
    Ok(touched)
}

async fn upsert_known_start(
    db: &PgPool,
    o: &Observed,
    started_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> anyhow::Result<usize> {
    let res = sqlx::query(
        r#"INSERT INTO agent_instance_sessions
               (agent_id, agent_name, instance_key, runtime, started_at, last_seen_at,
                kind, sandbox_key)
           VALUES ($1, $2, $3, $4, $5, $6, 'sandbox', $7)
           ON CONFLICT ON CONSTRAINT agent_instance_sessions_run_unique
           DO UPDATE SET last_seen_at = EXCLUDED.last_seen_at, ended_at = NULL"#,
    )
    .bind(o.agent_id)
    .bind(&o.agent_name)
    .bind(&o.instance_key)
    .bind(&o.runtime)
    .bind(started_at)
    .bind(now)
    .bind(&o.sandbox_key)
    .execute(db)
    .await?;
    Ok(res.rows_affected() as usize)
}

/// No backend start time: bump the open row, or open one at first sight.
async fn upsert_first_seen(db: &PgPool, o: &Observed, now: DateTime<Utc>) -> anyhow::Result<usize> {
    let bumped = sqlx::query(
        r#"UPDATE agent_instance_sessions SET last_seen_at = $2, ended_at = NULL
           WHERE instance_key = $1 AND kind = 'sandbox' AND ended_at IS NULL"#,
    )
    .bind(&o.instance_key)
    .bind(now)
    .execute(db)
    .await?
    .rows_affected();
    if bumped > 0 {
        return Ok(bumped as usize);
    }
    // Two replicas can both reach here; the unique open-row index makes the second insert a
    // no-op, and its bump then lands on the first one's row.
    let inserted = sqlx::query(
        r#"INSERT INTO agent_instance_sessions
               (agent_id, agent_name, instance_key, runtime, started_at, last_seen_at,
                kind, sandbox_key)
           VALUES ($1, $2, $3, $4, $5, $5, 'sandbox', $6)
           ON CONFLICT DO NOTHING"#,
    )
    .bind(o.agent_id)
    .bind(&o.agent_name)
    .bind(&o.instance_key)
    .bind(&o.runtime)
    .bind(now)
    .bind(&o.sandbox_key)
    .execute(db)
    .await?
    .rows_affected();
    if inserted == 0 {
        sqlx::query(
            r#"UPDATE agent_instance_sessions SET last_seen_at = $2
               WHERE instance_key = $1 AND kind = 'sandbox' AND ended_at IS NULL"#,
        )
        .bind(&o.instance_key)
        .bind(now)
        .execute(db)
        .await?;
    }
    Ok(1)
}

/// Per-agent sandbox-minutes within a window; same overlap rule as the container hours.
#[derive(Debug, sqlx::FromRow)]
pub struct AgentSandboxMinutes {
    pub agent_id: Uuid,
    pub minutes: f64,
}

/// Sum each sandbox row's overlap with `[start, end)` in minutes, grouped by agent; `agents`
/// narrows it as in `windowed_agent_hours`.
pub async fn windowed_sandbox_minutes(
    db: &PgPool,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    agents: Option<&[Uuid]>,
) -> sqlx::Result<Vec<AgentSandboxMinutes>> {
    sqlx::query_as(
        r#"SELECT s.agent_id,
               (SUM(GREATEST(EXTRACT(EPOCH FROM (
                    LEAST(COALESCE(s.ended_at, s.last_seen_at), $2)
                  - GREATEST(s.started_at, $1))), 0)) / 60.0)::float8 AS minutes
           FROM agent_instance_sessions s
           WHERE s.kind = 'sandbox'
             AND s.started_at < $2
             AND COALESCE(s.ended_at, s.last_seen_at) > $1
             AND ($3::uuid[] IS NULL OR s.agent_id = ANY($3))
           GROUP BY s.agent_id"#,
    )
    .persistent(false)
    .bind(start)
    .bind(end)
    .bind(agents)
    .fetch_all(db)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use nasiko_runtime::sandbox_fake::FakeSandboxProvider;
    use nasiko_runtime::{SandboxSpec, TransportKind};

    fn spec(key: SandboxKey) -> SandboxSpec {
        SandboxSpec {
            key,
            image: "img:1".into(),
            command: vec![],
            env: Default::default(),
            resources: None,
            workspace: None,
            runtime_class: None,
            ports: vec![8080],
            transport: TransportKind::Http,
            image_pull_secret: None,
        }
    }

    async fn list_after(provider: &FakeSandboxProvider, keys: &[SandboxKey]) -> Vec<SandboxInfo> {
        for k in keys {
            provider.acquire(&spec(k.clone())).await.unwrap();
        }
        provider.list().await.unwrap()
    }

    #[test]
    fn keys_attribute_to_agent_or_session() {
        let (a, s) = (Uuid::from_u128(1), Uuid::from_u128(2));
        assert_eq!(
            attribution(&SandboxKey::Agent { agent_id: a }),
            Attribution::Agent(a)
        );
        assert_eq!(
            attribution(&SandboxKey::AgentContext {
                agent_id: a,
                context_id: "c".into()
            }),
            Attribution::Agent(a)
        );
        assert_eq!(
            attribution(&SandboxKey::Session { session_id: s }),
            Attribution::Session(s)
        );
    }

    #[tokio::test]
    async fn only_running_attributable_sandboxes_are_observed() {
        let (agent, harness, other) = (Uuid::from_u128(1), Uuid::from_u128(9), Uuid::from_u128(5));
        let session = Uuid::from_u128(2);
        let provider = FakeSandboxProvider::default();
        let keys = [
            SandboxKey::Agent { agent_id: agent },
            SandboxKey::Session {
                session_id: session,
            },
            SandboxKey::Session {
                session_id: Uuid::from_u128(3),
            },
        ];
        let infos = list_after(&provider, &keys).await;
        let by_key: AgentByKey = [
            (Attribution::Agent(agent), (agent, "a".to_string())),
            (Attribution::Session(session), (harness, "h".to_string())),
            // Resolves for a key that is not listed; must not appear.
            (Attribution::Agent(other), (other, "o".to_string())),
        ]
        .into();
        let resolved = Resolved {
            by_key,
            by_ref: Default::default(),
        };

        let observed = observe(&infos, &resolved);
        let mut ids: Vec<Uuid> = observed.iter().map(|o| o.agent_id).collect();
        ids.sort();
        // The third session has no resolution, so it is skipped.
        assert_eq!(ids, vec![agent, harness]);
        assert!(observed.iter().all(|o| o.runtime == "fake"));
        assert!(
            observed
                .iter()
                .any(|o| o.sandbox_key == format!("s-{session}"))
        );
    }

    #[tokio::test]
    async fn suspended_sandbox_is_no_longer_observed() {
        let agent = Uuid::from_u128(1);
        let provider = FakeSandboxProvider::default();
        let key = SandboxKey::Agent { agent_id: agent };
        let infos = list_after(&provider, std::slice::from_ref(&key)).await;
        let resolved = Resolved {
            by_key: [(Attribution::Agent(agent), (agent, "a".to_string()))].into(),
            by_ref: Default::default(),
        };
        assert_eq!(observe(&infos, &resolved).len(), 1);

        provider.suspend(&infos[0].handle).await.unwrap();
        let after = provider.list().await.unwrap();
        assert!(observe(&after, &resolved).is_empty());

        provider.release(&infos[0].handle).await.unwrap();
        assert!(observe(&provider.list().await.unwrap(), &resolved).is_empty());
    }

    #[tokio::test]
    async fn session_sandbox_is_attributed_by_its_stored_ref() {
        // A non-UUID contextId hashes into the key, so only the stored ref can resolve it.
        let (harness, hashed) = (Uuid::from_u128(9), Uuid::from_u128(7));
        let provider = FakeSandboxProvider::default();
        let infos = list_after(&provider, &[SandboxKey::Session { session_id: hashed }]).await;
        let by_ref: AgentByRef = [(sandbox_ref(&infos[0]), (harness, "h".to_string()))].into();
        let resolved = Resolved {
            by_key: Default::default(),
            by_ref,
        };
        let observed = observe(&infos, &resolved);
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].agent_id, harness);
        assert_eq!(observed[0].sandbox_key, format!("s-{hashed}"));
        assert!(observe(&infos, &Resolved::default()).is_empty());
    }
}
