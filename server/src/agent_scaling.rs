//! How many replicas an agent deploys with, who may change that, and whether it may be parked.
//!
//! OSS runs every agent at one replica: [`SingleReplica`] is the whole implementation here.
//! An edition that scales agents implements [`AgentScaling`] and is handed to
//! [`AppState::from_config_with_scaling`](crate::state::AppState::from_config_with_scaling) at
//! the composition root. It is a constructor argument rather than a late swap because the
//! build worker and the HITL dispatcher copy `AppState` while it is built, so a replacement
//! installed afterwards would never reach their deploys.

use std::sync::Arc;

use async_trait::async_trait;
use nasiko_hitl::HitlStore;
use nasiko_runtime::{ContainerRuntime, DeploymentSpec, DeploymentStatus};
use serde::Serialize;
use uuid::Uuid;

/// The replica range an agent deploys with. `min == max` is a fixed count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ReplicaBounds {
    pub min: u32,
    pub max: u32,
    /// Per-replica capacity hint for the autoscaler; `None` when not autoscaled or unset.
    pub target: Option<u32>,
}

impl ReplicaBounds {
    /// One replica, no autoscaling.
    pub const SINGLE: Self = Self {
        min: 1,
        max: 1,
        target: None,
    };

    pub fn is_autoscaled(&self) -> bool {
        self.max > self.min
    }
}

#[async_trait]
pub trait AgentScaling: Send + Sync {
    /// Starts one deploy of a registered agent. Call before its deploy env is built, which
    /// may reset the agent's `state_mode`.
    async fn begin_deploy(&self, agent_id: Uuid) -> Box<dyn AgentDeploy>;

    /// Whether an autoscaler owns the replica count, so a manual count above 0 would be undone.
    async fn is_autoscaled(&self, agent_id: Uuid) -> sqlx::Result<bool>;

    /// The agent just went from in-memory to durable state. Must return at once.
    fn agent_became_durable(&self, agent_id: Uuid);

    /// Called once at startup with the store every HITL pause goes through. May wrap it.
    fn observe_hitl_store(&self, store: Arc<dyn HitlStore>) -> Arc<dyn HitlStore>;

    /// Whether this agent may be parked (no running workload while idle) or must stay resident.
    ///
    /// Parking is decided here, next to the replica bounds, so both editions answer the same
    /// question from the same rule ([`oss_parking_rule`]); an edition adds only what it
    /// knows beyond that rule. The parking resolver is the only consumer.
    async fn parking_eligibility(&self, agent_id: Uuid) -> sqlx::Result<ParkingEligibility>;

    /// Whether a parkable agent gets a sandbox per A2A conversation rather than one shared by all
    /// callers. The shared one is right for any agent whose state is durable; an edition that
    /// parks agents keeping per-conversation state in memory says so here. Per-conversation
    /// sandboxes are only handed to a caller who owns the conversation.
    async fn per_context_sandbox(&self, _agent_id: Uuid) -> sqlx::Result<bool> {
        Ok(false)
    }
}

/// Whether an agent may be parked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(tag = "eligibility", content = "reason", rename_all = "snake_case")]
pub enum ParkingEligibility {
    Parkable,
    Resident(ResidentReason),
}

/// Why an agent must stay resident.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResidentReason {
    /// The owner chose `resident`.
    PolicyResident,
    /// It keeps task state in memory, which parking would lose.
    InMemoryState,
    /// An edition scales it past one replica; a parked agent has none.
    Autoscaled,
    /// An edition injects context into its requests through a proxy that waking a parked
    /// agent bypasses.
    MemoryInjection,
    /// Harness agents are parked per session by the sessions module.
    Harness,
    /// Connectors and legacy coding agents never park; also reported when there is no live
    /// agent row.
    NotAnAgent,
}

/// The owner's parking choice (`agents.parking_policy`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ParkingPolicy {
    /// The platform may park the agent when idle.
    Auto,
    /// Always running.
    Resident,
}

impl ParkingPolicy {
    /// The name stored in `agents.parking_policy` and used on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Resident => "resident",
        }
    }
}

