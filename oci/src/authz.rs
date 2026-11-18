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
use nasiko_utils::image::strip_image_to_repo;

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

/// Parse an OCI repository path into namespace type and slug.
///
/// Returns `Ok(Owner(uuid, slug))` for `u-{32 lowercase hex}/{slug}`,
/// `Ok(Legacy(slug))` for `nasiko/{slug}`, or an error for malformed paths.
fn parse_repository_path(repo: &str) -> Result<RepoNamespace> {
    if let Some(rest) = repo.strip_prefix("u-") {
        let parts: Vec<&str> = rest.splitn(2, '/').collect();
        if parts.len() != 2 {
            return Err(OciError::BadRequest(format!(
                "invalid owner namespace: expected u-{{uuid}}/{{name}}, got '{}'",
                repo
            )));
        }
        let hex = parts[0];
        if hex.len() != 32
            || !hex
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            return Err(OciError::BadRequest(format!(
                "invalid owner namespace: uuid must be 32 lowercase hex chars, got '{}'",
                hex
            )));
        }
        let uuid = uuid::Uuid::parse_str(hex).map_err(|_| {
            OciError::BadRequest(format!(
                "invalid owner namespace: '{}' is not a valid UUID",
                hex
            ))
        })?;
        Ok(RepoNamespace::Owner(uuid, parts[1].to_string()))
    } else if let Some(slug) = repo.strip_prefix("nasiko/") {
        Ok(RepoNamespace::Legacy(slug.to_string()))
    } else {
        Err(OciError::BadRequest(format!(
            "unknown namespace in repository path '{}'",
            repo
        )))
    }
}

/// Repository namespace type.
#[derive(Debug, Clone)]
enum RepoNamespace {
    /// Per-owner namespace: `u-{uuid}/{slug}`
    Owner(uuid::Uuid, String),
    /// Legacy namespace: `nasiko/{slug}`
    Legacy(String),
}

/// Compute owners(R) - the set of owner_ids of live agents whose stored
/// `agents.image` (with registry host and tag stripped) equals repository R,
/// and of uploaded MCP connectors whose successful build image is R.
/// Narrows candidates in SQL via substring search, then confirms exact match
/// in Rust to avoid LIKE wildcard issues with `_` in names.
async fn owners_of_repository(state: &OciState, repo: &str) -> Result<Vec<uuid::Uuid>> {
    // Narrow in SQL: image must contain the repo path as a substring.
    // strpos returns >0 when found (1-indexed), 0 when not found.
    // This filters out most rows before pulling into Rust, without false
    // positives from LIKE wildcards treating `_` as single-char match.
    let rows: Vec<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT owner_id, image FROM agents WHERE deleted_at IS NULL AND strpos(image, $1) > 0",
    )
    .bind(repo)
    .fetch_all(&state.pool)
    .await
    .map_err(OciError::Database)?;

    let mut owner_ids = Vec::new();
    for (owner_id, image) in rows {
        // An external image with the same path is not an owner of the platform repository.
        if let Some(platform_repo) = nasiko_utils::image::platform_repository_from_image(
            &image,
            &state.platform_registry_host,
        ) && platform_repo == repo
            && !owner_ids.contains(&owner_id)
        {
            owner_ids.push(owner_id);
        }
    }

    let connector_rows: Vec<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT c.owner_id, b.image_tag \
         FROM mcp_connectors c \
         JOIN mcp_connector_builds b ON b.connector_id = c.id \
         WHERE b.status = 'success' AND b.image_tag IS NOT NULL AND strpos(b.image_tag, $1) > 0",
    )
    .bind(repo)
    .fetch_all(&state.pool)
    .await
    .map_err(OciError::Database)?;

    for (owner_id, image_tag) in connector_rows {
        if let Some(platform_repo) = nasiko_utils::image::platform_repository_from_image(
            &image_tag,
            &state.platform_registry_host,
        ) && platform_repo == repo
            && !owner_ids.contains(&owner_id)
        {
            owner_ids.push(owner_id);
        }
    }

    Ok(owner_ids)
}

/// Check write access - sessions checked via their namespace, BuildService unrestricted.
/// OSS has no agent grants, so ownership is the only check.
pub async fn check_repo_access(
    state: &OciState,
    caller: &CallerIdentity,
    repo: &str,
) -> Result<()> {
    // Kept for back-compat but delegates to write-access logic
    check_write_access(state, &Writer::Session(caller.clone()), repo).await
}

/// An agent name may carry capitals but an OCI repository name may not, so the
/// image is addressed by the lowercased name and the two compare case-insensitively.
fn repo_names_match(agent_name: &str, repo: &str) -> bool {
    agent_name.eq_ignore_ascii_case(repo)
}

