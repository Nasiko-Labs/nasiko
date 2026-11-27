//! Deploying an agent's workload from what the catalog stores about it, with no request to say
//! more: what startup reconciliation does for a `running` agent whose workload is missing, and
//! what the wake does for a parked agent that has none (one parked before parking scaled
//! workloads to zero, which destroyed them).
//!
//! The spec is a redeploy's: the agent's secrets and the platform's env, the port its last
//! deployment recorded, its writable mount and its pull credential, with the bounds and
//! parkability the scaling seam plans, deployed through the runtime every deploy goes through
//! (and so through every decorator an edition wraps it in).
//!
//! The wake holds no [`AppState`] (the state holds the wake), so it asks [`run`], a worker that
//! does, through a [`CatalogDeployer`], as deploys ask the build worker. A request for an agent
//! whose deploy is still under way (a pull that outlasted the wake that asked for it) joins that
//! deploy rather than start a second one of the same workload.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use nasiko_runtime::RuntimeError;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::agent_wake::WakeRefusal;
use crate::agent_wake::parked::WorkloadDeployer;
use crate::state::AppState;

/// The `agents` columns a deploy from the catalog reads.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CatalogAgent {
    pub id: Uuid,
    pub name: String,
    image: Option<String>,
    owner_id: Uuid,
    writable: bool,
    writable_path: Option<String>,
}

impl CatalogAgent {
    /// The image to deploy, qualified as every deploy path qualifies it; `None` when the agent
    /// has none (registered by URL).
    fn qualified_image(&self, registry: &str) -> Option<String> {
        self.image
            .as_deref()
            .filter(|image| !image.is_empty())
            .map(|image| crate::agents::qualify_deploy_image(registry, image))
    }
}

