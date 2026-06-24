//! Project-type detection — inspects the sandbox root for well-known manifest files and maps
//! to a language + default test command. Used by `run_tests` (and, in Phase 2, to pick the
//! per-language sandbox image).

use crate::sandbox::Sandbox;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    Node,
    Python,
    Go,
    Unknown,
}

impl Language {
    /// The conventional test command for this language, if known.
    pub fn default_test_command(self) -> Option<&'static str> {
        match self {
            Language::Rust => Some("cargo test"),
            Language::Node => Some("npm test"),
            Language::Python => Some("pytest"),
            Language::Go => Some("go test ./..."),
            Language::Unknown => None,
        }
    }
}

/// Detect the project language by probing the sandbox root for manifest files.
///
/// Probes in priority order; the first manifest found wins. Uses `read_file` (cheap, present in
/// the trait) rather than a directory listing so it works identically for remote sandboxes.
pub async fn detect(sandbox: &dyn Sandbox) -> Language {
    // (manifest path, language)
    const MARKERS: &[(&str, Language)] = &[
        ("Cargo.toml", Language::Rust),
        ("package.json", Language::Node),
        ("pyproject.toml", Language::Python),
        ("go.mod", Language::Go),
    ];
    for (marker, lang) in MARKERS {
        if sandbox.read_file(marker, Some((1, 1))).await.is_ok() {
            return *lang;
        }
    }
    // Python without pyproject (setup.py / requirements.txt).
    for marker in ["setup.py", "requirements.txt", "pytest.ini"] {
        if sandbox.read_file(marker, Some((1, 1))).await.is_ok() {
            return Language::Python;
        }
    }
    Language::Unknown
}

/// Resolve the test command for the current project: caller override, else detected default.
pub async fn test_command(sandbox: &dyn Sandbox, override_cmd: Option<&str>) -> Result<String, String> {
    if let Some(cmd) = override_cmd
        && !cmd.trim().is_empty() {
            return Ok(cmd.to_string());
        }
    detect(sandbox)
        .await
        .default_test_command()
        .map(str::to_string)
        .ok_or_else(|| {
            "could not detect project type; pass an explicit `command` to run_tests".to_string()
        })
}
