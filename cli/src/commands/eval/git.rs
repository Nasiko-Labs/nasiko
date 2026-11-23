//! The only eval module that runs `git`. Everything goes through the `git` program with
//! `std::process::Command`, as the CLI already does for `docker`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
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
        let mut dir = Path::new(path).parent();
        while let Some(d) = dir {
            let manifest = if d.as_os_str().is_empty() {
                "Cargo.toml".to_string()
            } else {
                format!("{}/Cargo.toml", d.display())
            };
            if let Some(text) = self.file_at(sha, &manifest)? {
                return Ok(package_name(&text));
            }
            dir = d.parent();
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

    /// A unified diff of one file from `old` (`None` = new file) to `new`, with `a/<path>` and
    /// `b/<path>` headers as `git diff` writes them. Uses `git diff --no-index` on temp files.
    pub fn diff_texts(&self, path: &str, old: Option<&str>, new: &str) -> Result<String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "nasiko-eval-diff-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let (a, b) = (dir.join("a").join(path), dir.join("b").join(path));
        for (file, text) in [(&a, old), (&b, Some(new))] {
            let Some(text) = text else { continue };
            std::fs::create_dir_all(file.parent().context("path has a parent")?)?;
            std::fs::write(file, text)?;
        }
        let old_arg = if old.is_some() {
            format!("a/{path}")
        } else {
            "/dev/null".to_string()
        };
        let out = Command::new("git")
            .current_dir(&dir)
            .args([
                "diff",
                "--no-index",
                "--no-color",
                &old_arg,
                &format!("b/{path}"),
            ])
            .output()
            .context("failed to run git diff --no-index")?;
        let _ = std::fs::remove_dir_all(&dir);
        // Exit code 1 means "files differ", which is the normal case here.
        if !matches!(out.status.code(), Some(0 | 1)) {
            bail!(
                "git diff --no-index failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| fix_no_index_header(l, path))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n")
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

/// `--no-index` names files by the paths it was given (`a/a/<path>`, `b/b/<path>`); rewrite the
/// header lines to the `a/<path>` / `b/<path>` form `git apply` expects in the repo.
fn fix_no_index_header(line: &str, path: &str) -> String {
    let doubled_a = format!("a/a/{path}");
    let doubled_b = format!("b/b/{path}");
    if line.starts_with("diff --git ") || line.starts_with("--- ") || line.starts_with("+++ ") {
        return line
            .replace(&doubled_a, &format!("a/{path}"))
            .replace(&doubled_b, &format!("b/{path}"))
            .replace("a/dev/null", "/dev/null");
    }
    line.to_string()
}

/// `[package] name = "…"` from a `Cargo.toml`.
fn package_name(manifest: &str) -> Option<String> {
    let doc: toml_edit::DocumentMut = manifest.parse().ok()?;
    doc.get("package")?
        .get("name")?
        .as_str()
        .map(str::to_string)
}

/// `Key: value` lines at the end of a commit message.
fn trailers(body: &str) -> Vec<String> {
    body.lines()
        .map(str::trim)
        .filter(|l| {
            l.split_once(": ").is_some_and(|(k, _)| {
                !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            })
        })
        .map(str::to_string)
        .collect()
}

fn owner_and_name(url: &str) -> Option<String> {
    let path = url.trim_end_matches(".git").replace(':', "/");
    let mut parts = path.rsplit('/');
    let name = parts.next()?;
    let owner = parts.next()?;
    (!name.is_empty() && !owner.is_empty()).then(|| format!("{owner}/{name}"))
}

fn slug_from_remote(url: &str) -> Option<String> {
    owner_and_name(url).and_then(|full| full.rsplit('/').next().map(str::to_string))
}

/// `base` with one file's unified diff (`a/<path>` → `b/<path>`) applied: the file as the diff
/// leaves it.
pub fn apply_to_text(path: &str, base: &str, file_diff: &str) -> Result<String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "nasiko-eval-apply-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().context("path has a parent")?)?;
    std::fs::write(&file, base)?;
    std::fs::write(dir.join(".patch"), file_diff)?;
    let out = Command::new("git")
        .current_dir(&dir)
        .args(["apply", "--whitespace=nowarn", ".patch"])
        .output()
        .context("failed to run git apply");
    let result = match out {
        Ok(out) if out.status.success() => std::fs::read_to_string(&file).map_err(Into::into),
        Ok(out) => Err(anyhow::anyhow!(
            "git apply failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(e) => Err(e),
    };
    let _ = std::fs::remove_dir_all(&dir);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_come_from_ssh_and_https_remotes() {
        assert_eq!(
            owner_and_name("git@github.com:Nasiko-Labs/nasiko-cloud-rs.git").as_deref(),
            Some("Nasiko-Labs/nasiko-cloud-rs")
        );
        assert_eq!(
            slug_from_remote("https://github.com/Nasiko-Labs/nasiko-cloud-rs").as_deref(),
            Some("nasiko-cloud-rs")
        );
    }

    #[test]
    fn diff_texts_writes_repo_relative_headers() {
        let repo = Repo {
            root: std::env::temp_dir(),
        };
        let changed = repo
            .diff_texts("src/a.rs", Some("fn a() {}\n"), "fn a() { 1; }\n")
            .expect("git diff runs");
        assert!(
            changed.starts_with("diff --git a/src/a.rs b/src/a.rs\n"),
            "{changed}"
        );
        assert!(
            changed.contains("\n--- a/src/a.rs\n+++ b/src/a.rs\n"),
            "{changed}"
        );
        let added = repo
            .diff_texts("src/b.rs", None, "x\n")
            .expect("git diff runs");
        assert!(added.contains("--- /dev/null\n+++ b/src/b.rs\n"), "{added}");
        assert!(
            diff::parse_unified(&added).expect("parses")[0]
                .old_path
                .is_none()
        );
    }

    #[test]
    fn package_name_comes_from_the_package_table() {
        assert_eq!(
            package_name("[package]\nname = \"nasiko-compress\"\n").as_deref(),
            Some("nasiko-compress")
        );
        assert_eq!(package_name("[workspace]\nmembers = []\n"), None);
    }

    #[test]
    fn trailers_are_key_value_lines() {
        let body = "Fix retry\n\nLonger text: with colon? no\nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>\n";
        assert_eq!(
            trailers(body),
            vec!["Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"]
        );
    }

    #[test]
    fn apply_to_text_returns_the_changed_file() {
        let repo = Repo {
            root: std::env::temp_dir(),
        };
        let diff = repo
            .diff_texts("src/lib.rs", Some("fn a() {}\n"), "fn b() {}\n")
            .expect("diff");
        let applied = apply_to_text("src/lib.rs", "fn a() {}\n", &diff).expect("applies");
        assert_eq!(applied, "fn b() {}\n");
        assert!(apply_to_text("src/lib.rs", "fn other() {}\n", &diff).is_err());
    }
}
