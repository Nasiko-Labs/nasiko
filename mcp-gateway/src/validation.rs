//! Pattern-based, fail-open validation for an uploaded MCP server's source tree.
//!
//! Unlike agent uploads (which hard-require a `main.py` entrypoint), there is no
//! universal manifest or entrypoint convention across the MCP ecosystem — see
//! `mcp_upload_flow_docs/MCP_SERVER_UPLOAD_RESEARCH_INDUSTRY.md`. Only a missing or
//! invalid Dockerfile is a hard rejection; everything else is *detected*, not
//! *enforced*. The live `initialize`/`tools/list` handshake performed after deploy
//! is the actual correctness gate, matching how Smithery.ai and the rest of the
//! ecosystem treat MCP server discovery.
//!
//! No `AppState`/DB/network dependency by design — keeps this hermetically
//! unit-testable (F.I.R.S.T.).

use std::path::Path;

/// What the build pipeline detected about an uploaded MCP server's project shape.
/// `Unknown` is not a rejection — it still proceeds to a real build attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedRuntime {
    Python,
    Node,
    Unknown,
}

impl DetectedRuntime {
    /// Diagnostic string stored on `mcp_connector_builds.detected_runtime`.
    pub fn as_str(&self) -> &'static str {
        match self {
            DetectedRuntime::Python => "python",
            DetectedRuntime::Node => "node",
            DetectedRuntime::Unknown => "unknown",
        }
    }
}

/// Only covers failures this module can actually detect from an *already
/// extracted* directory. Two related failure modes the original design doc
/// initially sketched as variants here in fact belong to earlier layers, since
/// they operate on the raw upload before extraction ever happens, and stay
/// there rather than being duplicated: an oversized upload is rejected while
/// streaming, before any bytes are ever unzipped
/// (`oss/server/src/multipart_util.rs::StreamUploadError::TooLarge`); a
/// zip-slip/path-traversal entry is rejected during extraction itself
/// (`oss/utils/src/zip.rs`, shared with the agent-upload path, not duplicated
/// here).
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("zip contains no root Dockerfile")]
    MissingDockerfile,
    #[error("Dockerfile has no FROM line")]
    InvalidDockerfile,
    #[error(
        "Dockerfile uses BuildKit-only syntax ({0}), which this platform's build backend \
         does not support — remove it and rewrite without BuildKit features"
    )]
    BuildkitOnlySyntax(&'static str),
}

/// Validates an extracted MCP-server upload directory.
///
/// Deliberately loose: only a missing/invalid Dockerfile is a hard rejection. An
/// `Unknown` runtime still returns `Ok` and proceeds to a real build attempt — the
/// live MCP handshake after deploy is the actual correctness gate.
pub fn validate_mcp_server_zip(extracted_dir: &Path) -> Result<DetectedRuntime, ValidationError> {
    let dockerfile = extracted_dir.join("Dockerfile");
    if !dockerfile.exists() {
        return Err(ValidationError::MissingDockerfile);
    }
    let contents = std::fs::read_to_string(&dockerfile).unwrap_or_default();
    if !contents
        .lines()
        .any(|l| l.trim_start().starts_with("FROM "))
    {
        return Err(ValidationError::InvalidDockerfile);
    }
    if let Some(feature) = detect_buildkit_only_syntax(&contents) {
        return Err(ValidationError::BuildkitOnlySyntax(feature));
    }

    if detect_python(extracted_dir) {
        return Ok(DetectedRuntime::Python);
    }
    if detect_node(extracted_dir) {
        return Ok(DetectedRuntime::Node);
    }
    Ok(DetectedRuntime::Unknown)
}

/// A hard rejection, unlike the hints below: our build backend
/// (`DockerRuntime::build`, `oss/runtime/src/docker/mod.rs`) always builds with
/// bollard's classic (non-BuildKit) builder, so any of these fail the build with
/// a raw daemon error ("the --mount option requires BuildKit") instead of a
/// readable one. Catching them here turns that into a clear, actionable message.
fn detect_buildkit_only_syntax(dockerfile: &str) -> Option<&'static str> {
    if dockerfile
        .lines()
        .any(|l| l.trim_start().starts_with('#') && l.contains("syntax="))
    {
        return Some("a `# syntax=` frontend directive");
    }
    if dockerfile.contains("--mount=") {
        return Some("`--mount`");
    }
    if dockerfile.contains("--secret=") {
        return Some("`--secret`");
    }
    if dockerfile.lines().any(|l| {
        let l = l.trim_start();
        (l.starts_with("RUN ") || l.starts_with("COPY ") || l.starts_with("ADD "))
            && l.contains("<<")
    }) {
        return Some("a heredoc (`<<`) instruction");
    }
    None
}

