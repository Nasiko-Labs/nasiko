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
use nasiko_runtime::{ContainerRuntime, DeploymentSpec, DeploymentStatus, WorkloadKind};
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

/// How one deploy runs an agent: within which replica bounds, and whether the platform parks
/// it when idle. Read once, from one look at the agent, so the two cannot disagree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeployPlan {
    pub bounds: ReplicaBounds,
    /// Becomes [`DeploymentSpec::parkable`]; see [`deploys_parkable`].
    pub parkable: bool,
}

impl DeployPlan {
    /// This plan for a deploy that runs the agent as `kind`, which may differ from its row (a
    /// request that names the kind): only a framework agent's workload is ever parkable.
    pub fn for_kind(self, kind: WorkloadKind) -> Self {
        Self {
            parkable: self.parkable && kind == WorkloadKind::Agent,
            ..self
        }
    }
}

/// Whether an agent of `workload_kind` (the `agents.workload_kind` token) whose owner chose
/// `policy` deploys as a parkable workload ([`DeploymentSpec::parkable`]): exactly when
/// [`oss_parking_rule`] says it is parkable. An edition holds such an agent to one replica.
pub fn deploys_parkable(workload_kind: &str, policy: ParkingPolicy) -> bool {
    oss_parking_rule(workload_kind, policy) == ParkingEligibility::Parkable
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

    /// The agent's parking policy was just saved, which may change how many replicas it may
    /// run. Must return at once. Nothing to do where every agent runs one replica.
    fn parking_policy_changed(&self, _agent_id: Uuid) {}

    /// Called once at startup with the store every HITL pause goes through. May wrap it.
    fn observe_hitl_store(&self, store: Arc<dyn HitlStore>) -> Arc<dyn HitlStore>;

    /// Whether an owner's `resident` choice keeps a framework agent's workload up. `true` here:
    /// the idle reaper parks only what [`oss_parking_rule`] calls parkable. An edition that parks
    /// every framework agent, whatever its owner chose, answers `false`; its
    /// [`AgentDeploy::plan`] must then call every framework agent parkable too, so each one
    /// deploys as a workload that can be parked.
    fn honours_resident_policy(&self) -> bool {
        true
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

    /// A request's `parking_policy` field. `None` when it is absent or blank, which keeps the
    /// stored policy (a new agent gets the column default, `auto`).
    pub fn from_request_field(value: Option<&str>) -> Result<Option<Self>, UnknownParkingPolicy> {
        value.filter(|v| !v.is_empty()).map(str::parse).transpose()
    }
}

/// A parking policy name that is neither `auto` nor `resident`. Its message is the one a
/// request naming it is refused with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownParkingPolicy(pub String);

impl std::fmt::Display for UnknownParkingPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "parking_policy must be one of: auto, resident (got {:?})",
            self.0
        )
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

/// The parking rule: a framework agent (`workload_kind = 'agent'`) whose owner left its parking
/// policy at `auto` may be parked; every other agent stays resident. Nothing else counts: an agent
/// that keeps task state in memory loses it when parked, a `--writable` agent finds its files
/// again on its volume when woken, and a parkable agent runs one replica.
pub fn oss_parking_rule(workload_kind: &str, policy: ParkingPolicy) -> ParkingEligibility {
    use ParkingEligibility::{Parkable, Resident};
    match workload_kind {
        "agent" if policy == ParkingPolicy::Resident => Resident(ResidentReason::PolicyResident),
        "agent" => Parkable,
        "harness" => Resident(ResidentReason::Harness),
        _ => Resident(ResidentReason::NotAnAgent),
    }
}

/// The live agent's kind and parking policy, `None` when there is no live agent.
async fn load_parking_inputs(
    db: &sqlx::PgPool,
    agent_id: Uuid,
) -> sqlx::Result<Option<(String, ParkingPolicy)>> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT workload_kind, parking_policy FROM agents WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(db)
    .await?;
    row.map(|(workload_kind, policy)| {
        let policy = policy
            .parse()
            .map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
        Ok((workload_kind, policy))
    })
    .transpose()
}

/// Whether the live agent `agent_id` runs as a parkable workload under `scaling`, as its next
/// deploy's plan would say, without starting a deploy: [`deploys_parkable`] over its row, with the
/// owner's `resident` counting as `auto` where `scaling` does not honour it. `false` when there is
/// no live agent.
pub async fn runs_parkable(
    scaling: &dyn AgentScaling,
    db: &sqlx::PgPool,
    agent_id: Uuid,
) -> sqlx::Result<bool> {
    let Some((workload_kind, policy)) = load_parking_inputs(db, agent_id).await? else {
        return Ok(false);
    };
    let policy = if scaling.honours_resident_policy() {
        policy
    } else {
        ParkingPolicy::Auto
    };
    Ok(deploys_parkable(&workload_kind, policy))
}

/// Saves the owner's parking choice (`agents.parking_policy`) for the live agent `agent_id`.
/// Saving the policy it already has writes nothing.
pub async fn save_parking_policy(
    db: impl sqlx::PgExecutor<'_>,
    agent_id: Uuid,
    policy: ParkingPolicy,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE agents SET parking_policy = $2, updated_at = now() \
         WHERE id = $1 AND deleted_at IS NULL AND parking_policy <> $2",
    )
    .bind(agent_id)
    .bind(policy.as_str())
    .execute(db)
    .await
    .map(|_| ())
}

