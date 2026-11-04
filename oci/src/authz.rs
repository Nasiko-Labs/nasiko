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

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<CallerIdentity>()
            .cloned()
            .ok_or((StatusCode::UNAUTHORIZED, "not authenticated"))
    }
}

/// A pull-scoped identity resolved from HTTP Basic auth against a minted,
/// per-agent credential (see `pull_credentials`) — distinct from
/// [`CallerIdentity`] (bearer-JWT, session-based) so it's structurally
/// impossible for a pull credential to reach a mutating route: every write
/// handler in `routes::` takes [`Writer`] (which has no `PullOnly` variant
/// at all — see its doc), not [`Caller`], and delete handlers take
/// `CallerIdentity` directly. Only the read-only handlers that accept
/// [`Caller`] can ever see one.
#[derive(Debug, Clone, Copy)]
pub struct PullOnlyIdentity {
    pub agent_id: uuid::Uuid,
}

impl<S: Send + Sync> FromRequestParts<S> for PullOnlyIdentity {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<PullOnlyIdentity>()
            .copied()
            .ok_or((StatusCode::UNAUTHORIZED, "not authenticated"))
    }
}

/// Fixed username the shared build-push credential logs in as (HTTP Basic
/// auth). Must match the enterprise CLI's `BUILD_PUSH_USERNAME`
/// literal — that's where the matching `.dockerconfigjson` Secret is
/// rendered; the two aren't code-linked (the enterprise CLI doesn't depend on this
/// crate) so they're kept in sync by convention, documented on both sides.
pub const BUILD_SERVICE_USERNAME: &str = "build-service";

/// Marker extension inserted by the host's OCI auth middleware when a
/// request's `Authorization: Basic` credential matches the shared,
/// cluster-wide build-push token (see `oss/server/src/lib.rs`'s
/// `authenticate_oci_request` and `GeneratedSecrets::build_push_token`).
/// Not per-agent, unlike [`PullOnlyIdentity`] — the in-cluster BuildKit
/// build Job is server-orchestrated, network-isolated infrastructure, not a
/// per-tenant boundary, so one shared credential (same trust tier as
/// `JWT_SECRET`/Postgres/MinIO creds) is adequate. A compromised `buildkitd`
/// could overwrite any agent's image under this credential, not just the
/// one it's building — an accepted risk given a `buildkitd` compromise
/// already implies the ability to poison whatever it's actively building.
/// If a third registry-consuming context ever needs its own scoped
/// credential (a CI system, an external `docker login` user), that's the
/// point to build the real OCI Distribution token-auth flow instead of a
/// fourth bespoke credential type.
#[derive(Debug, Clone, Copy)]
pub struct BuildServiceIdentity;

impl<S: Send + Sync> FromRequestParts<S> for BuildServiceIdentity {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<BuildServiceIdentity>()
            .copied()
            .ok_or((StatusCode::UNAUTHORIZED, "not authenticated"))
    }
}

/// A normal session identity, a pull-scoped one, or the build-push service
/// identity — the extractor used by read-only routes (manifest/blob/tags
/// GET+HEAD) that must accept all three (BuildKit HEAD-checks blobs before
/// pushing, so the build-push identity needs read access too). See
/// [`check_pull_access`]. Write routes use the separate [`Writer`] enum,
/// which has no `PullOnly` variant.
#[derive(Debug, Clone)]
pub enum Caller {
    Session(CallerIdentity),
    PullOnly(PullOnlyIdentity),
    BuildService,
}

impl<S: Send + Sync> FromRequestParts<S> for Caller {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        if let Some(identity) = parts.extensions.get::<CallerIdentity>() {
            return Ok(Caller::Session(identity.clone()));
        }
        if let Some(identity) = parts.extensions.get::<PullOnlyIdentity>() {
            return Ok(Caller::PullOnly(*identity));
        }
        if parts.extensions.get::<BuildServiceIdentity>().is_some() {
            return Ok(Caller::BuildService);
        }
        Err((StatusCode::UNAUTHORIZED, "not authenticated"))
    }
}

