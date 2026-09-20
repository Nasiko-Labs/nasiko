//! `LocalSandbox` — runs file ops and shell commands in a fixed workspace directory on the
//! host. Used by the CLI agent (workspace = current dir). Every path is contained to the root.

use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use super::{BoxFuture, ExecResult, Sandbox};

/// Max bytes of captured stdout/stderr returned to the model before truncation.
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
/// Default command timeout when the caller doesn't specify one.
const DEFAULT_TIMEOUT_S: u64 = 120;

#[derive(Clone)]
pub struct LocalSandbox {
    root: PathBuf,
}

impl LocalSandbox {
    /// Create a sandbox rooted at `root`, which must exist and be a directory.
    pub fn new(root: &str) -> Result<Self, String> {
        let canonical = std::fs::canonicalize(root)
            .map_err(|e| format!("workspace root '{root}' is not accessible: {e}"))?;
        if !canonical.is_dir() {
            return Err(format!("workspace root '{root}' is not a directory"));
        }
        Ok(Self { root: canonical })
    }

    /// Resolve a relative path against the root, rejecting anything that escapes it.
    ///
    /// Works lexically (no filesystem access) so it also validates paths that don't exist yet
    /// (e.g. `write_file` to a new file): absolute paths and `..` traversal above the root are
    /// rejected.
    fn resolve(&self, rel: &str) -> Result<PathBuf, String> {
        let p = Path::new(rel);
        let mut out = self.root.clone();
        for comp in p.components() {
            match comp {
                Component::Normal(c) => out.push(c),
                Component::CurDir => {}
                Component::ParentDir => {
                    // Pop, but never above the root.
                    if !out.pop() || !out.starts_with(&self.root) {
                        return Err(format!("path '{rel}' escapes the workspace"));
                    }
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(format!(
                        "absolute path '{rel}' is not allowed; use a relative path"
                    ));
                }
            }
        }
        if !out.starts_with(&self.root) {
            return Err(format!("path '{rel}' escapes the workspace"));
        }
        Ok(out)
    }
}

fn truncate(mut s: String) -> String {
    if s.len() > MAX_OUTPUT_BYTES {
        // Truncate on a char boundary at or below the limit.
        let mut cut = MAX_OUTPUT_BYTES;
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("\n…[output truncated]");
    }
    s
}

