//! The only eval module that runs `git`. Everything goes through the `git` program with
//! `std::process::Command`, as the CLI already does for `docker`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use nasiko_evals::remote::{RepoRef, owner_and_name, package_name, trailers};
use nasiko_evals::{CommitFacts, diff};

/// One git repository on disk.
pub struct Repo {
    root: PathBuf,
}

/// Inclusive time range for a commit search.
pub struct TimeWindow {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

/// Field separator for `git show --format`; never appears in commit metadata.
const SEP: &str = "\u{1f}";
/// Record separator between commits in `git log` output.
const RECORD: &str = "\u{1e}";

impl Repo {
    /// Opens the repository that contains `dir`.
    pub fn open(dir: &Path) -> Result<Self> {
        let out = run(dir, &["rev-parse", "--show-toplevel"])
            .with_context(|| format!("{} is not inside a git repository", dir.display()))?;
        Ok(Self {
            root: PathBuf::from(out.trim()),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A short name for the repo: the `origin` remote's last path part, else the folder name.
    pub fn slug(&self) -> String {
        let from_remote = self
            .git(&["remote", "get-url", "origin"])
            .ok()
            .and_then(|url| slug_from_remote(url.trim()));
        from_remote.unwrap_or_else(|| {
            self.root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "repo".into())
        })
    }

    /// `owner/name` from the `origin` remote, else the slug.
    pub fn full_name(&self) -> String {
        self.git(&["remote", "get-url", "origin"])
            .ok()
            .and_then(|url| owner_and_name(url.trim()))
            .unwrap_or_else(|| self.slug())
    }

    /// Every working folder of this repository (the main one and its `git worktree`s).
    pub fn worktree_roots(&self) -> Result<Vec<PathBuf>> {
        let out = self.git(&["worktree", "list", "--porcelain"])?;
        Ok(out
            .lines()
            .filter_map(|l| l.strip_prefix("worktree "))
            .map(PathBuf::from)
            .collect())
    }

    /// Commits in the window on any branch, oldest first, each with the files it changed.
    /// One `git log` call, so callers can drop unrelated commits before the costlier
    /// [`Repo::commit_facts`].
    pub fn commits_in(&self, window: &TimeWindow) -> Result<Vec<(String, Vec<String>)>> {
        let since = format!("--since=@{}", window.from.timestamp());
        let until = format!("--until=@{}", window.to.timestamp());
        let format = format!("--format={RECORD}%H");
        let out = self.git(&[
            "log",
            "--all",
            "--reverse",
            "--name-only",
            &format,
            &since,
            &until,
        ])?;
        Ok(out
            .split(RECORD)
            .filter_map(|record| {
                let mut lines = record.lines().map(str::trim).filter(|l| !l.is_empty());
                let sha = lines.next()?.to_string();
                Some((sha, lines.map(str::to_string).collect()))
            })
            .collect())
    }

    /// What linking needs to know about one commit.
    pub fn commit_facts(&self, sha: &str) -> Result<CommitFacts> {
        let format = format!("--format=%H{SEP}%P{SEP}%cI{SEP}%s{SEP}%B");
        let meta = self.git(&["show", "-s", &format, sha])?;
        let mut parts = meta.splitn(5, SEP);
        let mut next = || parts.next().unwrap_or_default().trim().to_string();
        let (sha, parents, date, subject, body) = (next(), next(), next(), next(), next());
        let parents: Vec<&str> = parents.split_whitespace().collect();
        let committed_at = DateTime::parse_from_rfc3339(&date)
            .with_context(|| format!("bad commit date {date:?} on {sha}"))?
            .with_timezone(&Utc);
        // A merge's diff is a combined `diff --cc`; merges are never tasks, so skip reading it.
        let files = if parents.len() > 1 {
            Vec::new()
        } else {
            diff::parse_unified(&self.git(&[
                "show",
                "--format=",
                "--no-color",
                "--unified=0",
                &sha,
            ])?)
            .with_context(|| format!("could not read the diff of {sha}"))?
        };
        let branches = self.git(&[
            "branch",
            "--all",
            "--contains",
            &sha,
            "--format=%(refname:short)",
        ])?;
        Ok(CommitFacts {
            parent: parents.first().map(|p| p.to_string()).unwrap_or_default(),
            is_merge: parents.len() > 1,
            committed_at,
            subject,
            added_lines: files.iter().flat_map(|f| f.added.clone()).collect(),
            files: files.iter().map(|f| f.path().to_string()).collect(),
            trailers: trailers(&body),
            branches: branches
                .lines()
                .map(|b| b.trim().trim_start_matches("origin/").to_string())
                .collect(),
            sha,
        })
    }

    /// The full diff between two commits, as `git apply` takes it.
    pub fn diff(&self, from: &str, to: &str) -> Result<String> {
        self.git(&["diff", "--no-color", "--binary", from, to])
    }

    /// Names of the files and folders at the repo root at a commit.
    pub fn root_files(&self, sha: &str) -> Result<Vec<String>> {
        Ok(self
            .git(&["ls-tree", "--name-only", sha])?
            .lines()
            .map(str::to_string)
            .collect())
    }

    /// Writes `git archive <sha>` (a tar of the tree, no history) to `out`.
    pub fn archive(&self, sha: &str, out: &Path) -> Result<()> {
        let out_arg = format!("--output={}", out.display());
        self.git(&["archive", "--format=tar", &out_arg, sha])
            .map(|_| ())
    }

    /// The Cargo package that owns `path` at a commit: the `[package] name` of the nearest
    /// `Cargo.toml` above it. `None` when there is none (e.g. a root-only workspace manifest).
    pub fn cargo_package_of(&self, sha: &str, path: &str) -> Result<Option<String>> {
        for manifest in nasiko_evals::spec::manifests_above(path) {
            if let Some(text) = self.file_at(sha, &manifest)? {
                return Ok(package_name(&text));
            }
        }
        Ok(None)
    }

    /// A file's content at a commit; `None` when it does not exist there.
    pub fn file_at(&self, sha: &str, path: &str) -> Result<Option<String>> {
        let spec = format!("{sha}:{path}");
        if self.git(&["cat-file", "-e", &spec]).is_err() {
            return Ok(None);
        }
        self.git(&["show", &spec]).map(Some)
    }

    fn git(&self, args: &[&str]) -> Result<String> {
        run(&self.root, args)
    }
}

fn run(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .context("failed to run git; is it installed?")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn slug_from_remote(url: &str) -> Option<String> {
    RepoRef::parse(url).map(|r| r.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_come_from_ssh_and_https_remotes() {
        assert_eq!(
            slug_from_remote("https://github.com/Nasiko-Labs/nasiko-cloud-rs").as_deref(),
            Some("nasiko-cloud-rs")
        );
        assert_eq!(
            slug_from_remote("git@github.com:Nasiko-Labs/nasiko-cloud-rs.git").as_deref(),
            Some("nasiko-cloud-rs")
        );
    }
}
