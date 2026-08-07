use sqlx::PgPool;
use uuid::Uuid;

/// Error from [`record_version_change`]. Lets callers tell a caller mistake
/// (bad or reused version, → 409/400) apart from a real DB failure (→ 500).
#[derive(Debug)]
pub enum VersionChangeError {
    InvalidVersion(String),
    VersionAlreadyExists(String),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for VersionChangeError {
    fn from(e: sqlx::Error) -> Self {
        VersionChangeError::Db(e)
    }
}

impl std::fmt::Display for VersionChangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VersionChangeError::InvalidVersion(v) => {
                write!(f, "version {v} must be in x.y.z format (e.g. 1.2.3)")
            }
            VersionChangeError::VersionAlreadyExists(v) => {
                write!(f, "version {v} already exists for this agent")
            }
            VersionChangeError::Db(e) => write!(f, "{e}"),
        }
    }
}

/// Same rule the CLI uses (`nasiko::version_prompt`), shared via
/// `nasiko_utils` so both sides can't drift apart.
pub use nasiko_utils::version::parse_plain_version;

/// Everything needed to record one version change.
pub struct VersionChange<'a> {
    pub agent_id: Uuid,
    /// `None` when there was no build (e.g. an already-built image pushed
    /// directly via `nasiko deploy`).
    pub build_id: Option<Uuid>,
    /// Must already be a decided version — this function never invents one.
    pub version: &'a str,
    pub image_tag: &'a str,
    pub changelog: Option<&'a str>,
    /// If `true`, replace an already-used version's content instead of
    /// rejecting it. Only set after explicit user consent (a confirm prompt
    /// or `--overwrite`) — never on by default.
    pub allow_overwrite: bool,
}

/// The one place that writes an agent's version history (`agent_versions`).
///
/// Archives the current active version, inserts the new one as active, and
/// marks the old one rollback-eligible — this is what `nasiko rollback`
/// depends on to find a target.
///
/// Rejects a version that's already been used for this agent (that's how
/// history used to collapse: everyone defaulting to `"latest"` and
/// overwriting the same row), unless `allow_overwrite` is set. Only accepts
/// a plain `x.y.z` version — no `"latest"`, no free-form text.
pub async fn record_version_change(
    db: &PgPool,
    change: VersionChange<'_>,
) -> Result<(), VersionChangeError> {
    let VersionChange {
        agent_id,
        build_id,
        version: new_version,
        image_tag,
        changelog,
        allow_overwrite,
    } = change;

    if parse_plain_version(new_version).is_none() {
        return Err(VersionChangeError::InvalidVersion(new_version.to_string()));
    }

    let mut tx = db.begin().await?;

    let version_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM agent_versions WHERE agent_id = $1 AND version = $2)",
    )
    .bind(agent_id)
    .bind(new_version)
    .fetch_one(&mut *tx)
    .await?;
    if version_exists && !allow_overwrite {
        return Err(VersionChangeError::VersionAlreadyExists(
            new_version.to_string(),
        ));
    }

    let prev_version: Option<String> = sqlx::query_scalar(
        "SELECT version FROM agent_versions WHERE agent_id = $1 AND is_active = true",
    )
    .bind(agent_id)
    .fetch_optional(&mut *tx)
    .await?;

    // Overwriting the version that's already active: just refresh its
    // content, no archiving or rollback pointer needed.
    if version_exists && prev_version.as_deref() == Some(new_version) {
        sqlx::query(
            "UPDATE agent_versions SET build_id = $2, image_tag = $3, changelog = $4, \
             created_at = now() WHERE agent_id = $1 AND version = $5",
        )
        .bind(agent_id)
        .bind(build_id)
        .bind(image_tag)
        .bind(changelog)
        .bind(new_version)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(());
    }

    sqlx::query(
        "UPDATE agent_versions SET is_active = false, status = 'archived' \
         WHERE agent_id = $1 AND is_active = true",
    )
    .bind(agent_id)
    .execute(&mut *tx)
    .await?;

    if version_exists {
        // Overwrite: reuse and reactivate the old archived row.
        sqlx::query(
            "UPDATE agent_versions SET build_id = $2, image_tag = $3, changelog = $4, \
             is_active = true, can_rollback = false, status = 'active', \
             previous_version = $5, created_at = now() \
             WHERE agent_id = $1 AND version = $6",
        )
        .bind(agent_id)
        .bind(build_id)
        .bind(image_tag)
        .bind(changelog)
        .bind(&prev_version)
        .bind(new_version)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query(
            "INSERT INTO agent_versions \
               (agent_id, build_id, version, image_tag, changelog, is_active, can_rollback, previous_version, status) \
             VALUES ($1, $2, $3, $4, $5, true, false, $6, 'active')",
        )
        .bind(agent_id)
        .bind(build_id)
        .bind(new_version)
        .bind(image_tag)
        .bind(changelog)
        .bind(&prev_version)
        .execute(&mut *tx)
        .await?;
    }

    if let Some(ref pv) = prev_version {
        sqlx::query(
            "UPDATE agent_versions SET can_rollback = true WHERE agent_id = $1 AND version = $2",
        )
        .bind(agent_id)
        .bind(pv)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_already_exists_display() {
        let e = VersionChangeError::VersionAlreadyExists("1.2.3".to_string());
        assert_eq!(e.to_string(), "version 1.2.3 already exists for this agent");
    }

    #[test]
    fn invalid_version_display() {
        let e = VersionChangeError::InvalidVersion("latest".to_string());
        assert!(e.to_string().contains("x.y.z format"));
    }
}
