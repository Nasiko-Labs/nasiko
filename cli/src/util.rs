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
    if card_path.exists() {
        if let Ok(s) = fs::read_to_string(&card_path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                if let Some(ver) = v.get("version").and_then(|v| v.as_str()) {
                    return Some(ver.to_string());
                }
            }
        }
    }

    // 2. pyproject.toml — [project] version or [tool.poetry] version
    let pyproject_path = source.join("pyproject.toml");
    if pyproject_path.exists() {
        if let Ok(s) = fs::read_to_string(&pyproject_path) {
            if let Some(ver) = parse_toml_version(&s, &["project", "tool.poetry"]) {
                return Some(ver);
            }
        }
    }

    // 3. Cargo.toml — [package] version
    let cargo_path = source.join("Cargo.toml");
    if cargo_path.exists() {
        if let Ok(s) = fs::read_to_string(&cargo_path) {
            if let Some(ver) = parse_toml_version(&s, &["package"]) {
                return Some(ver);
            }
        }
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
    if let Some(s) = read_from_zip("AgentCard.json") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            if let Some(ver) = v.get("version").and_then(|v| v.as_str()) {
                return Some(ver.to_string());
            }
        }
    }

    // 2. pyproject.toml
    if let Some(s) = read_from_zip("pyproject.toml") {
        if let Some(ver) = parse_toml_version(&s, &["project", "tool.poetry"]) {
            return Some(ver);
        }
    }

    // 3. Cargo.toml
    if let Some(s) = read_from_zip("Cargo.toml") {
        if let Some(ver) = parse_toml_version(&s, &["package"]) {
            return Some(ver);
        }
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
            in_section = sections.iter().any(|s| *s == header);
            continue;
        }
        if in_section {
            if let Some(rest) = trimmed.strip_prefix("version") {
                let rest = rest.trim();
                if let Some(rest) = rest.strip_prefix('=') {
                    let ver = rest.trim().trim_matches('"').trim_matches('\'');
                    if !ver.is_empty() {
                        return Some(ver.to_string());
                    }
                }
            }
        }
    }
    None
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
