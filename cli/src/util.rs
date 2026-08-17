use std::fs;
use std::io::Cursor;
use std::path::Path;

use anyhow::Result;
use include_dir::Dir;
use serde_json;

/// Resolves the container CLI binary to shell out to. Honors `NASIKO_CONTAINER_CLI`
/// if set, otherwise prefers `docker` and falls back to `podman` when `docker` isn't
/// on PATH (e.g. podman-only dev setups without the podman-docker compat shim).
pub fn container_bin() -> String {
    if let Ok(bin) = std::env::var("NASIKO_CONTAINER_CLI") {
        return bin;
    }
    if on_path("docker") {
        "docker".to_string()
    } else {
        "podman".to_string()
    }
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file()))
        .unwrap_or(false)
}

/// Split a Docker image reference into `(name, tag)`, stripping any registry
/// host/path prefix first so a `host:port/...` ref doesn't get its port
/// mistaken for part of the name or tag (e.g. `localhost:5000/my-agent:v2` ->
/// `("my-agent", "v2")`, not `("localhost", "5000/my-agent")`).
pub fn parse_image_name_and_tag(image: &str) -> (String, String) {
    let last_segment = image.rsplit('/').next().unwrap_or(image);
    match last_segment.rsplit_once(':') {
        Some((name, tag)) => (name.to_string(), tag.to_string()),
        None => (last_segment.to_string(), "latest".to_string()),
    }
}

/// True if `image` has a `:tag` written explicitly, as opposed to
/// [`parse_image_name_and_tag`]'s implicit `"latest"` fallback. Used so an
/// untagged image isn't mistaken for a deliberately chosen version.
pub fn image_has_explicit_tag(image: &str) -> bool {
    image.rsplit('/').next().unwrap_or(image).contains(':')
}

pub fn extract_tar_gz(data: &[u8], dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    let cursor = Cursor::new(data);
    let gz = flate2::read::GzDecoder::new(cursor);
    let mut archive = tar::Archive::new(gz);
    archive.unpack(dest)?;
    Ok(())
}

pub fn extract_embedded_dir(dir: &Dir, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for file in dir.files() {
        let file_name = file.path().file_name().unwrap_or_default();
        fs::write(dest.join(file_name), file.contents())?;
    }
    for sub in dir.dirs() {
        let sub_name = sub.path().file_name().unwrap_or_default();
        extract_embedded_dir(sub, &dest.join(sub_name))?;
    }
    Ok(())
}

/// Try to read a version string from common project files.
///
/// Works for both a source directory and a `.zip` file. Resolution order:
///   1. AgentCard.json → `version`
///   2. pyproject.toml → `[project] version` or `[tool.poetry] version`
///   3. Cargo.toml     → `[package] version`
///
/// Returns `None` if no version is found; callers should fall back to `"0.1.0"` on
/// first upload or send `"auto"` on reupload (server auto-bumps the patch digit).
pub fn detect_version_from_source(source: &Path) -> Option<String> {
    if source.is_dir() {
        detect_version_from_dir(source)
    } else if source.extension().and_then(|e| e.to_str()) == Some("zip") {
        detect_version_from_zip(source)
    } else {
        None
    }
}

fn detect_version_from_dir(source: &Path) -> Option<String> {
    // 1. AgentCard.json
    let card_path = source.join("AgentCard.json");
    if card_path.exists()
        && let Ok(s) = fs::read_to_string(&card_path)
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&s)
        && let Some(ver) = v.get("version").and_then(|v| v.as_str())
    {
        return Some(ver.to_string());
    }

    // 2. pyproject.toml — [project] version or [tool.poetry] version
    let pyproject_path = source.join("pyproject.toml");
    if pyproject_path.exists()
        && let Ok(s) = fs::read_to_string(&pyproject_path)
        && let Some(ver) = parse_toml_version(&s, &["project", "tool.poetry"])
    {
        return Some(ver);
    }

    // 3. Cargo.toml — [package] version
    let cargo_path = source.join("Cargo.toml");
    if cargo_path.exists()
        && let Ok(s) = fs::read_to_string(&cargo_path)
        && let Some(ver) = parse_toml_version(&s, &["package"])
    {
        return Some(ver);
    }

    None
}

