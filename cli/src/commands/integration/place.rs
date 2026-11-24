//! Where a coding session ran: its working folder and branch from the transcript, and the git
//! repository holding that folder, read from local git while syncing. The platform's replay
//! evals find a session's repository from this, so the person never has to name it.
//!
//! Only the remote's host and `owner/name` leave the machine: credentials in the remote URL are
//! dropped by `RepoRef::parse`.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use nasiko_evals::remote::RepoRef;
use nasiko_types::{CodingAgentRepo, CodingAgentSessionPlace};
use serde::Deserialize;

/// The session's place from a Claude Code transcript: the first `cwd` and `gitBranch` recorded,
/// plus the repository holding that folder. `None` when the transcript names no folder.
pub fn from_claude_transcript(content: &str) -> Option<CodingAgentSessionPlace> {
    let (cwd, git_branch) = claude_folder_and_branch(content);
    let cwd = cwd?;
    Some(CodingAgentSessionPlace {
        repo: repo_of(&cwd),
        git_branch,
        cwd: Some(cwd),
    })
}

#[derive(Deserialize)]
struct Line {
    cwd: Option<String>,
    #[serde(rename = "gitBranch")]
    git_branch: Option<String>,
}

fn claude_folder_and_branch(content: &str) -> (Option<String>, Option<String>) {
    let (mut cwd, mut branch) = (None, None);
    for line in content.lines() {
        let Ok(line) = serde_json::from_str::<Line>(line) else {
            continue;
        };
        if cwd.is_none() {
            cwd = line.cwd.filter(|c| !c.trim().is_empty());
        }
        if branch.is_none() {
            branch = line.git_branch.filter(|b| !b.trim().is_empty());
        }
        if cwd.is_some() && branch.is_some() {
            break;
        }
    }
    (cwd, branch)
}

/// The repository holding `cwd`, asked of git once per folder per process. `None` when the
/// folder is gone, is not a git repository, or has no `origin` remote that names one.
fn repo_of(cwd: &str) -> Option<CodingAgentRepo> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<CodingAgentRepo>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(known) = cache.lock().ok().and_then(|c| c.get(cwd).cloned()) {
        return known;
    }
    let found = read_repo(Path::new(cwd));
    if let Ok(mut c) = cache.lock() {
        c.insert(cwd.to_string(), found.clone());
    }
    found
}

fn read_repo(cwd: &Path) -> Option<CodingAgentRepo> {
    if !cwd.is_dir() {
        return None;
    }
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let remote = RepoRef::parse(&git(&["remote", "get-url", "origin"])?)?;
    Some(CodingAgentRepo {
        host: remote.host,
        owner: remote.owner,
        name: remote.name,
        root: git(&["rev-parse", "--show-toplevel"]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_and_branch_come_from_the_first_lines_that_have_them() {
        let transcript = concat!(
            "{\"type\":\"summary\"}\n",
            "not json\n",
            "{\"type\":\"user\",\"cwd\":\"/src/app\",\"gitBranch\":\"\"}\n",
            "{\"type\":\"assistant\",\"cwd\":\"/src/other\",\"gitBranch\":\"feat/x\"}\n",
        );
        assert_eq!(
            claude_folder_and_branch(transcript),
            (Some("/src/app".into()), Some("feat/x".into()))
        );
        assert_eq!(claude_folder_and_branch("{}\n"), (None, None));
    }

    #[test]
    fn the_repo_comes_from_origin_without_credentials() {
        let dir = tempfile::tempdir().expect("temp dir");
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .current_dir(dir.path())
                .args(args)
                .status()
                .expect("git runs")
                .success();
            assert!(ok, "git {args:?}");
        };
        run(&["init", "-q"]);
        run(&[
            "remote",
            "add",
            "origin",
            "https://x-access-token:secret@github.com/Acme/widgets.git",
        ]);
        std::fs::create_dir(dir.path().join("sub")).expect("sub");
        let repo = read_repo(&dir.path().join("sub")).expect("a repo");
        assert_eq!(
            (repo.host.as_str(), repo.owner.as_str(), repo.name.as_str()),
            ("github.com", "Acme", "widgets")
        );
        let root = std::fs::canonicalize(repo.root.expect("root")).expect("exists");
        assert_eq!(root, std::fs::canonicalize(dir.path()).expect("exists"));
        assert!(read_repo(Path::new("/definitely/not/here")).is_none());
    }
}
