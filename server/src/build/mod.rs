pub mod routes;

use axum::Router;
use bytes::BufMut;
use futures::StreamExt;
use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// Mirrors the Postgres `build_status` enum (migration 010 §11). Deriving
/// `sqlx::Type` lets sqlx encode/decode it directly instead of treating the
/// column as TEXT; serde keeps the JSON wire shape identical to the old TEXT
/// column ("queued"/"building"/"success"/"failed") so the UI/CLI are unaffected.
/// Shared by `build::routes` and `agents::routes` so both bind the column the
/// same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "build_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum BuildStatus {
    Queued,
    Building,
    Success,
    Failed,
}

pub fn router() -> Router<AppState> {
    routes::router()
}

/// Create a tar archive from a directory (for passing to ContainerRuntime::build).
pub fn tar_directory(dir: &std::path::Path) -> Result<Vec<u8>, String> {
    use tar::Builder;

    let buf = Vec::new();
    let mut archive = Builder::new(buf);
    archive
        .append_dir_all(".", dir)
        .map_err(|e| format!("tar append_dir_all: {e}"))?;
    archive
        .into_inner()
        .map_err(|e| format!("tar finalize: {e}"))
}

/// Decompress and unpack a gzipped tar archive into `dest`.
pub fn extract_tar_gzip(data: &[u8], dest: &std::path::Path) -> Result<(), String> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let decoder = GzDecoder::new(std::io::Cursor::new(data));
    let mut archive = Archive::new(decoder);
    archive.unpack(dest).map_err(|e| e.to_string())?;
    Ok(())
}

/// Validate an `owner/repo` string: both segments must be non-empty, ≤100 chars,
/// and contain only ASCII alphanumerics, hyphens, dots, or underscores.
/// Also strips a trailing `.git` suffix before checking, so stored URLs like
/// `https://github.com/owner/repo.git` round-trip cleanly.
pub(crate) fn is_valid_repo_name(repo: &str) -> bool {
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let Some((owner, name)) = repo.split_once('/') else {
        return false;
    };
    let is_safe_segment = |s: &str| {
        !s.is_empty()
            && s.len() <= 100
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_')
    };
    is_safe_segment(owner) && is_safe_segment(name)
}

/// Download a GitHub repository tarball (gzipped tar). `full_repo` is "owner/repo".
/// Token is an already-decrypted OAuth access token. Enforces a 100 MB size cap.
pub(crate) async fn download_repo_tarball(
    client: &reqwest::Client,
    token: &str,
    full_repo: &str,
) -> Result<bytes::Bytes, String> {
    const MAX_TARBALL_BYTES: usize = 100 * 1024 * 1024;

    let url = format!("https://api.github.com/repos/{full_repo}/tarball/HEAD");
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", "nasiko-cp")
        .send()
        .await
        .map_err(|e| format!("github request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("github returned HTTP {}", response.status()));
    }

    let mut buf = bytes::BytesMut::with_capacity(64 * 1024);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("stream error: {e}"))?;
        if buf.len() + chunk.len() > MAX_TARBALL_BYTES {
            return Err("repository archive exceeds 100 MB limit".into());
        }
        buf.put(chunk);
    }
    Ok(buf.freeze())
}

#[cfg(test)]
mod tests {
    use super::is_valid_repo_name;

    #[test]
    fn valid_repo_names_accepted() {
        assert!(is_valid_repo_name("owner/repo"));
        assert!(is_valid_repo_name("my-org/my-repo.v2"));
        assert!(is_valid_repo_name("user_123/agent_sdk"));
        assert!(is_valid_repo_name("owner/repo.git")); // .git suffix stripped
    }

    #[test]
    fn invalid_repo_names_rejected() {
        assert!(!is_valid_repo_name("no-slash"));
        assert!(!is_valid_repo_name("../etc/passwd"));
        assert!(!is_valid_repo_name("owner/repo/extra"));
        assert!(!is_valid_repo_name("owner/ repo"));
        assert!(!is_valid_repo_name("owner/"));
        assert!(!is_valid_repo_name("/repo"));
        assert!(!is_valid_repo_name("owner/repo;rm -rf /"));
        assert!(!is_valid_repo_name(&format!("a/{}", "x".repeat(101))));
    }
}