impl Sandbox for LocalSandbox {
    fn read_file<'a>(
        &'a self,
        path: &'a str,
        range: Option<(usize, usize)>,
    ) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let full = self.resolve(path)?;
            let content = tokio::fs::read_to_string(&full)
                .await
                .map_err(|e| format!("read '{path}': {e}"))?;
            let out = match range {
                Some((start, end)) => {
                    let start = start.max(1);
                    content
                        .lines()
                        .enumerate()
                        .filter(|(i, _)| {
                            let n = i + 1;
                            n >= start && n <= end
                        })
                        .map(|(i, l)| format!("{}\t{}", i + 1, l))
                        .collect::<Vec<_>>()
                        .join("\n")
                }
                None => content
                    .lines()
                    .enumerate()
                    .map(|(i, l)| format!("{}\t{}", i + 1, l))
                    .collect::<Vec<_>>()
                    .join("\n"),
            };
            Ok(truncate(out))
        })
    }

    fn read_file_raw<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let full = self.resolve(path)?;
            tokio::fs::read_to_string(&full)
                .await
                .map_err(|e| format!("read '{path}': {e}"))
        })
    }

    fn write_file<'a>(
        &'a self,
        path: &'a str,
        content: &'a str,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let full = self.resolve(path)?;
            if let Some(parent) = full.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| format!("create parent dirs for '{path}': {e}"))?;
            }
            tokio::fs::write(&full, content)
                .await
                .map_err(|e| format!("write '{path}': {e}"))
        })
    }

    fn list_dir<'a>(
        &'a self,
        path: &'a str,
        depth: Option<usize>,
    ) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let base = self.resolve(path)?;
            let root = self.root.clone();
            let max_depth = depth.unwrap_or(usize::MAX);
            // walkdir is sync; run it on a blocking thread.
            let listing = tokio::task::spawn_blocking(move || {
                let mut entries = Vec::new();
                let walker = walkdir::WalkDir::new(&base)
                    .max_depth(max_depth)
                    .sort_by_file_name()
                    .into_iter()
                    .filter_entry(|e| {
                        let name = e.file_name().to_string_lossy();
                        !matches!(name.as_ref(), ".git" | "target" | "node_modules")
                    });
                for entry in walker.flatten() {
                    let rel = entry.path().strip_prefix(&root).unwrap_or(entry.path());
                    if rel.as_os_str().is_empty() {
                        continue;
                    }
                    let suffix = if entry.file_type().is_dir() { "/" } else { "" };
                    entries.push(format!("{}{}", rel.display(), suffix));
                }
                entries
            })
            .await
            .map_err(|e| format!("list '{path}': {e}"))?;

            if listing.is_empty() {
                Ok(format!("(empty) {path}"))
            } else {
                Ok(truncate(listing.join("\n")))
            }
        })
    }

    fn exec<'a>(
        &'a self,
        command: &'a str,
        timeout_s: Option<u64>,
    ) -> BoxFuture<'a, Result<ExecResult, String>> {
        Box::pin(async move {
            let timeout = Duration::from_secs(timeout_s.unwrap_or(DEFAULT_TIMEOUT_S));
            let child = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(&self.root)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("spawn command failed: {e}"))?;

            let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
                Ok(res) => res.map_err(|e| format!("command failed: {e}"))?,
                Err(_) => {
                    return Err(format!(
                        "command timed out after {}s: {command}",
                        timeout.as_secs()
                    ));
                }
            };

            Ok(ExecResult {
                stdout: truncate(String::from_utf8_lossy(&output.stdout).into_owned()),
                stderr: truncate(String::from_utf8_lossy(&output.stderr).into_owned()),
                exit_code: output.status.code().unwrap_or(-1),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        // Unique per test via thread name; created fresh.
        let base = std::env::temp_dir().join(format!(
            "coding-agent-test-{}",
            std::thread::current()
                .name()
                .unwrap_or("t")
                .replace("::", "_")
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::canonicalize(&base).unwrap()
    }

    #[tokio::test]
    async fn write_read_roundtrip() {
        let root = temp_root();
        let sb = LocalSandbox::new(root.to_str().unwrap()).unwrap();
        sb.write_file("src/lib.rs", "fn a() {}\nfn b() {}\n")
            .await
            .unwrap();
        let out = sb.read_file("src/lib.rs", None).await.unwrap();
        assert!(out.contains("1\tfn a() {}"));
        assert!(out.contains("2\tfn b() {}"));
    }

    #[tokio::test]
    async fn read_range() {
        let root = temp_root();
        let sb = LocalSandbox::new(root.to_str().unwrap()).unwrap();
        sb.write_file("f.txt", "l1\nl2\nl3\nl4\n").await.unwrap();
        let out = sb.read_file("f.txt", Some((2, 3))).await.unwrap();
        assert_eq!(out, "2\tl2\n3\tl3");
    }

    #[tokio::test]
    async fn list_skips_noise_dirs() {
        let root = temp_root();
        let sb = LocalSandbox::new(root.to_str().unwrap()).unwrap();
        sb.write_file("src/main.rs", "").await.unwrap();
        sb.write_file("target/junk", "").await.unwrap();
        sb.write_file(".git/cfg", "").await.unwrap();
        let out = sb.list_dir(".", None).await.unwrap();
        assert!(out.contains("src/main.rs"));
        assert!(!out.contains("target"));
        assert!(!out.contains(".git"));
    }

    #[tokio::test]
    async fn exec_captures_output_and_code() {
        let root = temp_root();
        let sb = LocalSandbox::new(root.to_str().unwrap()).unwrap();
        let r = sb
            .exec("echo hi; echo err 1>&2; exit 3", None)
            .await
            .unwrap();
        assert_eq!(r.stdout.trim(), "hi");
        assert_eq!(r.stderr.trim(), "err");
        assert_eq!(r.exit_code, 3);
    }

    #[tokio::test]
    async fn path_containment_rejects_escape() {
        let root = temp_root();
        let sb = LocalSandbox::new(root.to_str().unwrap()).unwrap();
        assert!(sb.read_file("../escape.txt", None).await.is_err());
        assert!(sb.write_file("../../etc/passwd", "x").await.is_err());
        assert!(sb.read_file("/etc/passwd", None).await.is_err());
        // A `..` that stays within root is fine.
        sb.write_file("a/b.txt", "x").await.unwrap();
        assert!(sb.read_file("a/../a/b.txt", None).await.is_ok());
    }
}