/// A normal session identity or the build-push service identity — the
/// extractor used by write routes (blob upload init/patch/complete,
/// manifest put). Deliberately **no `PullOnly` variant**: a request carrying
/// only a [`PullOnlyIdentity`] extension cannot satisfy this extractor at
/// all, so it's structurally impossible (not just correctly-implemented-
/// today) for a pull credential to reach a write — mirrors why
/// [`PullOnlyIdentity`]'s own doc says the same about the routes that take
/// `CallerIdentity`/`Writer` instead of `Caller`. Delete routes are stricter
/// still and take `CallerIdentity` directly, so `BuildService` can never
/// delete either.
#[derive(Debug, Clone)]
pub enum Writer {
    Session(CallerIdentity),
    BuildService,
}

impl<S: Send + Sync> FromRequestParts<S> for Writer {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        if let Some(identity) = parts.extensions.get::<CallerIdentity>() {
            return Ok(Writer::Session(identity.clone()));
        }
        if parts.extensions.get::<BuildServiceIdentity>().is_some() {
            return Ok(Writer::BuildService);
        }
        Err((StatusCode::UNAUTHORIZED, "not authenticated"))
    }
}

/// The repository component an image name is pushed under, as a SQL
/// expression over the text expression `$name`.
///
/// It must equal `nasiko_server::agents::image_name_slug` (lowercase, spaces
/// to `-`, drop everything outside `[a-z0-9_-]`, trim `-`, empty → `agent`):
/// agents push to `nasiko/<slug of name>` and uploaded MCP connectors to
/// `nasiko/<slug of "mcp-" + name>`. The server's `oci_repo_ownership` test
/// compares the two over sample names, so a change to one fails it.
fn repo_slug_sql(name: &str) -> String {
    format!(
        "COALESCE(NULLIF(btrim(regexp_replace(replace(lower({name}), ' ', '-'), \
         '[^a-z0-9_-]', '', 'g'), '-'), ''), 'agent')"
    )
}

/// Every owner whose agent or uploaded MCP connector pushes to `repo`, as
/// `(does every one of them equal the caller, is there any)`.
///
/// The repository is named after the agent or connector name, not its id
/// (see [`repo_slug_sql`]), and names are unique only per owner, so several
/// owners can map to one repository. The caller owns it only when *all* of
/// them are the caller: a repository two owners share cannot say whose image
/// a tag holds, so neither may write it (NAS-1112). Comparing the real
/// repository name matters too: an exact `agents.name = repo` match missed
/// every name with uppercase or spaces and every connector, and an unmatched
/// repository counted as unclaimed, which anyone may push to.
async fn repo_claim_status(
    state: &OciState,
    caller: &CallerIdentity,
    repo: &str,
) -> Result<(bool, bool)> {
    let Ok(caller_uuid) = caller.user_id.parse::<uuid::Uuid>() else {
        return Ok((false, false));
    };
    let query = format!(
        r#"WITH claimants AS (
             SELECT owner_id FROM agents
              WHERE deleted_at IS NULL AND {agent} = $1
             UNION ALL
             SELECT owner_id FROM mcp_connectors
              WHERE source_kind = 'uploaded_build' AND {connector} = $1
           )
           SELECT COALESCE(bool_and(owner_id IS NOT DISTINCT FROM $2), false) AS owns,
                  EXISTS(SELECT 1 FROM claimants) AS claimed
             FROM claimants"#,
        agent = repo_slug_sql("name"),
        connector = repo_slug_sql("'mcp-' || name"),
    );
    sqlx::query_as(&query)
        .bind(repo)
        .bind(caller_uuid)
        .fetch_one(&state.pool)
        .await
        .map_err(OciError::Database)
}

/// Enforce that `caller` may operate on `repo` (the agent-name path segment).
///
/// Policy: superusers may always proceed. Otherwise, if any agent or uploaded
/// MCP connector already pushes to this repository, the caller must own every
/// one of them (see [`repo_claim_status`]). If none does yet, the request is
/// allowed through — the CLI pushes an image
/// *before* registering the agent (`oss/cli/src/commands/push.rs`), so a
/// brand-new repo name has no owner to check against yet. This means a
/// not-yet-registered name is first-come-first-served until an agent claims
/// it; closing that narrower race is tracked separately (see MIGRATION plan,
/// Phase A3 follow-up) and is a materially smaller blast radius than the
/// "any authenticated stranger can read/delete any existing agent's image"
/// gap this closes.
///
/// Use [`check_repo_delete_access`] instead for destructive operations.
pub async fn check_repo_access(
    state: &OciState,
    caller: &CallerIdentity,
    repo: &str,
) -> Result<()> {
    if caller.is_superuser {
        return Ok(());
    }
    let (owns, claimed) = repo_claim_status(state, caller, repo).await?;
    if owns || !claimed {
        Ok(())
    } else {
        Err(OciError::Forbidden(format!(
            "not permitted to access repository '{repo}'"
        )))
    }
}