/// Hint only, never a rejection: `pyproject.toml` exists and either declares an
/// `mcp`/`fastmcp` dependency, or some `*.py` file imports one of those modules.
fn detect_python(dir: &Path) -> bool {
    let pyproject = dir.join("pyproject.toml");
    if !pyproject.exists() {
        return false;
    }
    if let Ok(contents) = std::fs::read_to_string(&pyproject)
        && (contents.contains("mcp") || contents.contains("fastmcp"))
    {
        return true;
    }
    walk_files(dir, "py", |contents| {
        contents.contains("from mcp")
            || contents.contains("from fastmcp")
            || contents.contains("import mcp")
            || contents.contains("import fastmcp")
    })
}

/// Hint only, never a rejection: `package.json` exists and declares
/// `@modelcontextprotocol/sdk` as a dependency or dev-dependency.
fn detect_node(dir: &Path) -> bool {
    let package_json = dir.join("package.json");
    let Ok(contents) = std::fs::read_to_string(&package_json) else {
        return false;
    };
    contents.contains("@modelcontextprotocol/sdk")
}

/// Walks `dir` recursively, applying `matches` to the contents of every file with
/// extension `ext`, short-circuiting on the first match. Best-effort: unreadable
/// entries are skipped, never an error — this is a soft hint, not validation.
fn walk_files(dir: &Path, ext: &str, matches: impl Fn(&str) -> bool + Copy) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if walk_files(&path, ext, matches) {
                return true;
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some(ext)
            && let Ok(contents) = std::fs::read_to_string(&path)
            && matches(&contents)
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, rel: &str, contents: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn detects_python_fastmcp_shape() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Dockerfile", "FROM python:3.12\n");
        write(
            dir.path(),
            "pyproject.toml",
            "[project]\ndependencies = [\"fastmcp\"]\n",
        );
        write(dir.path(), "server.py", "from fastmcp import FastMCP\n");
        assert_eq!(
            validate_mcp_server_zip(dir.path()).unwrap(),
            DetectedRuntime::Python
        );
    }

    #[test]
    fn detects_node_sdk_shape() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Dockerfile", "FROM node:20\n");
        write(
            dir.path(),
            "package.json",
            r#"{"dependencies": {"@modelcontextprotocol/sdk": "^1.0.0"}}"#,
        );
        assert_eq!(
            validate_mcp_server_zip(dir.path()).unwrap(),
            DetectedRuntime::Node
        );
    }

    #[test]
    fn unknown_shape_still_proceeds_ok() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Dockerfile", "FROM alpine:3.20\n");
        write(dir.path(), "run.sh", "#!/bin/sh\necho hi\n");
        assert_eq!(
            validate_mcp_server_zip(dir.path()).unwrap(),
            DetectedRuntime::Unknown
        );
    }

    #[test]
    fn missing_dockerfile_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "server.py", "print(1)\n");
        assert!(matches!(
            validate_mcp_server_zip(dir.path()),
            Err(ValidationError::MissingDockerfile)
        ));
    }

    #[test]
    fn dockerfile_without_from_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Dockerfile", "RUN echo hi\n");
        assert!(matches!(
            validate_mcp_server_zip(dir.path()),
            Err(ValidationError::InvalidDockerfile)
        ));
    }

    #[test]
    fn dockerfile_with_mount_flag_is_rejected_before_a_real_build_attempt() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "Dockerfile",
            "FROM python:3.12\nRUN --mount=type=cache,target=/root/.cache pip install mcp\n",
        );
        assert!(matches!(
            validate_mcp_server_zip(dir.path()),
            Err(ValidationError::BuildkitOnlySyntax("`--mount`"))
        ));
    }

    #[test]
    fn dockerfile_with_secret_flag_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "Dockerfile",
            "FROM node:20\nRUN --secret=id=npm,target=/root/.npmrc npm install\n",
        );
        assert!(matches!(
            validate_mcp_server_zip(dir.path()),
            Err(ValidationError::BuildkitOnlySyntax("`--secret`"))
        ));
    }

    #[test]
    fn dockerfile_with_syntax_directive_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "Dockerfile",
            "# syntax=docker/dockerfile:1\nFROM alpine:3.20\n",
        );
        assert!(matches!(
            validate_mcp_server_zip(dir.path()),
            Err(ValidationError::BuildkitOnlySyntax(
                "a `# syntax=` frontend directive"
            ))
        ));
    }

    #[test]
    fn dockerfile_with_heredoc_run_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "Dockerfile",
            "FROM alpine:3.20\nRUN <<EOF\necho hi\nEOF\n",
        );
        assert!(matches!(
            validate_mcp_server_zip(dir.path()),
            Err(ValidationError::BuildkitOnlySyntax(
                "a heredoc (`<<`) instruction"
            ))
        ));
    }

    #[test]
    fn ordinary_multi_stage_copy_from_is_not_mistaken_for_buildkit_only_syntax() {
        // `COPY --from=` is a classic-builder feature (multi-stage builds), not BuildKit-only.
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "Dockerfile",
            "FROM golang:1.22 AS build\nFROM alpine:3.20\nCOPY --from=build /bin/app /bin/app\n",
        );
        assert_eq!(
            validate_mcp_server_zip(dir.path()).unwrap(),
            DetectedRuntime::Unknown
        );
    }
}
