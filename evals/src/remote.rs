//! Naming repositories and reading commit metadata, the same way from local git and a forge's
//! API.

use serde::{Deserialize, Serialize};

/// A repository as its `origin` remote names it: `github.com` + `owner/name`. Credentials in
/// the URL are never kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoRef {
    pub host: String,
    pub owner: String,
    pub name: String,
}

impl RepoRef {
    /// Reads `https://[user[:token]@]host/owner/name[.git]`, `ssh://git@host[:port]/owner/name`
    /// and scp-style `git@host:owner/name.git`. `None` for anything else (a local path).
    pub fn parse(url: &str) -> Option<Self> {
        let url = url.trim();
        let (host_part, path) = if let Some((scheme, rest)) = url.split_once("://") {
            if !matches!(scheme, "https" | "http" | "ssh" | "git") {
                return None;
            }
            rest.split_once('/')?
        } else {
            // scp-style: `[user@]host:path`, never a Windows drive or a plain path.
            let (host, path) = url.split_once(':')?;
            if host.contains('/') || host.len() < 2 {
                return None;
            }
            (host, path)
        };
        let host = host_part
            .rsplit('@')
            .next()?
            .split(':')
            .next()?
            .to_lowercase();
        let mut parts = path
            .trim_matches('/')
            .trim_end_matches(".git")
            .rsplitn(2, '/');
        let name = parts.next()?.to_string();
        let owner = parts.next()?.to_string();
        let ok = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        };
        // GitLab subgroups (`group/sub/name`) keep the whole owner path.
        let owner_ok = owner.split('/').all(ok);
        (!host.is_empty() && owner_ok && ok(&name)).then_some(Self { host, owner, name })
    }

    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    pub fn is_github(&self) -> bool {
        self.host == "github.com"
    }
}

/// `owner/name` from a remote URL.
pub fn owner_and_name(url: &str) -> Option<String> {
    RepoRef::parse(url).map(|r| r.full_name())
}

/// `Key: value` lines at the end of a commit message.
pub fn trailers(body: &str) -> Vec<String> {
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

/// `[package] name = "…"` from a `Cargo.toml`.
pub fn package_name(manifest: &str) -> Option<String> {
    let doc: toml_edit::DocumentMut = manifest.parse().ok()?;
    doc.get("package")?
        .get("name")?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gh(owner: &str, name: &str) -> Option<RepoRef> {
        Some(RepoRef {
            host: "github.com".into(),
            owner: owner.into(),
            name: name.into(),
        })
    }

    #[test]
    fn reads_every_remote_form_and_drops_credentials() {
        assert_eq!(
            RepoRef::parse("git@github.com:Nasiko-Labs/nasiko-cloud-rs.git"),
            gh("Nasiko-Labs", "nasiko-cloud-rs")
        );
        assert_eq!(
            RepoRef::parse("https://github.com/Nasiko-Labs/nasiko-cloud-rs"),
            gh("Nasiko-Labs", "nasiko-cloud-rs")
        );
        assert_eq!(
            RepoRef::parse("https://x-access-token:ghs_secret@github.com/o/r.git"),
            gh("o", "r")
        );
        assert_eq!(
            RepoRef::parse("ssh://git@github.com:22/o/r.git"),
            gh("o", "r")
        );
        let lab = RepoRef::parse("git@gitlab.com:group/sub/r.git").expect("gitlab");
        assert_eq!(
            (lab.host.as_str(), lab.owner.as_str()),
            ("gitlab.com", "group/sub")
        );
        assert!(!lab.is_github());
        assert_eq!(RepoRef::parse("/home/me/repo"), None);
        assert_eq!(RepoRef::parse("C:\\repo"), None);
        assert_eq!(
            owner_and_name("git@github.com:o/r.git").as_deref(),
            Some("o/r")
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
}