/// Read version from a zip file by extracting specific candidate files using
/// `unzip -p` (prints file contents to stdout without extracting to disk).
fn detect_version_from_zip(zip_path: &Path) -> Option<String> {
    use std::process::Command;

    let zip = zip_path.to_string_lossy();

    // Helper: run `unzip -p <zip> <file>` and return stdout on success.
    let read_from_zip = |file: &str| -> Option<String> {
        let out = Command::new("unzip")
            .args(["-p", &zip, file])
            .output()
            .ok()?;
        if out.status.success() && !out.stdout.is_empty() {
            String::from_utf8(out.stdout).ok()
        } else {
            None
        }
    };

    // 1. AgentCard.json
    if let Some(s) = read_from_zip("AgentCard.json")
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&s)
        && let Some(ver) = v.get("version").and_then(|v| v.as_str())
    {
        return Some(ver.to_string());
    }

    // 2. pyproject.toml
    if let Some(s) = read_from_zip("pyproject.toml")
        && let Some(ver) = parse_toml_version(&s, &["project", "tool.poetry"])
    {
        return Some(ver);
    }

    // 3. Cargo.toml
    if let Some(s) = read_from_zip("Cargo.toml")
        && let Some(ver) = parse_toml_version(&s, &["package"])
    {
        return Some(ver);
    }

    None
}

/// Minimal TOML version extractor: scans for `version = "..."` under any of the
/// given section headers. Does not depend on a TOML parser crate.
pub fn parse_toml_version(content: &str, sections: &[&str]) -> Option<String> {
    let mut in_section = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            let header = trimmed.trim_start_matches('[').trim_end_matches(']').trim();
            in_section = sections.contains(&header);
            continue;
        }
        if in_section && let Some(rest) = trimmed.strip_prefix("version") {
            let rest = rest.trim();
            if let Some(rest) = rest.strip_prefix('=') {
                let ver = rest.trim().trim_matches('"').trim_matches('\'');
                if !ver.is_empty() {
                    return Some(ver.to_string());
                }
            }
        }
    }
    None
}

/// Writes the resolved deploy/push version back into `AgentCard.json` so the
/// file reflects what's actually running instead of going stale the moment a
/// prompt or `--version` picks something different from what's on disk. A
/// no-op if the file already has this version — avoids reformatting the file
/// (and thus a spurious diff) on every deploy/push that didn't change it.
pub fn sync_card_version(card_path: &Path, card: &serde_json::Value, version: &str) -> Result<()> {
    if card.get("version").and_then(|v| v.as_str()) == Some(version) {
        return Ok(());
    }
    let mut updated = card.clone();
    updated["version"] = serde_json::Value::String(version.to_string());
    fs::write(card_path, serde_json::to_string_pretty(&updated)?)?;
    Ok(())
}

pub fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(first) => first.to_uppercase().to_string() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_has_explicit_tag_true_for_a_real_tag() {
        assert!(image_has_explicit_tag("legal-agent:1.0.1"));
        assert!(image_has_explicit_tag("nasiko/legal-agent:1.0.1"));
    }

    #[test]
    fn image_has_explicit_tag_false_for_a_bare_name() {
        assert!(!image_has_explicit_tag("legal-agent"));
        assert!(!image_has_explicit_tag("nasiko/legal-agent"));
    }

    #[test]
    fn image_has_explicit_tag_ignores_colons_in_a_registry_host_port() {
        // A `:` before the last `/` is a registry port, not a tag.
        assert!(!image_has_explicit_tag(
            "registry.example.com:5000/legal-agent"
        ));
        assert!(image_has_explicit_tag(
            "registry.example.com:5000/legal-agent:1.0.1"
        ));
    }

    #[test]
    fn sync_card_version_updates_a_changed_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AgentCard.json");
        let card = serde_json::json!({"name": "books", "version": "1.0.0", "skills": ["a"]});

        sync_card_version(&path, &card, "1.0.1").unwrap();

        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["version"], "1.0.1");
        assert_eq!(written["name"], "books");
        assert_eq!(written["skills"], serde_json::json!(["a"]));
    }

    #[test]
    fn sync_card_version_is_a_noop_when_version_already_matches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AgentCard.json");
        let card = serde_json::json!({"version": "1.0.0"});

        sync_card_version(&path, &card, "1.0.0").unwrap();

        assert!(
            !path.exists(),
            "should not write the file when nothing changed"
        );
    }
}
