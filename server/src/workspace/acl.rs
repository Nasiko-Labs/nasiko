//! Who may *read* a workspace file.
//!
//! The seam an enterprise edition wraps to add team and department grants, layered
//! exactly the way `AuthService::can_access_agent` is: the open-source policy answers
//! for the owner, a direct `user` grant and a `public` grant; anything org-shaped is
//! the wrapper's business. Writes are never delegated — only the owner writes,
//! always — so there is deliberately no `can_write`.
//!
//! Every answer fails closed: a store error denies, never grants.

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

use super::catalog::FileRow;

/// The read-access policy for workspace files. Received on `AppState` as
/// `workspace_acl`; the composition root decides which edition's policy is in play.
///
/// Both methods answer with a plain `bool` on purpose. A policy that cannot reach
/// its store must deny rather than surface an error to the caller, because the
/// alternative — an error path a caller might treat as "undecided" — is how a
/// denied read turns into a granted one.
#[async_trait]
pub trait WorkspaceAccessPolicy: Send + Sync {
    /// Whether `user_id` may read `file`.
    ///
    /// This is the **only** read gate for a file the caller does not own: the
    /// `read_file` tool, the download route and `GET /api/workspace/files/{id}`
    /// all route through it. It is not an existence oracle — callers map `false`
    /// to "no such file", so a denied reader learns nothing about whether the
    /// identifier it tried is real.
    async fn can_read(&self, db: &PgPool, user_id: Uuid, file: &FileRow) -> bool;

    /// Whether `sharer` may create a grant of `grant_type` to `grantee_id` — the
    /// check `share_file` and the grants route run before `catalog::add_grant`.
    ///
    /// Open source accepts `public` (sentinel grantee `*`) and `user` (an existing,
    /// undeleted account, which any user may share with); an enterprise policy
    /// scopes `user` by org visibility and admits `team` and `department`.
    async fn may_share_with(
        &self,
        db: &PgPool,
        sharer: Uuid,
        grant_type: &str,
        grantee_id: &str,
    ) -> bool;
}

/// The open-source policy: the owner, a direct `user` grant, or a `public` grant.
///
/// `team` and `department` grants are enterprise concepts; this policy never
/// grants them and never admits them as share targets, so an open-source binary
/// pointed at a database an enterprise deployment populated still honours only
/// the rows it understands.
pub struct OssWorkspaceAccess;

#[async_trait]
impl WorkspaceAccessPolicy for OssWorkspaceAccess {
    /// The owner reads without a query; everyone else needs a `user` grant naming
    /// them or a `public` grant on the file.
    async fn can_read(&self, db: &PgPool, user_id: Uuid, file: &FileRow) -> bool {
        if file.user_id == user_id {
            return true;
        }
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM workspace_file_grants WHERE file_id=$1
               AND ((grant_type='user' AND grantee_id=$2::text) OR grant_type='public'))",
        )
        .bind(file.id)
        .bind(user_id)
        .fetch_one(db)
        .await
        // Fail closed: a store error denies, never grants.
        .unwrap_or(false)
    }

    /// `public` must carry the `*` sentinel; `user` must name an existing, undeleted
    /// account; every other type is enterprise-only and refused here.
    async fn may_share_with(
        &self,
        db: &PgPool,
        _sharer: Uuid,
        grant_type: &str,
        grantee_id: &str,
    ) -> bool {
        match grant_type {
            "public" => grantee_id == "*",
            "user" => match Uuid::parse_str(grantee_id) {
                Ok(grantee) => sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND deleted_at IS NULL)",
                )
                .bind(grantee)
                .fetch_one(db)
                .await
                // Fail closed: a store error denies, never grants.
                .unwrap_or(false),
                Err(_) => false,
            },
            // `team` and `department` are enterprise grants.
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    /// A pool that never connects. The tests below assert precisely that the arms
    /// they exercise decide *without* the database: if one of them ever grew a
    /// query, the test would fail on a refused connection rather than pass.
    fn unusable_pool() -> PgPool {
        PgPool::connect_lazy("postgres://unused@127.0.0.1:1/unused")
            .expect("a lazy pool is constructed without connecting")
    }

    fn owned_file(owner: Uuid) -> FileRow {
        let now = Utc::now();
        FileRow {
            id: Uuid::new_v4(),
            agent_id: Uuid::new_v4(),
            user_id: owner,
            rel_path: "notes/plan.md".into(),
            blob_key: "blob".into(),
            size_bytes: 1,
            mime_type: "text/markdown".into(),
            digest: None,
            origin: "agent".into(),
            origin_agent_id: None,
            source_file_id: None,
            trace_id: "0".repeat(32),
            session_id: None,
            state: "kept".into(),
            staged_until: None,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    /// Ownership is decided from the row alone, so the owner's own reads never
    /// pay for, or depend on, a grants lookup.
    #[tokio::test]
    async fn the_owner_reads_without_touching_the_database() {
        let owner = Uuid::new_v4();
        let file = owned_file(owner);

        assert!(
            OssWorkspaceAccess
                .can_read(&unusable_pool(), owner, &file)
                .await
        );
    }

    #[tokio::test]
    async fn a_public_share_requires_the_star_sentinel() {
        let policy = OssWorkspaceAccess;
        let sharer = Uuid::new_v4();

        assert!(
            policy
                .may_share_with(&unusable_pool(), sharer, "public", "*")
                .await
        );
        assert!(
            !policy
                .may_share_with(&unusable_pool(), sharer, "public", "someone")
                .await
        );
    }

    #[tokio::test]
    async fn team_and_department_grants_are_refused_without_a_lookup() {
        let policy = OssWorkspaceAccess;
        let sharer = Uuid::new_v4();
        let grantee = Uuid::new_v4().to_string();

        for grant_type in ["team", "department"] {
            assert!(
                !policy
                    .may_share_with(&unusable_pool(), sharer, grant_type, &grantee)
                    .await,
                "{grant_type} grants are enterprise-only"
            );
        }
    }

    /// A grantee that is not a UUID can never name a user, so it is refused
    /// before the existence check — the parse failure is the answer.
    #[tokio::test]
    async fn a_user_share_to_a_malformed_id_is_refused_without_a_lookup() {
        assert!(
            !OssWorkspaceAccess
                .may_share_with(&unusable_pool(), Uuid::new_v4(), "user", "not-a-uuid")
                .await
        );
    }
}