/// Enforce read access per namespace (coordinator rules 2, 4, 5).
pub async fn check_pull_access(state: &OciState, caller: &Caller, repo: &str) -> Result<()> {
    match caller {
        Caller::BuildService => Ok(()),
        Caller::Session(identity) => {
            if identity.is_superuser {
                return Ok(());
            }
            let caller_uuid = identity
                .user_id
                .parse::<uuid::Uuid>()
                .map_err(|_| OciError::Forbidden("invalid user id".to_string()))?;

            match parse_repository_path(repo)? {
                RepoNamespace::Owner(uuid, _slug) => {
                    if uuid == caller_uuid {
                        Ok(())
                    } else {
                        Err(OciError::Forbidden(format!(
                            "not permitted to access repository '{}'",
                            repo
                        )))
                    }
                }
                RepoNamespace::Legacy(_slug) => {
                    let owners = owners_of_repository(state, repo).await?;
                    if owners.is_empty() || (owners.len() == 1 && owners[0] == caller_uuid) {
                        Ok(())
                    } else {
                        Err(OciError::Forbidden(format!(
                            "not permitted to access repository '{}'",
                            repo
                        )))
                    }
                }
            }
        }
        Caller::PullOnly(pull) => {
            let bound_image: Option<String> =
                sqlx::query_scalar("SELECT image FROM agents WHERE id = $1 AND deleted_at IS NULL")
                    .bind(pull.agent_id)
                    .fetch_optional(&state.pool)
                    .await
                    .map_err(OciError::Database)?;

            let Some(image_ref) = bound_image.as_deref() else {
                // Connector: its first pull happens before the build is marked successful, so
                // pending and building builds count too.
                let connector_data: Option<(uuid::Uuid, Option<String>)> = sqlx::query_as(
                    "SELECT c.owner_id, b.image_tag \
                     FROM mcp_connectors c \
                     LEFT JOIN mcp_connector_builds b \
                       ON b.connector_id = c.id \
                       AND b.status IN ('pending', 'building', 'success') \
                       AND b.image_tag IS NOT NULL \
                     WHERE c.id = $1 \
                     ORDER BY b.created_at DESC \
                     LIMIT 1",
                )
                .bind(pull.agent_id)
                .fetch_optional(&state.pool)
                .await
                .map_err(OciError::Database)?;

                if let Some((owner_id, Some(image))) = connector_data {
                    let bound_repo = strip_image_to_repo(&image);
                    // Check the repository matches
                    if bound_repo != repo {
                        return Err(OciError::Forbidden(format!(
                            "not permitted to access repository '{}' (bound to '{}')",
                            repo, bound_repo
                        )));
                    }
                    // Check the namespace matches the connector's owner
                    match nasiko_utils::image::parse_owner_namespace(repo) {
                        Ok(Some((ns_owner, _))) => {
                            if ns_owner != owner_id {
                                return Err(OciError::Forbidden(
                                    "repository namespace does not match connector owner"
                                        .to_string(),
                                ));
                            }
                        }
                        Ok(None) => {
                            // Legacy nasiko/ namespace - allow for backward compat
                        }
                        Err(_) => {
                            return Err(OciError::Forbidden(format!(
                                "invalid namespace in repository '{}'",
                                repo
                            )));
                        }
                    }
                    return Ok(());
                } else {
                    return Err(OciError::Forbidden(
                        "no build found for connector or connector not found".to_string(),
                    ));
                }
            };

            // Agent case: the bound image must be this platform repository, in the owner's namespace.
            let bound_repo = nasiko_utils::image::platform_repository_from_image(
                image_ref,
                &state.platform_registry_host,
            );
            if !bound_repo.is_some_and(|bound| repo_names_match(bound, repo)) {
                return Err(OciError::Forbidden(format!(
                    "not permitted to access repository '{}'",
                    repo
                )));
            }

            // Also verify the namespace matches the agent's owner
            let agent_owner: uuid::Uuid =
                sqlx::query_scalar("SELECT owner_id FROM agents WHERE id = $1")
                    .bind(pull.agent_id)
                    .fetch_one(&state.pool)
                    .await
                    .map_err(OciError::Database)?;

            match nasiko_utils::image::parse_owner_namespace(repo) {
                Ok(Some((ns_owner, _))) => {
                    if ns_owner != agent_owner {
                        return Err(OciError::Forbidden(
                            "repository namespace does not match agent owner".to_string(),
                        ));
                    }
                }
                Ok(None) => {
                    // Legacy nasiko/ namespace - allow for backward compat
                }
                Err(_) => {
                    return Err(OciError::Forbidden(format!(
                        "invalid namespace in repository '{}'",
                        repo
                    )));
                }
            }
            Ok(())
        }
    }
}

