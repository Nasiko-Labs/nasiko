//! Caller identity + per-repository access control for the OCI registry.
//!
//! `nasiko-oci` has no knowledge of the host application's auth types (it is a
//! lower-level crate mounted by `oss/server`), so the host is responsible for
//! authenticating the request and inserting a [`CallerIdentity`] into the
//! request extensions *before* it reaches this crate's routes — see
//! `oss/server/src/lib.rs`'s OCI mount point, which layers `require_auth` then
//! a small adapter middleware that copies the resolved identity across.

use axum::extract::FromRequestParts;
use axum::http::{StatusCode, request::Parts};

use crate::OciState;
use crate::error::{OciError, Result};

/// The authenticated caller, as resolved by the host application's auth layer.
#[derive(Debug, Clone)]
pub struct CallerIdentity {
    pub user_id: String,
    pub is_superuser: bool,
}

impl<S: Send + Sync> FromRequestParts<S> for CallerIdentity {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> std::result::Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<CallerIdentity>()
            .cloned()
            .ok_or((StatusCode::UNAUTHORIZED, "not authenticated"))
    }
}

/// `(does the caller own an agent named `repo`, does ANY agent own that name)`.
///
/// Two existence checks rather than fetching a single `owner_id` — agent
/// names are unique only **per owner** (migration 015's `(owner_id, name)`
/// index), and the OCI repo path is name-only (the `owner` path segment is a
/// constant written by every CLI caller, carrying no real per-tenant
/// meaning). If two different owners both have an agent named "foo", a
/// single-row lookup would pick an arbitrary one of them, wrongly denying
/// the caller who legitimately owns "their" foo, or wrongly comparing against
/// someone else's. Asking two yes/no questions instead is sound regardless of
/// how many owners share a name: "do I own a match" and "does anyone" never
/// require picking a single arbitrary winner.
async fn repo_claim_status(state: &OciState, caller: &CallerIdentity, repo: &str) -> Result<(bool, bool)> {
    let Ok(caller_uuid) = caller.user_id.parse::<uuid::Uuid>() else {
        return Ok((false, false));
    };
    sqlx::query_as(
        r#"SELECT
             EXISTS(SELECT 1 FROM agents WHERE name = $1 AND owner_id = $2 AND deleted_at IS NULL) AS owns,
             EXISTS(SELECT 1 FROM agents WHERE name = $1 AND deleted_at IS NULL) AS claimed"#,
    )
    .bind(repo)
    .bind(caller_uuid)
    .fetch_one(&state.pool)
    .await
    .map_err(OciError::Database)
}

/// Enforce that `caller` may operate on `repo` (the agent-name path segment).
///
/// Policy: superusers may always proceed. Otherwise, if an `agents` row
/// already exists with this name, the caller must own it. If no such row
/// exists yet, the request is allowed through — the CLI pushes an image
/// *before* registering the agent (`oss/cli/src/commands/push.rs`), so a
/// brand-new repo name has no owner to check against yet. This means a
/// not-yet-registered name is first-come-first-served until an agent claims
/// it; closing that narrower race is tracked separately (see MIGRATION plan,
/// Phase A3 follow-up) and is a materially smaller blast radius than the
/// "any authenticated stranger can read/delete any existing agent's image"
/// gap this closes.
///
/// Use [`check_repo_delete_access`] instead for destructive operations.
pub async fn check_repo_access(state: &OciState, caller: &CallerIdentity, repo: &str) -> Result<()> {
    if caller.is_superuser {
        return Ok(());
    }
    let (owns, claimed) = repo_claim_status(state, caller, repo).await?;
    if owns || !claimed {
        Ok(())
    } else {
        Err(OciError::Forbidden(format!("not permitted to access repository '{repo}'")))
    }
}

/// Stricter variant for destructive operations (blob/manifest delete).
///
/// Unlike [`check_repo_access`], an unclaimed repo name does NOT grant
/// access here: blob storage is globally content-addressed and deduplicated
/// by digest, so "no agent row yet" does not mean "nothing of value exists
/// under this digest" — a stranger could push to an unrelated, never-to-be-
/// registered repo name purely to reach `delete_blob` and destroy a blob
/// that's actually shared with someone else's claimed, registered agent.
/// Destroying something must require an existing claim (or superuser).
pub async fn check_repo_delete_access(state: &OciState, caller: &CallerIdentity, repo: &str) -> Result<()> {
    if caller.is_superuser {
        return Ok(());
    }
    let (owns, _claimed) = repo_claim_status(state, caller, repo).await?;
    if owns {
        Ok(())
    } else {
        Err(OciError::Forbidden(format!("not permitted to delete from repository '{repo}'")))
    }
}