/// One deploy of one agent, from [`AgentScaling::begin_deploy`].
#[async_trait]
pub trait AgentDeploy: Send + Sync {
    /// Read right before the deploy env is built, from the agent's row as it stands then, so a
    /// parking policy or bounds change saved while the image built is the one deployed. An
    /// agent with no live row deploys one replica and is not parkable. An error fails the
    /// deploy.
    async fn plan(&self) -> sqlx::Result<DeployPlan>;

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
    async fn begin_deploy(&self, agent_id: Uuid) -> Box<dyn AgentDeploy> {
        Box::new(SingleReplicaDeploy {
            runtime: self.runtime.clone(),
            db: self.db.clone(),
            agent_id,
        })
    }

    async fn is_autoscaled(&self, _agent_id: Uuid) -> sqlx::Result<bool> {
        Ok(false)
    }

    fn agent_became_durable(&self, _agent_id: Uuid) {}

    fn observe_hitl_store(&self, store: Arc<dyn HitlStore>) -> Arc<dyn HitlStore> {
        store
    }
}

struct SingleReplicaDeploy {
    runtime: Arc<dyn ContainerRuntime>,
    db: sqlx::PgPool,
    agent_id: Uuid,
}

#[async_trait]
impl AgentDeploy for SingleReplicaDeploy {
    async fn plan(&self) -> sqlx::Result<DeployPlan> {
        let parkable = load_parking_inputs(&self.db, self.agent_id)
            .await?
            .is_some_and(|(kind, policy)| deploys_parkable(&kind, policy));
        Ok(DeployPlan {
            bounds: ReplicaBounds::SINGLE,
            parkable,
        })
    }

    async fn deploy(&self, spec: &DeploymentSpec) -> nasiko_runtime::Result<DeploymentStatus> {
        self.runtime.deploy(spec).await
    }
}

#[cfg(test)]
mod tests {
    use super::ParkingEligibility::{Parkable, Resident};
    use super::*;

    #[test]
    fn only_a_framework_agent_left_at_auto_is_parkable() {
        let cases = [
            ("agent", ParkingPolicy::Auto, Parkable),
            (
                "agent",
                ParkingPolicy::Resident,
                Resident(ResidentReason::PolicyResident),
            ),
            (
                "harness",
                ParkingPolicy::Auto,
                Resident(ResidentReason::Harness),
            ),
            (
                "harness",
                ParkingPolicy::Resident,
                Resident(ResidentReason::Harness),
            ),
            (
                "mcp_connector",
                ParkingPolicy::Auto,
                Resident(ResidentReason::NotAnAgent),
            ),
            (
                "acp_coding_agent",
                ParkingPolicy::Auto,
                Resident(ResidentReason::NotAnAgent),
            ),
        ];
        for (kind, policy, want) in cases {
            assert_eq!(oss_parking_rule(kind, policy), want, "{kind} {policy:?}");
            assert_eq!(
                deploys_parkable(kind, policy),
                want == Parkable,
                "{kind} {policy:?}"
            );
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
    fn a_plan_is_parkable_only_for_a_framework_agent_deploy() {
        let parkable = DeployPlan {
            bounds: ReplicaBounds::SINGLE,
            parkable: true,
        };
        assert!(parkable.for_kind(WorkloadKind::Agent).parkable);
        assert!(!parkable.for_kind(WorkloadKind::AcpCodingAgent).parkable);
        assert!(!parkable.for_kind(WorkloadKind::McpConnector).parkable);
        let resident = DeployPlan {
            parkable: false,
            ..parkable
        };
        assert!(!resident.for_kind(WorkloadKind::Agent).parkable);
    }

    #[test]
    fn a_request_field_names_a_policy_or_keeps_the_stored_one() {
        assert_eq!(ParkingPolicy::from_request_field(None), Ok(None));
        assert_eq!(ParkingPolicy::from_request_field(Some("")), Ok(None));
        assert_eq!(
            ParkingPolicy::from_request_field(Some("auto")),
            Ok(Some(ParkingPolicy::Auto))
        );
        assert_eq!(
            ParkingPolicy::from_request_field(Some("resident")),
            Ok(Some(ParkingPolicy::Resident))
        );
        let refused = ParkingPolicy::from_request_field(Some("sometimes")).unwrap_err();
        assert!(
            refused.to_string().contains("auto, resident"),
            "the refusal names the valid values: {refused}"
        );
    }

    #[test]
    fn eligibility_serializes_as_tag_and_reason() {
        let parkable = serde_json::to_value(Parkable).unwrap();
        assert_eq!(parkable, serde_json::json!({"eligibility": "parkable"}));
        let resident = serde_json::to_value(Resident(ResidentReason::PolicyResident)).unwrap();
        assert_eq!(
            resident,
            serde_json::json!({"eligibility": "resident", "reason": "policy_resident"})
        );
    }
}
