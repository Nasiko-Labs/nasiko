//! Cloning a session's repository into the workspace on the sandbox's first start.
//!
//! The control plane never pulls source: the shim does it from inside the sandbox, before it
//! spawns the harness, so it behaves the same on Docker and on pods. A failure does not stop the
//! shim: it keeps serving and tells the user in their first turn (see `server::Relay`), and the
//! workspace stays empty so the next start retries.
//!
//! The clone is `git clone --depth 1`, pinned to `https` and `ssh`, with terminal prompts off.
//! A token, when given, reaches git only through `GIT_ASKPASS`: a script the shim writes to a
//! private temporary file and removes afterwards. It is never in the URL, in argv, or in git's
//! environment.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Where the sandbox mounts its persistent workspace. Must equal `nasiko_runtime::WORKSPACE_MOUNT`;
/// the shim cannot depend on that crate, so the two are cross-referenced.
pub const WORKSPACE_MOUNT: &str = "/workspace";

const REPO_ENV: &str = "WORKSPACE_REPO";
/// The credential for a private repository. Removed from the harness's environment.
pub const REPO_TOKEN_ENV: &str = "WORKSPACE_REPO_TOKEN";
const TIMEOUT_ENV: &str = "WORKSPACE_CLONE_TIMEOUT_SECS";
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
/// The directory the image creates in the mount for the harness's `$HOME`.
const HOME_DIR: &str = "home";
/// Where a clone lands before its contents are moved into the (non-empty) mount.
const STAGING_DIR: &str = ".nasiko-clone";
/// How many of git's last stderr lines go into the notice shown to the user.
const NOTICE_LINES: usize = 6;

/// What to clone and where.
#[derive(Clone)]
pub struct CloneSpec {
    pub repo: String,
    pub token: Option<String>,
    pub mount: PathBuf,
    pub timeout: Duration,
}

impl std::fmt::Debug for CloneSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloneSpec")
            .field("repo", &self.repo)
            .field("token", &self.token.as_ref().map(|_| "***"))
            .field("mount", &self.mount)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl CloneSpec {
    /// From `WORKSPACE_REPO`, `WORKSPACE_REPO_TOKEN` and `WORKSPACE_CLONE_TIMEOUT_SECS`; `None`
    /// when no repository is configured.
    pub fn from_env(mount: PathBuf) -> Option<Self> {
        let repo = std::env::var(REPO_ENV)
            .ok()
            .filter(|r| !r.trim().is_empty())?;
        let timeout = std::env::var(TIMEOUT_ENV)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .filter(|secs| *secs > 0)
            .map_or(DEFAULT_TIMEOUT, Duration::from_secs);
        Some(Self {
            repo,
            token: std::env::var(REPO_TOKEN_ENV).ok().filter(|t| !t.is_empty()),
            mount,
            timeout,
        })
    }
}

