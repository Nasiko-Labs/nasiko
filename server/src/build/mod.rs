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

pub fn degradable_router() -> Router<AppState> {
    routes::degradable_router()
}

/// Create a tar archive from a directory (for passing to ContainerRuntime::build).
///
/// Symlinks are archived as symlinks, never followed (NAS-1115). `dir` holds a
/// user's source, and a repo acquired by `git clone` keeps its symlinks: the
/// `tar` crate follows them by default, so `leak -> /var/run/secrets/...` put
/// the server's own file (or a whole directory) into the build context, where a
/// `COPY leak /leak` baked it into the user's image. Kept as a link, it is
/// resolved by the image builder inside the context, which holds nothing of
/// the server's; a link within the repo (`README -> docs/README.md`) still works.
pub fn tar_directory(dir: &std::path::Path) -> Result<Vec<u8>, String> {
    use tar::Builder;

    let buf = Vec::new();
    let mut archive = Builder::new(buf);
    archive.follow_symlinks(false);
    // Normalize modes to 0644 (0755 for dirs and executables). Uploaded sources are
    // extracted under the server's umask (often 0600), and a root-owned 0600 file is
    // unreadable to an image that switches to a non-root `USER`.
    archive.mode(tar::HeaderMode::Deterministic);
    archive
        .append_dir_all(".", dir)
        .map_err(|e| format!("tar append_dir_all: {e}"))?;
    archive
        .into_inner()
        .map_err(|e| format!("tar finalize: {e}"))
}

/// Decompress and unpack a gzipped tar archive into `dest`.
///
/// Hardened like the zip path (`routes::extract_zip_reader`): link entries
/// (symlinks / hardlinks) are rejected outright — they are the classic tar
/// traversal/escape vector — and every regular entry's path is validated to be
/// relative, `..`-free, and contained within `dest` before it is written. File
/// count and uncompressed size are capped to bound tar-bomb / disk-exhaustion.
pub fn extract_tar_gzip(data: &[u8], dest: &std::path::Path) -> Result<(), String> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    const MAX_TAR_FILES: usize = 1_000;
    const MAX_TAR_UNCOMPRESSED: u64 = 200 * 1024 * 1024; // 200 MiB

    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let decoder = GzDecoder::new(std::io::Cursor::new(data));
    let mut archive = Archive::new(decoder);

    let mut count: usize = 0;
    let mut uncompressed_total: u64 = 0;

    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let etype = entry.header().entry_type();

        // Reject link entries — extracting them would let an archive plant a
        // symlink/hardlink that escapes `dest` on a later write.
        if etype.is_symlink() || etype.is_hard_link() {
            let p = entry.path().map(|p| p.into_owned()).unwrap_or_default();
            return Err(format!(
                "tar contains a link entry which is not allowed: {}",
                p.display()
            ));
        }

        count += 1;
        if count > MAX_TAR_FILES {
            return Err(format!("tar contains more than {MAX_TAR_FILES} entries"));
        }

        // Path-traversal guard: reject absolute paths and any `..` component,
        // then confirm the joined path stays inside `dest`.
        let rel = entry.path().map_err(|e| e.to_string())?.into_owned();
        if rel.is_absolute()
            || rel
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(format!("tar traversal attempt: {}", rel.display()));
        }
        let out_path = dest.join(&rel);
        if !out_path.starts_with(dest) {
            return Err(format!(
                "tar traversal attempt (join escaped dest): {}",
                rel.display()
            ));
        }

        if etype.is_dir() {
            std::fs::create_dir_all(&out_path).map_err(|e| e.to_string())?;
            continue;
        }

        // Tar-bomb guard: check declared size before writing.
        uncompressed_total = uncompressed_total.saturating_add(entry.header().size().unwrap_or(0));
        if uncompressed_total > MAX_TAR_UNCOMPRESSED {
            return Err(format!(
                "tar uncompressed size exceeds {MAX_TAR_UNCOMPRESSED} bytes — possible tar bomb"
            ));
        }

        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = std::fs::File::create(&out_path).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
    }
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
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_')
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
mod tar_directory_tests {
    //! NAS-1115: a cloned repo is a user's directory, and it may hold symlinks
    //! to anything on the server's filesystem. The build context must carry
    //! the link, never the file it points at.
    use super::tar_directory;
    use std::path::{Path, PathBuf};

    /// A fresh directory under the system temp dir, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("nasiko-tar-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// `(path, is a symlink, link target or file contents)` for each entry.
    fn entries(tar: &[u8]) -> Vec<(String, bool, String)> {
        let mut archive = tar::Archive::new(std::io::Cursor::new(tar));
        archive
            .entries()
            .unwrap()
            .map(|entry| {
                let mut entry = entry.unwrap();
                let path = entry.path().unwrap().display().to_string();
                let is_link = entry.header().entry_type().is_symlink();
                let detail = if is_link {
                    entry.link_name().unwrap().unwrap().display().to_string()
                } else {
                    let mut contents = String::new();
                    std::io::Read::read_to_string(&mut entry, &mut contents).unwrap_or_default();
                    contents
                };
                (path, is_link, detail)
            })
            .collect()
    }

    fn symlink(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[test]
    fn a_symlink_out_of_the_context_is_archived_as_a_link_not_as_the_host_file() {
        let scratch = Scratch::new();
        let outside = scratch.0.join("outside");
        let context = scratch.0.join("context");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(&context).unwrap();
        std::fs::write(outside.join("marker.txt"), "host-secret").unwrap();
        std::fs::write(
            context.join("Dockerfile"),
            "FROM scratch\nCOPY leak /leak\n",
        )
        .unwrap();
        symlink(&outside.join("marker.txt"), &context.join("leak"));
        symlink(&outside, &context.join("leak-dir"));

        let tar = tar_directory(&context).unwrap();
        let entries = entries(&tar);

        assert!(
            entries
                .iter()
                .all(|(_, _, detail)| !detail.contains("host-secret")),
            "a host file's contents reached the build context: {entries:?}"
        );
        let leak = entries
            .iter()
            .find(|(path, ..)| path.ends_with("leak"))
            .unwrap();
        assert!(leak.1, "`leak` must stay a symlink: {leak:?}");
    }

    #[test]
    fn owner_only_files_are_world_readable_in_the_context() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new();
        let file = scratch.0.join("__main__.py");
        std::fs::write(&file, "print(1)").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();

        let tar = tar_directory(&scratch.0).unwrap();
        let mut archive = tar::Archive::new(std::io::Cursor::new(tar));
        let mode: u32 = archive
            .entries()
            .unwrap()
            .map(|e| e.unwrap())
            .find(|e| e.path().unwrap().ends_with("__main__.py"))
            .map(|e| e.header().mode().unwrap())
            .unwrap();
        assert_eq!(
            mode & 0o044,
            0o044,
            "mode {mode:o} not group/other readable"
        );
    }

    #[test]
    fn a_symlink_inside_the_context_still_works() {
        let scratch = Scratch::new();
        std::fs::write(scratch.0.join("README.md"), "docs").unwrap();
        symlink(Path::new("README.md"), &scratch.0.join("README"));

        let entries = entries(&tar_directory(&scratch.0).unwrap());

        let readme = entries
            .iter()
            .find(|(path, ..)| path.ends_with("/README") || path == "README")
            .unwrap();
        assert_eq!((readme.1, readme.2.as_str()), (true, "README.md"));
    }
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