/// A parking policy name that is neither `auto` nor `resident`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownParkingPolicy(pub String);

impl std::fmt::Display for UnknownParkingPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown parking policy {:?}", self.0)
    }
}

impl std::error::Error for UnknownParkingPolicy {}

impl TryFrom<String> for ParkingPolicy {
    type Error = UnknownParkingPolicy;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl std::str::FromStr for ParkingPolicy {
    type Err = UnknownParkingPolicy;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "resident" => Ok(Self::Resident),
            other => Err(UnknownParkingPolicy(other.to_owned())),
        }
    }
}

/// The `agents` columns the parking rule reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParkingInputs {
    pub workload_kind: String,
    pub parking_policy: ParkingPolicy,
    pub writable: bool,
    pub state_mode: String,
}

/// The open-source parking rule; editions that know more wrap it rather than copy it.
///
/// A `--writable` agent parks like any other: its state is the durable per-agent subPath
/// (`{owner_id}/{agent_id}`), which the idle-kill leaves in place and the woken sandbox
/// remounts. It still runs a single replica, because autoscaling bounds are decided
/// separately. Accepted trade-off: the reaper kills only after CPU has been quiescent for
/// the grace window, so a write not yet flushed at that instant is at risk.
pub fn oss_parking_rule(row: &ParkingInputs) -> ParkingEligibility {
    use ParkingEligibility::{Parkable, Resident};
    match row.workload_kind.as_str() {
        "agent" => {}
        "harness" => return Resident(ResidentReason::Harness),
        _ => return Resident(ResidentReason::NotAnAgent),
    }
    if row.parking_policy == ParkingPolicy::Resident {
        return Resident(ResidentReason::PolicyResident);
    }
    if row.state_mode == "memory" {
        return Resident(ResidentReason::InMemoryState);
    }
    Parkable
}