/// Enforce write access per namespace (coordinator rules 3, 4).
pub async fn check_write_access(state: &OciState, writer: &Writer, repo: &str) -> Result<()> {
    match writer {
        Writer::BuildService => Ok(()),
        Writer::Session(identity) => {
            if identity.is_superuser {
                return Ok(());
            }
            let caller_uuid = identity
                .user_id
                .parse::<uuid::Uuid>()
                .map_err(|_| OciError::Forbidden("invalid user id".to_string()))?;

            match parse_repository_path(repo)? {
                RepoNamespace::Owner(uuid, _slug) => {
                    if uuid == caller_uuid {
                        Ok(())
                    } else {
                        Err(OciError::Forbidden(format!(
                            "not permitted to write to repository '{}'",
                            repo
                        )))
                    }
                }
                RepoNamespace::Legacy(_slug) => {
                    let owners = owners_of_repository(state, repo).await?;
                    if owners.is_empty() {
                        Err(OciError::Forbidden(format!(
                            "legacy repository '{}' is unclaimed - use your own namespace instead",
                            repo
                        )))
                    } else if owners.len() == 1 && owners[0] == caller_uuid {
                        Ok(())
                    } else {
                        Err(OciError::Forbidden(format!(
                            "not permitted to write to repository '{}'",
                            repo
                        )))
                    }
                }
            }
        }
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
    let caller_uuid = caller
        .user_id
        .parse::<uuid::Uuid>()
        .map_err(|_| OciError::Forbidden("invalid user id".to_string()))?;

    match parse_repository_path(repo)? {
        RepoNamespace::Owner(uuid, _) => {
            if uuid == caller_uuid {
                Ok(())
            } else {
                Err(OciError::Forbidden(format!(
                    "not permitted to delete from repository '{}'",
                    repo
                )))
            }
        }
        RepoNamespace::Legacy(_) => {
            let owners = owners_of_repository(state, repo).await?;
            if owners.len() == 1 && owners[0] == caller_uuid {
                Ok(())
            } else {
                Err(OciError::Forbidden(format!(
                    "not permitted to delete from repository '{}'",
                    repo
                )))
            }
        }
    }
}

#[cfg(test)]
mod namespace_tests {
    use super::*;

    #[test]
    fn parse_owner_namespace_valid() {
        let uuid_str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4";
        let repo = format!("u-{}/my-agent", uuid_str);
        let result = parse_repository_path(&repo);
        assert!(result.is_ok());
        match result.unwrap() {
            RepoNamespace::Owner(uuid, slug) => {
                assert_eq!(uuid.as_simple().to_string(), uuid_str);
                assert_eq!(slug, "my-agent");
            }
            _ => panic!("expected Owner namespace"),
        }
    }

    #[test]
    fn parse_owner_namespace_uppercase_rejected() {
        let repo = "u-A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4/agent";
        assert!(parse_repository_path(repo).is_err());
    }

    #[test]
    fn parse_owner_namespace_wrong_length() {
        let repo = "u-short/agent";
        assert!(parse_repository_path(repo).is_err());
    }

    #[test]
    fn parse_legacy_namespace() {
        let repo = "nasiko/code-reviewer";
        let result = parse_repository_path(repo);
        assert!(result.is_ok());
        match result.unwrap() {
            RepoNamespace::Legacy(slug) => {
                assert_eq!(slug, "code-reviewer");
            }
            _ => panic!("expected Legacy namespace"),
        }
    }

    #[test]
    fn parse_unknown_namespace() {
        let repo = "unknown/agent";
        assert!(parse_repository_path(repo).is_err());
    }

    #[test]
    fn strip_image_to_repo_bare() {
        assert_eq!(strip_image_to_repo("u-abc123/agent:1.0"), "u-abc123/agent");
    }

    #[test]
    fn strip_image_to_repo_with_registry() {
        assert_eq!(
            strip_image_to_repo("registry.io/u-abc123/agent:1.0"),
            "u-abc123/agent"
        );
    }

    #[test]
    fn strip_image_to_repo_legacy() {
        assert_eq!(
            strip_image_to_repo("nasiko/code-reviewer:latest"),
            "nasiko/code-reviewer"
        );
    }

    #[test]
    fn strip_image_to_repo_no_tag() {
        assert_eq!(strip_image_to_repo("u-abc123/agent"), "u-abc123/agent");
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
