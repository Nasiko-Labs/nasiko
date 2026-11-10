//! Files a session's configuration asks the shim to put in place before the harness starts.
//!
//! A harness reads some of its settings only from disk (Codex's `config.toml`), and a sandbox
//! image is shared by every session, so the per-session settings cannot be baked in. They arrive
//! with the rest of the configuration — the `files` block of `POST /control/configure` in
//! handshake mode, the `HARNESS_FILES` environment variable (the same JSON) in env mode — and
//! are written before every spawn, so a reconfigure that changes them takes effect with the
//! restart that applies it.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// The env-mode carrier: a JSON array of [`FileSpec`]. Removed from the harness's environment.
pub const FILES_ENV: &str = "HARNESS_FILES";

/// Permissions a file gets when the spec names none: owner read/write only, since a file may
/// carry a credential.
const DEFAULT_MODE: u32 = 0o600;

/// One file to write: an absolute `path`, its `content`, and an optional unix `mode`.
#[derive(Deserialize, Clone, PartialEq, Eq)]
pub struct FileSpec {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub mode: Option<u32>,
}

/// The content may carry a credential, so only its size is shown.
impl std::fmt::Debug for FileSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileSpec")
            .field("path", &self.path)
            .field("content_bytes", &self.content.len())
            .field("mode", &self.mode.map(|m| format!("{m:o}")))
            .finish()
    }
}

/// Parses the env-mode carrier. Unset or blank means no files.
pub fn parse_env_value(raw: Option<&str>) -> Result<Vec<FileSpec>> {
    match raw.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(Vec::new()),
        Some(json) => serde_json::from_str(json).with_context(|| format!("parsing {FILES_ENV}")),
    }
}

/// Reads [`FILES_ENV`] from the process environment.
pub fn from_env() -> Result<Vec<FileSpec>> {
    parse_env_value(std::env::var(FILES_ENV).ok().as_deref())
}

/// `~` or `~/…` becomes `home`; anything else is unchanged (no `~user` support). Sessions
/// sharing a pod each have their own home, so a configuration cannot name it as a constant.
pub(crate) fn expand_home(value: &str, home: &str) -> String {
    match value.strip_prefix('~') {
        Some("") => home.to_owned(),
        Some(rest) if rest.starts_with('/') => format!("{home}{rest}"),
        _ => value.to_owned(),
    }
}

/// Writes every file, creating parent directories; a leading `~` means `home`. A relative path
/// is refused: the shim's working directory is the workspace, and a session's settings must not
/// land in a repository.
pub fn write_all(files: &[FileSpec], home: &str) -> Result<()> {
    for file in files {
        write_one(file, home).with_context(|| format!("writing {}", file.path))?;
    }
    Ok(())
}

fn write_one(file: &FileSpec, home: &str) -> Result<()> {
    let expanded = expand_home(&file.path, home);
    let path = Path::new(&expanded);
    if !path.is_absolute() {
        bail!("path is not absolute");
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, &file.content)?;
    std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode(file.mode.unwrap_or(DEFAULT_MODE)),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(path: &Path, mode: Option<u32>) -> FileSpec {
        FileSpec {
            path: path.to_string_lossy().into_owned(),
            content: "[x]\nkey = 1\n".into(),
            mode,
        }
    }

    #[test]
    fn writes_into_missing_directories_with_the_default_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("home/.codex/config.toml");
        write_all(&[spec(&path, None)], "/unused").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[x]\nkey = 1\n");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn honours_an_explicit_mode_and_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, "old").unwrap();
        write_all(&[spec(&path, Some(0o644))], "/unused").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[x]\nkey = 1\n");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644);
    }

    #[test]
    fn a_leading_tilde_expands_to_home_and_nothing_else_does() {
        assert_eq!(
            expand_home("~/.codex/config.toml", "/w/s1/home"),
            "/w/s1/home/.codex/config.toml"
        );
        assert_eq!(expand_home("~", "/w/s1/home"), "/w/s1/home");
        assert_eq!(expand_home("/abs/~/x", "/w/s1/home"), "/abs/~/x");
        assert_eq!(expand_home("~user/x", "/w/s1/home"), "~user/x");
    }

    #[test]
    fn a_relative_path_is_refused() {
        let err = write_all(&[spec(Path::new("rel/config.toml"), None)], "/unused").unwrap_err();
        assert!(err.to_string().contains("rel/config.toml"), "{err:#}");
        assert!(format!("{err:#}").contains("not absolute"), "{err:#}");
    }

    #[test]
    fn env_value_parses_or_is_empty() {
        assert!(parse_env_value(None).unwrap().is_empty());
        assert!(parse_env_value(Some("  ")).unwrap().is_empty());
        let files = parse_env_value(Some(
            r#"[{"path":"/a","content":"b"},{"path":"/c","content":"d","mode":420}]"#,
        ))
        .unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[1].mode, Some(0o644));
        assert!(parse_env_value(Some("not json")).is_err());
    }

    #[test]
    fn debug_hides_the_content() {
        let shown = format!("{:?}", spec(Path::new("/p"), Some(0o600)));
        assert!(
            shown.contains("/p") && !shown.contains("key = 1"),
            "{shown}"
        );
    }
}