/// Whether the workspace still wants its repository: it has no `.git` and holds nothing but the
/// image's `home/` directory. A workspace with anything else in it is the user's, and is left
/// alone.
pub fn needs_clone(mount: &Path) -> io::Result<bool> {
    let entries = match std::fs::read_dir(mount) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let name = entry?.file_name();
        if name != HOME_DIR && name != STAGING_DIR {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Clone the repository if the workspace needs it. `Some(notice)` is the user-facing message when
/// it failed; `None` when it succeeded or was not needed.
pub async fn ensure_cloned(spec: &CloneSpec) -> Option<String> {
    match needs_clone(&spec.mount) {
        Ok(false) => {
            tracing::info!("workspace already has content; not cloning");
            return None;
        }
        Ok(true) => {}
        Err(e) => return Some(format!("clone failed: cannot read the workspace: {e}")),
    }
    tracing::info!(mount = %spec.mount.display(), "cloning the session's repository");
    match clone_into_workspace(spec).await {
        Ok(()) => {
            tracing::info!("repository cloned");
            None
        }
        Err(reason) => {
            let reason = redact(&reason, spec.token.as_deref());
            tracing::error!(%reason, "repository clone failed");
            Some(format!("clone failed: {reason}"))
        }
    }
}

async fn clone_into_workspace(spec: &CloneSpec) -> Result<(), String> {
    std::fs::create_dir_all(&spec.mount).map_err(|e| e.to_string())?;
    let staging = spec.mount.join(STAGING_DIR);
    let _ = std::fs::remove_dir_all(&staging);
    let askpass = match &spec.token {
        Some(token) => Some(AskpassFile::create(token).map_err(|e| format!("askpass: {e}"))?),
        None => None,
    };
    let result = run_git_clone(spec, &staging, askpass.as_ref()).await;
    drop(askpass);
    let moved = match result {
        Ok(()) => move_contents(&staging, &spec.mount).map_err(|e| e.to_string()),
        Err(e) => Err(e),
    };
    let _ = std::fs::remove_dir_all(&staging);
    moved
}

fn allowed_protocols() -> &'static str {
    // `file` lets this module's own tests clone from a local bare repository.
    if cfg!(test) {
        "https:ssh:file"
    } else {
        "https:ssh"
    }
}

async fn run_git_clone(
    spec: &CloneSpec,
    target: &Path,
    askpass: Option<&AskpassFile>,
) -> Result<(), String> {
    let mut command = Command::new("git");
    command
        .args(["clone", "--depth", "1", "--", &spec.repo])
        .arg(target)
        .env("GIT_ALLOW_PROTOCOL", allowed_protocols())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove(REPO_TOKEN_ENV)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(askpass) = askpass {
        command.env("GIT_ASKPASS", askpass.path());
    }
    let mut child = crate::reaper::OwnChild::spawn(&mut command)
        .map_err(|e| format!("could not run git: {e}"))?;
    let stderr = child.take_stderr().ok_or("git stderr not piped")?;
    let token = spec.token.clone();
    // Git's stderr goes to the shim log line by line; the tail is kept for the user's notice.
    let logger = tokio::spawn(async move {
        let mut tail: Vec<String> = Vec::new();
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            // Git can echo a URL or a header carrying the credential: nothing it prints is logged
            // or shown to the user unredacted.
            let line = redact(&line, token.as_deref());
            tracing::info!(target: "git", "{line}");
            tail.push(line);
            if tail.len() > NOTICE_LINES {
                tail.remove(0);
            }
        }
        tail
    });
    let status = match tokio::time::timeout(spec.timeout, child.wait()).await {
        Ok(status) => status.map_err(|e| format!("git did not finish: {e}"))?,
        Err(_) => {
            let _ = child.kill().await;
            return Err(format!("timed out after {} s", spec.timeout.as_secs()));
        }
    };
    let tail = logger.await.unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        let detail = tail.join("; ");
        Err(if detail.is_empty() {
            format!("git exited with {status}")
        } else {
            detail
        })
    }
}

/// Move everything a clone produced (including `.git`) into the mount. An entry the mount
/// already has (the image's `home/`) is kept, and the repository's is skipped with a warning.
fn move_contents(from: &Path, to: &Path) -> io::Result<()> {
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if dest.exists() {
            tracing::warn!(entry = ?entry.file_name(), "workspace already has this entry; keeping it");
            continue;
        }
        std::fs::rename(entry.path(), dest)?;
    }
    Ok(())
}

fn redact(text: &str, token: Option<&str>) -> String {
    match token {
        Some(token) if !token.is_empty() => text.replace(token, "***"),
        _ => text.to_owned(),
    }
}

/// The `GIT_ASKPASS` script: answers git's username prompt with a fixed name and its password
/// prompt with the token. Created private and removed on drop.
pub struct AskpassFile(PathBuf);