/// An agent name may carry capitals but an OCI repository name may not, so the
/// image is addressed by the lowercased name and the two compare case-insensitively.
fn repo_names_match(agent_name: &str, repo: &str) -> bool {
    agent_name.eq_ignore_ascii_case(repo)
}

/// Enforce that `caller` may read `repo` — the [`Caller`]-accepting
/// counterpart of [`check_repo_access`], used by the read-only manifest/blob/
/// tags handlers. A [`Caller::Session`] goes through the exact same policy
/// as `check_repo_access`; a [`Caller::PullOnly`] is granted access only if
/// `repo` is literally the current name of the agent its credential is
/// bound to — no ownership/superuser concept applies, since the credential
/// is scoped to one agent by construction, not one user.
pub async fn check_pull_access(state: &OciState, caller: &Caller, repo: &str) -> Result<()> {
    match caller {
        Caller::Session(identity) => check_repo_access(state, identity, repo).await,
        Caller::PullOnly(pull) => {
            // Check agents first, then fall back to MCP connectors — the
            // credential's `agent_id` column is effectively a workload_id
            // that can reference either table.
            // Compared as the repository the workload's image is pushed to
            // ([`repo_slug_sql`]), not its display name: an agent named
            // "General Assistant" pulls from `nasiko/general-assistant`.
            let bound_repo: Option<String> = sqlx::query_scalar(&format!(
                "SELECT {} FROM agents WHERE id = $1 AND deleted_at IS NULL",
                repo_slug_sql("name")
            ))
            .bind(pull.agent_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(OciError::Database)?;
            let bound_repo = match bound_repo {
                Some(name) => Some(name),
                None => sqlx::query_scalar(&format!(
                    "SELECT {} FROM mcp_connectors WHERE id = $1",
                    repo_slug_sql("'mcp-' || name")
                ))
                .bind(pull.agent_id)
                .fetch_optional(&state.pool)
                .await
                .map_err(OciError::Database)?,
            };
            if bound_repo
                .as_deref()
                .is_some_and(|bound| repo_names_match(bound, repo))
            {
                Ok(())
            } else {
                Err(OciError::Forbidden(format!(
                    "not permitted to access repository '{repo}'"
                )))
            }
        }
        // Trusted, server-orchestrated infrastructure — no per-repo restriction.
        Caller::BuildService => Ok(()),
    }
}

/// Enforce that `writer` may push to `repo` — the [`Writer`]-accepting
/// counterpart of [`check_repo_access`], used by the blob-upload and
/// manifest-put handlers. A [`Writer::Session`] goes through the exact same
/// policy as `check_repo_access` (zero change to the existing JWT-based
/// interactive/CLI push path); `Writer::BuildService` is unrestricted, same
/// reasoning as [`Caller::BuildService`] above.
pub async fn check_write_access(state: &OciState, writer: &Writer, repo: &str) -> Result<()> {
    match writer {
        Writer::Session(identity) => check_repo_access(state, identity, repo).await,
        Writer::BuildService => Ok(()),
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
pub async fn check_repo_delete_access(
    state: &OciState,
    caller: &CallerIdentity,
    repo: &str,
) -> Result<()> {
    if caller.is_superuser {
        return Ok(());
    }
    let (owns, _claimed) = repo_claim_status(state, caller, repo).await?;
    if owns {
        Ok(())
    } else {
        Err(OciError::Forbidden(format!(
            "not permitted to delete from repository '{repo}'"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::repo_names_match;

    #[test]
    fn a_capitalised_agent_name_matches_its_lowercase_repository() {
        assert!(repo_names_match("Calendar-Agent", "calendar-agent"));
        assert!(repo_names_match("translator", "translator"));
    }

    #[test]
    fn a_different_name_never_matches() {
        assert!(!repo_names_match("Calendar-Agent", "email-agent"));
        assert!(!repo_names_match("calendar-agent-1", "calendar-agent"));
    }
}
