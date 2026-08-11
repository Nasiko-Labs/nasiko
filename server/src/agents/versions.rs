use sqlx::{PgPool, Postgres, Transaction};
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

/// Validates the version, locks the agent row, and reports whether (and
/// under what status) this version already has a row — shared by
/// [`record_version_change_in_tx`] and [`record_pushed_version_in_tx`], the
/// two ways a version change gets recorded.
///
/// The row lock serializes concurrent version changes for this agent —
/// without it, two concurrent requests could both read the same "current
/// active version" and interleave their archive/insert writes, leaving more
/// than one row with `is_active = true` (there's no DB constraint preventing
/// it).
async fn lock_agent_and_check_existing_version(
    tx: &mut Transaction<'_, Postgres>,
    agent_id: Uuid,
    new_version: &str,
) -> Result<Option<String>, VersionChangeError> {
    if parse_plain_version(new_version).is_none() {
        return Err(VersionChangeError::InvalidVersion(new_version.to_string()));
    }

    sqlx::query("SELECT id FROM agents WHERE id = $1 FOR UPDATE")
        .bind(agent_id)
        .fetch_optional(&mut **tx)
        .await?;

    let existing_status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM agent_versions WHERE agent_id = $1 AND version = $2",
    )
    .bind(agent_id)
    .bind(new_version)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(existing_status)
}

/// The one place that activates a real deploy's version in history
/// (`update`/`upload`/`deploy`).
///
/// Archives the current active version, inserts the new one as active, and
/// marks the old one rollback-eligible — this is what `nasiko rollback`
/// depends on to find a target.
///
/// Rejects a version that's already been used for this agent (that's how
/// history used to collapse: everyone defaulting to `"latest"` and
/// overwriting the same row), unless `allow_overwrite` is set. Only accepts
/// a plain `x.y.z` version — no `"latest"`, no free-form text.
///
/// See [`record_pushed_version_in_tx`] for `nasiko push`, which registers a
/// version without deploying it.
/// Records a version change after a deploy that already succeeded, retrying
/// once before giving up and just logging it loudly. Used by every "record
/// history after the fact" call site (`update`, `upload`'s two build
/// pipelines) — the deploy already happened, so a failure here can't be
/// undone by re-validating input, only given a second chance in case it was
/// a transient DB blip.
pub async fn record_version_change_with_retry<'a>(
    db: &PgPool,
    make_change: impl Fn() -> VersionChange<'a>,
) {
    let agent_id = make_change().agent_id;
    if let Err(e) = record_version_change(db, make_change()).await {
        tracing::error!(%e, %agent_id, "record version change failed, retrying once");
        if let Err(e) = record_version_change(db, make_change()).await {
            let new_version = make_change().version.to_string();
            tracing::error!(
                %e, %agent_id, %new_version,
                "record version change failed after retry — agent is running this version \
                 but it is missing from history"
            );
        }
    }
}

pub async fn record_version_change(
    db: &PgPool,
    change: VersionChange<'_>,
) -> Result<(), VersionChangeError> {
    let mut tx = db.begin().await?;
    record_version_change_in_tx(&mut tx, change).await?;
    tx.commit().await?;
    Ok(())
}

/// Same as [`record_version_change`], but runs against a transaction the
/// caller already holds open — so this write commits atomically together
/// with whatever else the caller is doing in the same transaction (e.g. the
/// catalog `agents` row update), instead of as a separate, independently
/// committed write that can succeed while the rest of the request fails.
pub async fn record_version_change_in_tx(
    tx: &mut Transaction<'_, Postgres>,
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

    let existing_status = lock_agent_and_check_existing_version(tx, agent_id, new_version).await?;
    let version_exists = existing_status.is_some();
    // A `push`-only row (`status = "pushed"`) was never really "used" —
    // promoting it to active here is not a reuse and must not need
    // `allow_overwrite`. Re-pushing it again still is (enforced in
    // `record_pushed_version_in_tx`, which never sees this exception).
    let is_promotable_push = existing_status.as_deref() == Some("pushed");
    if version_exists && !allow_overwrite && !is_promotable_push {
        return Err(VersionChangeError::VersionAlreadyExists(
            new_version.to_string(),
        ));
    }

    let prev_version: Option<String> = sqlx::query_scalar(
        "SELECT version FROM agent_versions WHERE agent_id = $1 AND is_active = true",
    )
    .bind(agent_id)
    .fetch_optional(&mut **tx)
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
        .execute(&mut **tx)
        .await?;
        return Ok(());
    }

    sqlx::query(
        "UPDATE agent_versions SET is_active = false, status = 'archived' \
         WHERE agent_id = $1 AND is_active = true",
    )
    .bind(agent_id)
    .execute(&mut **tx)
    .await?;

    if version_exists {
        // Overwrite (or promote a `push`ed row): reuse and (re)activate it.
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
        .execute(&mut **tx)
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
        .execute(&mut **tx)
        .await?;
    }

    if let Some(ref pv) = prev_version {
        sqlx::query(
            "UPDATE agent_versions SET can_rollback = true WHERE agent_id = $1 AND version = $2",
        )
        .bind(agent_id)
        .bind(pv)
        .execute(&mut **tx)
        .await?;
    }

    Ok(())
}

/// Records a version `nasiko push` made available in the registry, without
/// deploying it. Inserted (or, on a re-push, refreshed) as inactive
/// (`status = "pushed"`) — never archiving whatever version is genuinely
/// active, and never claiming this one is live.
///
/// Rejects re-pushing an already-recorded version unless `allow_overwrite`
/// is set — same duplicate-version guard as [`record_version_change_in_tx`].
/// A later real deploy of this version promotes it via that function
/// instead: promoting a `status = "pushed"` row is not a reuse.
pub async fn record_pushed_version_in_tx(
    tx: &mut Transaction<'_, Postgres>,
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

    let existing_status = lock_agent_and_check_existing_version(tx, agent_id, new_version).await?;
    if existing_status.is_some() && !allow_overwrite {
        return Err(VersionChangeError::VersionAlreadyExists(
            new_version.to_string(),
        ));
    }

    if existing_status.is_some() {
        sqlx::query(
            "UPDATE agent_versions SET build_id = $2, image_tag = $3, changelog = $4, \
             created_at = now() WHERE agent_id = $1 AND version = $5",
        )
        .bind(agent_id)
        .bind(build_id)
        .bind(image_tag)
        .bind(changelog)
        .bind(new_version)
        .execute(&mut **tx)
        .await?;
    } else {
        sqlx::query(
            "INSERT INTO agent_versions \
               (agent_id, build_id, version, image_tag, changelog, is_active, can_rollback, status) \
             VALUES ($1, $2, $3, $4, $5, false, false, 'pushed')",
        )
        .bind(agent_id)
        .bind(build_id)
        .bind(new_version)
        .bind(image_tag)
        .bind(changelog)
        .execute(&mut **tx)
        .await?;
    }

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