impl AskpassFile {
    pub fn create(token: &str) -> io::Result<Self> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path =
            std::env::temp_dir().join(format!("nasiko-askpass-{}-{nanos}", std::process::id()));
        // Owner-only. It must be executable for git to run it, so 0700 rather than 0600.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&path)?;
        let quoted = token.replace('\'', "'\\''");
        write!(
            file,
            "#!/bin/sh\ncase \"$1\" in\n  Username*) printf '%s\\n' 'x-access-token' ;;\n  *) printf '%s\\n' '{quoted}' ;;\nesac\n"
        )?;
        Ok(Self(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for AskpassFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn touch(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir.join(name)).unwrap();
    }

    #[test]
    fn an_empty_or_home_only_workspace_wants_the_clone() {
        let dir = tempfile::tempdir().unwrap();
        assert!(needs_clone(dir.path()).unwrap());
        touch(dir.path(), "home");
        assert!(needs_clone(dir.path()).unwrap());
        assert!(needs_clone(&dir.path().join("missing")).unwrap());
    }

    #[test]
    fn a_workspace_with_content_or_a_repository_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "home");
        touch(dir.path(), ".git");
        assert!(!needs_clone(dir.path()).unwrap());
        let other = tempfile::tempdir().unwrap();
        touch(other.path(), "src");
        assert!(!needs_clone(other.path()).unwrap());
    }

    #[test]
    fn the_askpass_file_is_private_answers_both_prompts_and_is_removed() {
        let path;
        {
            let askpass = AskpassFile::create("tok'en").unwrap();
            path = askpass.path().to_owned();
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "owner only");
            let answer = |prompt: &str| {
                let out = std::process::Command::new(&path)
                    .arg(prompt)
                    .output()
                    .unwrap();
                String::from_utf8(out.stdout).unwrap()
            };
            assert_eq!(
                answer("Username for 'https://github.com': "),
                "x-access-token\n"
            );
            assert_eq!(answer("Password for 'https://x@github.com': "), "tok'en\n");
        }
        assert!(!path.exists(), "removed after the clone");
    }

    #[test]
    fn a_token_is_redacted_from_a_failure_message() {
        assert_eq!(redact("bad tok123 here", Some("tok123")), "bad *** here");
        assert_eq!(redact("plain", None), "plain");
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// A bare repository with one commit, as a `file://` URL.
    fn bare_repo() -> (tempfile::TempDir, String) {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        std::fs::create_dir(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main"]);
        std::fs::write(work.join("README.md"), "hello").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-q", "-m", "init"]);
        let bare = root.path().join("bare.git");
        git(
            root.path(),
            &[
                "clone",
                "-q",
                "--bare",
                work.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );
        let url = format!("file://{}", bare.display());
        (root, url)
    }

    fn spec(repo: String, mount: &Path) -> CloneSpec {
        CloneSpec {
            repo,
            token: None,
            mount: mount.to_owned(),
            timeout: Duration::from_secs(60),
        }
    }

    #[tokio::test]
    async fn the_repo_is_cloned_into_the_mount_itself_beside_the_images_home() {
        let (_keep, url) = bare_repo();
        let mount = tempfile::tempdir().unwrap();
        touch(mount.path(), "home");

        assert_eq!(ensure_cloned(&spec(url, mount.path())).await, None);
        assert_eq!(
            std::fs::read_to_string(mount.path().join("README.md")).unwrap(),
            "hello"
        );
        assert!(mount.path().join(".git").exists());
        assert!(
            mount.path().join("home").exists(),
            "the image's home is kept"
        );
        assert!(
            !mount.path().join(STAGING_DIR).exists(),
            "no staging left behind"
        );
        // The workspace now has a repository, so it is not cloned again.
        assert!(!needs_clone(mount.path()).unwrap());
    }

    #[tokio::test]
    async fn a_failed_clone_reports_why_and_leaves_the_workspace_retryable() {
        let mount = tempfile::tempdir().unwrap();
        let notice = ensure_cloned(&spec("file:///definitely/not/a/repo".into(), mount.path()))
            .await
            .expect("a failure notice");
        assert!(notice.starts_with("clone failed: "), "{notice}");
        assert!(
            needs_clone(mount.path()).unwrap(),
            "still empty, so the next start retries"
        );
    }

    #[tokio::test]
    async fn a_workspace_with_content_is_not_cloned_into() {
        let (_keep, url) = bare_repo();
        let mount = tempfile::tempdir().unwrap();
        touch(mount.path(), "mine");
        assert_eq!(ensure_cloned(&spec(url, mount.path())).await, None);
        assert!(!mount.path().join("README.md").exists());
    }

    #[tokio::test]
    async fn a_clone_that_outlasts_its_timeout_fails_with_a_timeout_notice() {
        let (_keep, url) = bare_repo();
        let mount = tempfile::tempdir().unwrap();
        let mut slow = spec(url, mount.path());
        slow.timeout = Duration::from_millis(1);
        let notice = ensure_cloned(&slow).await.expect("a failure notice");
        assert!(notice.contains("timed out"), "{notice}");
        assert!(needs_clone(mount.path()).unwrap());
    }
}