/// The live agent's catalog row, whatever its status; `None` when there is no live agent.
pub async fn load(db: &sqlx::PgPool, agent_id: Uuid) -> sqlx::Result<Option<CatalogAgent>> {
    sqlx::query_as(
        "SELECT id, name, image, owner_id, writable, writable_path FROM agents \
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(db)
    .await
}

/// Why a deploy from the catalog did not happen.
#[derive(Debug, thiserror::Error)]
pub enum CatalogDeployError {
    #[error("the agent has no image to deploy")]
    NoImage,
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
    #[error("runtime: {0}")]
    Runtime(#[from] RuntimeError),
}

/// What a deploy from the catalog started.
#[derive(Debug)]
pub struct Deployed {
    /// The address the runtime reports for the new workload (empty when it has none yet).
    pub url: String,
    /// The image it runs, qualified.
    pub image: String,
}

/// Deploys `agent`'s workload from its catalog row, and records the deployment so the crash guardian
/// watches it (`ensure_deployment_tracked`). Leaves the agent's row alone: each caller records what
/// the deploy means for it.
pub async fn deploy(
    state: &AppState,
    agent: &CatalogAgent,
) -> Result<Deployed, CatalogDeployError> {
    let image = agent
        .qualified_image(&state.config.agent_image_registry)
        .ok_or(CatalogDeployError::NoImage)?;
    // Read before the deploy env is built, as on every deploy path.
    let scaling = state.agent_scaling.begin_deploy(agent.id).await;
    let plan = scaling.plan().await?;
    // The same image comes back, so a durable agent is durable again as it starts.
    let mut env = crate::agents::deploy_env::build(
        state,
        agent.id,
        agent.owner_id,
        HashMap::new(),
        crate::agents::deploy_env::StateModeOnDeploy::Keep,
    )
    .await;
    let ports = crate::agents::recorded_ports(&state.db, agent.id, &mut env).await;
    let mut spec = crate::agents::build_agent_spec(
        agent.id,
        &agent.name,
        image.clone(),
        ports,
        env,
        &state.config.agent_default_memory,
        plan,
        agent.writable,
        agent.writable_path.clone(),
        agent.owner_id,
    );
    crate::agents::attach_pull_credential(
        &state.db,
        &state.config.agent_runtime,
        &state.config.agent_image_registry,
        &mut spec,
        agent.id,
    )
    .await;
    let status = scaling.deploy(&spec).await?;
    let url = crate::agents::resolve_agent_url(&state.runtime, &status, &spec.container_id).await;
    crate::agents::utils::ensure_deployment_tracked(
        &state.db,
        agent.id,
        Some(agent.owner_id),
        &image,
    )
    .await;
    Ok(Deployed { url, image })
}

/// How a deploy for the wake went: `Err` carries the message its caller is refused with, and
/// whether the runtime declined the deploy.
type DeployOutcome = Result<(), WakeRefusal>;

/// What a caller is told when its agent's workload could not be deployed; the cause is logged.
const COULD_NOT_DEPLOY: &str = "the agent's workload could not be deployed";

/// One request from the wake: deploy this agent's workload, and say how it went.
struct DeployRequest {
    agent_id: Uuid,
    reply: oneshot::Sender<DeployOutcome>,
}

/// The wake's handle on [`run`]: deploys a parked agent's missing workload from its catalog row.
pub struct CatalogDeployer {
    requests: mpsc::UnboundedSender<DeployRequest>,
}

/// The requests a [`CatalogDeployer`] sends, for [`run`] to serve.
pub struct DeployRequests(mpsc::UnboundedReceiver<DeployRequest>);

/// A deployer for the wake and the requests [`run`] serves for it.
pub fn deployer() -> (CatalogDeployer, DeployRequests) {
    let (requests, receiver) = mpsc::unbounded_channel();
    (CatalogDeployer { requests }, DeployRequests(receiver))
}

#[async_trait]
impl WorkloadDeployer for CatalogDeployer {
    async fn deploy(&self, agent_id: Uuid) -> Result<(), WakeRefusal> {
        let (reply, answer) = oneshot::channel();
        self.requests
            .send(DeployRequest { agent_id, reply })
            .map_err(|_| "the deploy worker has stopped".to_owned())?;
        answer
            .await
            .map_err(|_| "the deploy worker dropped the request".to_owned())?
    }
}

/// Serves a [`CatalogDeployer`]'s requests for the life of the process. Exits when every deployer
/// is gone.
pub async fn run(state: AppState, requests: DeployRequests) {
    serve(requests, move |agent_id| {
        let state = state.clone();
        async move { deploy_missing_workload(&state, agent_id).await }
    })
    .await;
}

/// The waiting callers of each agent's deploy under way.
type InFlight = Arc<Mutex<HashMap<Uuid, Vec<oneshot::Sender<DeployOutcome>>>>>;

/// Serves `requests` with `deploy_agent`, each agent's deploy in its own task, so one slow image
/// pull does not hold up another agent's wake. A request for an agent whose deploy is under way
/// joins it and gets its outcome: a second deploy of the same workload would race the first. A
/// deploy runs to its end even if every caller stops waiting: the workload is then there for the
/// next call to scale up. Each deploy runs in a task of its own, so one that panics still answers
/// its callers and leaves no entry behind for later requests to join.
async fn serve<D, F>(DeployRequests(mut requests): DeployRequests, deploy_agent: D)
where
    D: Fn(Uuid) -> F,
    F: Future<Output = DeployOutcome> + Send + 'static,
{
    let in_flight = InFlight::default();
    while let Some(DeployRequest { agent_id, reply }) = requests.recv().await {
        if !join_or_start(&in_flight, agent_id, reply) {
            continue;
        }
        let deploying = tokio::spawn(deploy_agent(agent_id));
        let in_flight = in_flight.clone();
        tokio::spawn(async move {
            let outcome = deploying.await.unwrap_or_else(|e| {
                tracing::error!(%agent_id, error = %e, "agent wake: the deploy of a parked agent's missing workload panicked");
                Err(COULD_NOT_DEPLOY.to_owned().into())
            });
            let waiting = in_flight
                .lock()
                .expect("in-flight deploys lock poisoned")
                .remove(&agent_id)
                .unwrap_or_default();
            for reply in waiting {
                let _ = reply.send(outcome.clone());
            }
        });
    }
}

/// Adds `reply` to the callers of `agent_id`'s deploy; `true` when there was none under way, so
/// this caller starts it.
fn join_or_start(
    in_flight: &InFlight,
    agent_id: Uuid,
    reply: oneshot::Sender<DeployOutcome>,
) -> bool {
    let mut in_flight = in_flight.lock().expect("in-flight deploys lock poisoned");
    let waiting = in_flight.entry(agent_id).or_default();
    waiting.push(reply);
    waiting.len() == 1
}

/// Deploys a parked agent's workload and, once the runtime accepted it, refreshes its card in the
/// background: the image may be newer than the card the catalog stores (a deploy that parked an
/// agent fetched none). The cause of a failure is logged; the message carries no internal detail.
///
/// The dedupe in [`serve`] covers one replica. Across replicas the deploy runs under a lock of its
/// own, held until the deploy returns (it can outlast the wake that asked for it): a second
/// replica's request waits for it, then finds the workload there and scales it up instead.
async fn deploy_missing_workload(state: &AppState, agent_id: Uuid) -> DeployOutcome {
    let db_error = |e: sqlx::Error| logged(agent_id, &CatalogDeployError::Db(e));
    // On a connection of its own, not the pool's: the deploy below needs pooled connections while
    // this one holds the lock.
    let _lock = deploy_lock(state, agent_id).await.map_err(db_error)?;
    let container_id = nasiko_runtime::ContainerId::from_uuid(agent_id);
    // The same test the wake made: a workload that can be scaled is there.
    match state.runtime.scale(&container_id, 1).await {
        Err(RuntimeError::ContainerNotFound(_)) => {}
        Ok(()) => {
            tracing::info!(%agent_id, "agent wake: another replica deployed the missing workload");
            return Ok(());
        }
        Err(e) => return Err(logged(agent_id, &CatalogDeployError::Runtime(e))),
    }
    let agent = match load(&state.db, agent_id).await {
        Ok(Some(agent)) => agent,
        Ok(None) => return Err("the agent no longer exists".to_owned().into()),
        Err(e) => return Err(db_error(e)),
    };
    let deployed = deploy(state, &agent)
        .await
        .map_err(|e| logged(agent_id, &e))?;
    tracing::info!(%agent_id, image = %deployed.image, "agent wake: deployed a parked agent's missing workload");
    tokio::spawn(crate::agents::utils::fetch_agent_card_with_retry(
        state.db.clone(),
        state.http_client.clone(),
        state.runtime.clone(),
        state.llm_backend.clone(),
        agent_id,
        deployed.url,
    ));
    Ok(())
}

/// Takes `agent_id`'s missing-workload deploy lock, waiting up to the agent startup budget for a
/// deploy under way on another replica. Released when the returned connection is dropped (its
/// session ends).
async fn deploy_lock(state: &AppState, agent_id: Uuid) -> sqlx::Result<sqlx::PgConnection> {
    use sqlx::Connection as _;
    let mut conn = sqlx::PgConnection::connect_with(&state.db.connect_options()).await?;
    sqlx::query("SELECT set_config('lock_timeout', $1, false)")
        .bind(format!("{}s", state.config.agent_startup_budget_secs))
        .execute(&mut conn)
        .await?;
    sqlx::query("SELECT pg_advisory_lock(hashtext('nasiko.deploy_missing:' || $1))")
        .bind(agent_id.to_string())
        .execute(&mut conn)
        .await?;
    Ok(conn)
}

fn logged(agent_id: Uuid, e: &CatalogDeployError) -> WakeRefusal {
    tracing::warn!(%agent_id, error = %e, "agent wake: could not deploy a parked agent's missing workload");
    match e {
        // The refusing layer worded this for the caller.
        CatalogDeployError::Runtime(RuntimeError::Declined { message, .. }) => WakeRefusal {
            message: message.clone(),
            declined: true,
        },
        _ => COULD_NOT_DEPLOY.to_owned().into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(image: Option<&str>) -> CatalogAgent {
        CatalogAgent {
            id: Uuid::nil(),
            name: "echo".into(),
            image: image.map(str::to_owned),
            owner_id: Uuid::nil(),
            writable: false,
            writable_path: None,
        }
    }

    #[test]
    fn a_deploy_the_runtime_declined_is_reported_in_its_own_words_and_any_other_failure_generically()
     {
        let declined = CatalogDeployError::Runtime(RuntimeError::Declined {
            code: "plan_refused".into(),
            message: "This workspace cannot start more agents.".into(),
        });
        assert_eq!(
            logged(Uuid::nil(), &declined),
            WakeRefusal {
                message: "This workspace cannot start more agents.".into(),
                declined: true,
            }
        );
        for other in [
            CatalogDeployError::NoImage,
            CatalogDeployError::Db(sqlx::Error::PoolClosed),
            CatalogDeployError::Runtime(RuntimeError::Internal("boom".into())),
        ] {
            assert_eq!(
                logged(Uuid::nil(), &other),
                COULD_NOT_DEPLOY.to_owned().into(),
                "{other}"
            );
        }
    }

    use std::time::Duration;

    use tokio::sync::Semaphore;

    /// How many times each agent's deploy ran.
    type Runs = Arc<Mutex<HashMap<Uuid, usize>>>;

    /// Serves a deployer's requests with a deploy that counts its runs per agent and finishes once
    /// `gate` has a permit (closed while it has none).
    fn serving(runs: Runs, gate: Arc<Semaphore>) -> Arc<CatalogDeployer> {
        let (deployer, requests) = deployer();
        tokio::spawn(serve(requests, move |agent_id| {
            *runs.lock().unwrap().entry(agent_id).or_default() += 1;
            let gate = gate.clone();
            async move {
                let _open = gate.acquire().await.expect("the gate stays open");
                Ok(())
            }
        }));
        Arc::new(deployer)
    }

    fn runs_of(runs: &Runs, agent_id: Uuid) -> usize {
        runs.lock().unwrap().get(&agent_id).copied().unwrap_or(0)
    }

    async fn until(what: &str, check: impl Fn() -> bool) {
        for _ in 0..400 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("timed out waiting for {what}");
    }

    /// A wake that gave up on a slow pull leaves its deploy running; the next call's wake finds no
    /// workload yet and asks again, and joins that deploy rather than start a second one.
    #[tokio::test]
    async fn a_request_for_an_agent_already_deploying_joins_that_deploy() {
        let runs = Runs::default();
        let gate = Arc::new(Semaphore::new(0));
        let deployer = serving(runs.clone(), gate.clone());
        let (agent, other) = (Uuid::new_v4(), Uuid::new_v4());

        let first = tokio::spawn({
            let deployer = deployer.clone();
            async move { WorkloadDeployer::deploy(deployer.as_ref(), agent).await }
        });
        until("the first deploy to start", || runs_of(&runs, agent) == 1).await;
        // Sent in this order from one task, so the worker has taken the second request for
        // `agent` by the time it starts `other`'s deploy.
        let later = tokio::spawn({
            let deployer = deployer.clone();
            async move {
                tokio::join!(
                    WorkloadDeployer::deploy(deployer.as_ref(), agent),
                    WorkloadDeployer::deploy(deployer.as_ref(), other),
                )
            }
        });
        until("the other agent's deploy to start", || {
            runs_of(&runs, other) == 1
        })
        .await;
        gate.add_permits(Semaphore::MAX_PERMITS / 2);

        assert_eq!(first.await.unwrap(), Ok(()));
        let (second, elsewhere) = later.await.unwrap();
        assert_eq!((second, elsewhere), (Ok(()), Ok(())));
        assert_eq!(runs_of(&runs, agent), 1, "one deploy, joined");
    }

    /// A deploy that panics answers its callers with a refusal and leaves no entry behind: the next
    /// request starts a new deploy instead of joining one that will never answer.
    #[tokio::test]
    async fn a_deploy_that_panics_answers_its_callers_and_the_next_request_starts_another() {
        let runs = Runs::default();
        let (deployer, requests) = deployer();
        let counted = runs.clone();
        tokio::spawn(serve(requests, move |agent_id| {
            let run = {
                let mut runs = counted.lock().unwrap();
                let run = runs.entry(agent_id).or_default();
                *run += 1;
                *run
            };
            async move {
                assert!(run > 1, "the first deploy panics");
                Ok(())
            }
        }));
        let agent = Uuid::new_v4();

        assert_eq!(
            WorkloadDeployer::deploy(&deployer, agent).await,
            Err(COULD_NOT_DEPLOY.to_owned().into())
        );
        assert_eq!(WorkloadDeployer::deploy(&deployer, agent).await, Ok(()));

        assert_eq!(runs_of(&runs, agent), 2);
    }

    /// Once a deploy has finished, the next request for the agent starts a new one.
    #[tokio::test]
    async fn a_request_after_the_deploy_finished_starts_another() {
        let runs = Runs::default();
        let deployer = serving(
            runs.clone(),
            Arc::new(Semaphore::new(Semaphore::MAX_PERMITS / 2)),
        );
        let agent = Uuid::new_v4();

        assert_eq!(
            WorkloadDeployer::deploy(deployer.as_ref(), agent).await,
            Ok(())
        );
        assert_eq!(
            WorkloadDeployer::deploy(deployer.as_ref(), agent).await,
            Ok(())
        );

        assert_eq!(runs_of(&runs, agent), 2);
    }

    #[test]
    fn an_agent_without_an_image_has_nothing_to_deploy() {
        assert_eq!(agent(None).qualified_image("registry.example.com"), None);
        assert_eq!(
            agent(Some("")).qualified_image("registry.example.com"),
            None
        );
    }

    #[test]
    fn the_image_is_qualified_as_every_deploy_path_qualifies_it() {
        assert_eq!(
            agent(Some("nasiko/echo:1.0.0")).qualified_image("registry.example.com"),
            Some("registry.example.com/nasiko/echo:1.0.0".to_owned())
        );
    }
}
