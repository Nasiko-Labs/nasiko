//! What a deployment may decide about a MAF workflow beyond what the routes do.
//!
//! The workflow routes (`crate::maf`) resolve each step to an agent, check the
//! caller may use it, store the definition, and later queue runs. A policy gets a
//! say at two points: it may rewrite or reject a definition before it is stored,
//! and reject a run before it is queued.
//!
//! The open-source policy is [`NoMafWorkflowPolicy`]: it stores only the fields
//! this crate defines and accepts every run. A distribution with more to enforce
//! replaces it on [`crate::state::AppState`] at its composition root, the same way
//! it replaces `routing_engine`.

use async_trait::async_trait;
use nasiko_orchestrator::maf::types::MafDefinition;
use sqlx::PgPool;
use uuid::Uuid;

#[async_trait]
pub trait MafWorkflowPolicy: Send + Sync {
    /// The last word on a definition before it is stored, once every step has its
    /// agent. May fill in or rewrite it; `Err` rejects the save with `400`.
    /// `previous` is the stored definition an update replaces; `None` on create.
    async fn finalize(
        &self,
        db: &PgPool,
        user_id: Uuid,
        definition: MafDefinition,
        previous: Option<&MafDefinition>,
    ) -> Result<MafDefinition, String>;

    /// Called before a run is queued, after the access checks. `Err` rejects the
    /// run with `400`.
    async fn check_run(
        &self,
        db: &PgPool,
        user_id: Uuid,
        definition: &MafDefinition,
        content: Option<&str>,
    ) -> Result<(), String>;
}

/// The open-source policy: the routes' own behavior, unchanged.
pub struct NoMafWorkflowPolicy;

#[async_trait]
impl MafWorkflowPolicy for NoMafWorkflowPolicy {
    /// Drops every step field this crate does not define, so what is stored is
    /// exactly what the routes stored before steps could carry extra fields.
    async fn finalize(
        &self,
        _db: &PgPool,
        _user_id: Uuid,
        mut definition: MafDefinition,
        _previous: Option<&MafDefinition>,
    ) -> Result<MafDefinition, String> {
        for step in &mut definition.steps {
            step.ext.clear();
        }
        Ok(definition)
    }

    async fn check_run(
        &self,
        _db: &PgPool,
        _user_id: Uuid,
        _definition: &MafDefinition,
        _content: Option<&str>,
    ) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nasiko_orchestrator::maf::types::MafStep;

    /// Never connected. The assertions below are that the open-source policy does
    /// not touch it: a query would hang or error rather than pass.
    fn unusable_pool() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://127.0.0.1:1/does-not-exist")
            .expect("a lazy pool is built without connecting")
    }

    fn step_with_ext(ext: serde_json::Map<String, serde_json::Value>) -> MafStep {
        MafStep {
            step_id: Uuid::new_v4(),
            step_index: 0,
            agent_id: Uuid::new_v4(),
            agent_name: "agent".to_string(),
            agent_endpoint: "http://localhost:1".to_string(),
            task_description: "task".to_string(),
            ext,
        }
    }

    #[tokio::test]
    async fn finalize_stores_only_the_fields_this_crate_defines() {
        let mut ext = serde_json::Map::new();
        ext.insert("to".to_string(), serde_json::json!([{"node_id": "next"}]));
        let definition = MafDefinition {
            description: None,
            steps: vec![step_with_ext(ext)],
            output_generation: None,
        };

        let stored = NoMafWorkflowPolicy
            .finalize(&unusable_pool(), Uuid::new_v4(), definition, None)
            .await
            .expect("the open-source policy never rejects a definition");

        let json = serde_json::to_value(&stored.steps[0]).unwrap();
        assert!(
            json.get("to").is_none(),
            "extra step fields must be dropped"
        );
        assert_eq!(json["task_description"], "task");
    }

    #[tokio::test]
    async fn accepts_every_run() {
        let definition = MafDefinition {
            description: None,
            steps: vec![step_with_ext(serde_json::Map::new())],
            output_generation: None,
        };

        assert!(
            NoMafWorkflowPolicy
                .check_run(&unusable_pool(), Uuid::new_v4(), &definition, None)
                .await
                .is_ok()
        );
    }
}
