//! Best-effort: point this checkout's git hooks at `.githooks` so the
//! oss/Cargo.toml regeneration hook (see .githooks/pre-commit) runs without
//! every contributor remembering the one-time `git config` step manually.
//! No-op in CI, in the public standalone oss/ repo (no .githooks there), or
//! if git isn't reachable — never fails the build over this.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    if std::env::var_os("CI").is_some() {
        return;
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let Some(repo_root) = find_repo_root(&manifest_dir) else {
        return;
    };

    if !repo_root.join(".githooks").is_dir() {
        return;
    }

    let repo_root_str = repo_root.to_string_lossy();

    let already_set = Command::new("git")
        .args(["-C", &repo_root_str, "config", "--local", "core.hooksPath"])
        .output()
        .is_ok_and(|out| {
            out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == ".githooks"
        });

    if already_set {
        return;
    }

    let set = Command::new("git")
        .args([
            "-C",
            &repo_root_str,
            "config",
            "core.hooksPath",
            ".githooks",
        ])
        .status();

    if set.is_ok_and(|status| status.success()) {
        eprintln!(
            "nasiko-utils/build.rs: configured git core.hooksPath=.githooks for this checkout"
        );
    }
}

/// Walks up from `start` looking for a `.git` entry (dir for a normal clone,
/// file for a worktree) to find the repo root, without assuming a fixed
/// depth — this crate lives 2 levels down in the private monorepo
/// (repo_root/oss/utils) but only 1 level down once synced to the public
/// standalone oss/ repo (repo_root/utils).
fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}