/// The live agent's parking inputs. A missing agent is [`ResidentReason::NotAnAgent`].
pub async fn load_parking_inputs(
    db: &sqlx::PgPool,
    agent_id: Uuid,
) -> sqlx::Result<Option<ParkingInputs>> {
    let row: Option<(String, String, bool, String)> = sqlx::query_as(
        "SELECT workload_kind, parking_policy, writable, state_mode \
         FROM agents WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(db)
    .await?;
    row.map(|(workload_kind, policy, writable, state_mode)| {
        Ok(ParkingInputs {
            workload_kind,
            parking_policy: policy
                .parse()
                .map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
            writable,
            state_mode,
        })
    })
    .transpose()
}

/// One deploy of one agent, from [`AgentScaling::begin_deploy`].
#[async_trait]
pub trait AgentDeploy: Send + Sync {
    /// Read right before the deploy env is built. An error fails the deploy.
    async fn bounds(&self) -> sqlx::Result<ReplicaBounds>;

    /// Used in place of [`ContainerRuntime::deploy`].
    async fn deploy(&self, spec: &DeploymentSpec) -> nasiko_runtime::Result<DeploymentStatus>;
}

/// The OSS behaviour: one replica per agent, no autoscaler.
pub struct SingleReplica {
    runtime: Arc<dyn ContainerRuntime>,
    db: sqlx::PgPool,
}

impl SingleReplica {
    pub fn new(runtime: Arc<dyn ContainerRuntime>, db: sqlx::PgPool) -> Self {
        Self { runtime, db }
    }
}

#[async_trait]
impl AgentScaling for SingleReplica {
    async fn begin_deploy(&self, _agent_id: Uuid) -> Box<dyn AgentDeploy> {
        Box::new(SingleReplicaDeploy {
            runtime: self.runtime.clone(),
        })
    }

    async fn is_autoscaled(&self, _agent_id: Uuid) -> sqlx::Result<bool> {
        Ok(false)
    }

    fn agent_became_durable(&self, _agent_id: Uuid) {}

    fn observe_hitl_store(&self, store: Arc<dyn HitlStore>) -> Arc<dyn HitlStore> {
        store
    }

    async fn parking_eligibility(&self, agent_id: Uuid) -> sqlx::Result<ParkingEligibility> {
        Ok(load_parking_inputs(&self.db, agent_id).await?.map_or(
            ParkingEligibility::Resident(ResidentReason::NotAnAgent),
            |row| oss_parking_rule(&row),
        ))
    }
}

struct SingleReplicaDeploy {
    runtime: Arc<dyn ContainerRuntime>,
}

#[async_trait]
impl AgentDeploy for SingleReplicaDeploy {
    async fn bounds(&self) -> sqlx::Result<ReplicaBounds> {
        Ok(ReplicaBounds::SINGLE)
    }

    async fn deploy(&self, spec: &DeploymentSpec) -> nasiko_runtime::Result<DeploymentStatus> {
        self.runtime.deploy(spec).await
    }
}

#[cfg(test)]
mod tests {
    use super::ParkingEligibility::{Parkable, Resident};
    use super::*;

    fn inputs(kind: &str, policy: ParkingPolicy, writable: bool, state: &str) -> ParkingInputs {
        ParkingInputs {
            workload_kind: kind.into(),
            parking_policy: policy,
            writable,
            state_mode: state.into(),
        }
    }

    #[test]
    fn the_parking_rule_covers_every_branch() {
        let cases = [
            (
                inputs("agent", ParkingPolicy::Auto, false, "durable"),
                Parkable,
            ),
            (
                inputs("agent", ParkingPolicy::Resident, false, "durable"),
                Resident(ResidentReason::PolicyResident),
            ),
            (
                inputs("agent", ParkingPolicy::Auto, true, "durable"),
                Parkable,
            ),
            (
                inputs("agent", ParkingPolicy::Resident, true, "durable"),
                Resident(ResidentReason::PolicyResident),
            ),
            (
                inputs("agent", ParkingPolicy::Auto, false, "memory"),
                Resident(ResidentReason::InMemoryState),
            ),
            (
                inputs("harness", ParkingPolicy::Auto, false, "durable"),
                Resident(ResidentReason::Harness),
            ),
            (
                inputs("mcp_connector", ParkingPolicy::Auto, false, "durable"),
                Resident(ResidentReason::NotAnAgent),
            ),
            (
                inputs("acp_coding_agent", ParkingPolicy::Auto, false, "durable"),
                Resident(ResidentReason::NotAnAgent),
            ),
            // Precedence: policy beats memory; writable no longer matters.
            (
                inputs("agent", ParkingPolicy::Resident, true, "memory"),
                Resident(ResidentReason::PolicyResident),
            ),
            (
                inputs("agent", ParkingPolicy::Auto, true, "memory"),
                Resident(ResidentReason::InMemoryState),
            ),
            // The kind beats everything.
            (
                inputs("harness", ParkingPolicy::Resident, true, "memory"),
                Resident(ResidentReason::Harness),
            ),
        ];
        for (row, want) in cases {
            assert_eq!(oss_parking_rule(&row), want, "{row:?}");
        }
    }

    #[test]
    fn parking_policies_round_trip_through_their_names() {
        for policy in [ParkingPolicy::Auto, ParkingPolicy::Resident] {
            assert_eq!(policy.as_str().parse(), Ok(policy));
            assert_eq!(
                serde_json::to_value(policy).unwrap(),
                serde_json::json!(policy.as_str())
            );
        }
        assert_eq!(
            "sometimes".parse::<ParkingPolicy>(),
            Err(UnknownParkingPolicy("sometimes".into()))
        );
    }

    #[test]
    fn eligibility_serializes_as_tag_and_reason() {
        let parkable = serde_json::to_value(Parkable).unwrap();
        assert_eq!(parkable, serde_json::json!({"eligibility": "parkable"}));
        let resident = serde_json::to_value(Resident(ResidentReason::InMemoryState)).unwrap();
        assert_eq!(
            resident,
            serde_json::json!({"eligibility": "resident", "reason": "in_memory_state"})
        );
    }
}
