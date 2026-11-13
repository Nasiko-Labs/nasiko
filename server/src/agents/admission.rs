//! Whether a new agent may be added, asked inside the transaction that inserts its row.
//!
//! Asked there, just before the insert, so a distribution that limits agents can count and insert
//! under one lock: adds that all passed an earlier count cannot all land. The open-source
//! admission, [`AdmitEveryAgent`], admits everything; a distribution with such a limit installs
//! its own on [`crate::state::AppState`] at its composition root.

use async_trait::async_trait;
use axum::Json;
use axum::http::StatusCode;
use uuid::Uuid;

#[async_trait]
pub trait AgentAdmission: Send + Sync {
    /// Asked inside the transaction that inserts a new agent row of `owner_id` named `name`, just
    /// before the insert. `Err` refuses it with the status and body to answer; a lock the
    /// admission takes on `conn` is held until that transaction ends.
    async fn admit(
        &self,
        conn: &mut sqlx::PgConnection,
        owner_id: Uuid,
        name: &str,
    ) -> Result<(), (StatusCode, Json<serde_json::Value>)>;
}

/// The open-source admission: never refuses.
pub struct AdmitEveryAgent;

#[async_trait]
impl AgentAdmission for AdmitEveryAgent {
    async fn admit(
        &self,
        _conn: &mut sqlx::PgConnection,
        _owner_id: Uuid,
        _name: &str,
    ) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
        Ok(())
    }
}
