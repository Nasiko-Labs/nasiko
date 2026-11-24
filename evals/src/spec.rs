//! How to build and test one repository inside the eval container.

use serde::{Deserialize, Serialize};

use crate::testlog::LogParserKind;

/// Build and test settings for one repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvSpec {
    pub repo: String,
    pub language: Language,
    /// Image the eval image starts from (the toolchain), e.g. `rust:1.92-trixie`.
    pub toolchain_image: String,
    /// Extra Debian packages the build needs.
    #[serde(default)]
    pub system_packages: Vec<String>,
    /// Commands run while the task image is built, with network on (e.g. `cargo fetch`).
    #[serde(default)]
    pub setup: Vec<String>,
    /// Test command; `{scope}` is replaced by the task's test scope (e.g. `-p crate_a -p crate_b`).
    pub test_cmd: String,
    /// Compile-only command run before the tests; same `{scope}` placeholder.
    pub build_cmd: String,
    /// Globs of test files.
    pub test_paths: Vec<String>,
    pub log_parser: LogParserKind,
    /// Wall-clock limit for one test run.
    pub timeout_secs: u64,
    /// Where the repository is checked out on this machine (images are built from it).
    #[serde(default)]
    pub local_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
}

/// Default Rust toolchain image when the repo pins none. Debian 13 (trixie), not 12: prebuilt
/// native libraries that crates download at build time (ONNX Runtime via `ort-sys`, for one)
/// need glibc 2.38+ and GCC 14's C++ runtime.
pub const DEFAULT_RUST_IMAGE: &str = "rust:1-trixie";

/// Proposes a spec from the files at the repo root; `toolchain` is the content of
/// `rust-toolchain.toml` / `rust-toolchain` when present. `None` for unsupported repos.
pub fn propose(repo: &str, root_files: &[String], toolchain: Option<&str>) -> Option<EnvSpec> {
    if !root_files.iter().any(|f| f == "Cargo.toml") {
        return None;
    }
    let image = toolchain
        .and_then(rust_channel)
        .map(|v| format!("rust:{v}-trixie"))
        .unwrap_or_else(|| DEFAULT_RUST_IMAGE.to_string());
    Some(EnvSpec {
        repo: repo.to_string(),
        language: Language::Rust,
        toolchain_image: image,
        system_packages: vec![
            "pkg-config".into(),
            "libssl-dev".into(),
            "protobuf-compiler".into(),
        ],
        setup: vec!["cargo fetch".into()],
        build_cmd: "cargo test {scope} --no-run".into(),
        test_cmd: "cargo test {scope} --no-fail-fast".into(),
        test_paths: vec![
            "**/tests/**".into(),
            "**/*_test.rs".into(),
            "**/*_tests.rs".into(),
            "**/tests.rs".into(),
        ],
        log_parser: LogParserKind::Cargo,
        timeout_secs: 1800,
        local_path: None,
    })
}

/// The pinned version from `rust-toolchain(.toml)`: `channel = "1.92"` or a bare `1.92.0`.
/// Named channels (`stable`, `nightly-…`) give `None`.
pub fn rust_channel(text: &str) -> Option<String> {
    let value = text
        .lines()
        .map(str::trim)
        .find_map(|l| {
            l.strip_prefix("channel")
                .map(|r| r.trim_start_matches([' ', '=']))
        })
        .unwrap_or_else(|| text.trim())
        .trim_matches(['"', '\'', ' ']);
    value
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit())
        .then(|| value.to_string())
}

/// Fills `{scope}` in a command.
pub fn with_scope(cmd: &str, scope: &[String]) -> String {
    cmd.replace("{scope}", &scope.join(" "))
}

/// Cargo test scope for a set of package names: `-p a -p b`, or `--workspace` when empty.
pub fn cargo_scope(packages: &[String]) -> Vec<String> {
    if packages.is_empty() {
        return vec!["--workspace".into()];
    }
    packages.iter().map(|p| format!("-p {p}")).collect()
}

/// The `Cargo.toml` paths that may own `path`, nearest first: each folder above it up to the
/// repo root. The first one that exists names the file's package.
pub fn manifests_above(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut dir = std::path::Path::new(path).parent();
    while let Some(d) = dir {
        out.push(if d.as_os_str().is_empty() {
            "Cargo.toml".to_string()
        } else {
            format!("{}/Cargo.toml", d.display())
        });
        dir = d.parent();
    }
    out
}

/// Exit codes of [`eval_script`] that are not the test command's own.
pub const EXIT_MODEL_PATCH: i32 = 10;
pub const EXIT_TEST_PATCH: i32 = 11;
pub const EXIT_BUILD: i32 = 12;

/// The grading script run in a fresh container. It applies `/eval/model.patch` (when present),
/// then `/eval/test.patch`, compiles, and runs the tests. SWE-bench keeps the same script per
/// row as `eval_script`.
pub fn eval_script(spec: &EnvSpec, scope: &[String]) -> String {
    format!(
        "#!/bin/bash\nset -uo pipefail\ncd /testbed\n\
         if [ -s /eval/model.patch ]; then git apply --whitespace=nowarn /eval/model.patch || exit {EXIT_MODEL_PATCH}; fi\n\
         if [ -s /eval/test.patch ]; then git apply --whitespace=nowarn /eval/test.patch || exit {EXIT_TEST_PATCH}; fi\n\
         {build} || exit {EXIT_BUILD}\n\
         {test}\n",
        build = with_scope(&spec.build_cmd, scope),
        test = with_scope(&spec.test_cmd, scope),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifests_are_listed_nearest_first() {
        assert_eq!(
            manifests_above("oss/x/src/lib.rs"),
            vec![
                "oss/x/src/Cargo.toml",
                "oss/x/Cargo.toml",
                "oss/Cargo.toml",
                "Cargo.toml"
            ]
        );
        assert_eq!(manifests_above("lib.rs"), vec!["Cargo.toml"]);
    }

    #[test]
    fn reads_pinned_channels_only() {
        assert_eq!(
            rust_channel("[toolchain]\nchannel = \"1.92\"\n").as_deref(),
            Some("1.92")
        );
        assert_eq!(rust_channel("1.90.0\n").as_deref(), Some("1.90.0"));
        assert_eq!(rust_channel("[toolchain]\nchannel = \"stable\"\n"), None);
    }

    #[test]
    fn proposes_rust_for_cargo_repos() {
        let spec =
            propose("o/r", &["Cargo.toml".into()], Some("channel = \"1.92\"")).expect("rust");
        assert_eq!(spec.toolchain_image, "rust:1.92-trixie");
        assert!(propose("o/r", &["package.json".into()], None).is_none());
    }

    #[test]
    fn script_applies_patches_then_builds_then_tests() {
        let spec = propose("o/r", &["Cargo.toml".into()], None).expect("rust");
        let script = eval_script(&spec, &cargo_scope(&["nasiko-compress".into()]));
        let apply = script.find("model.patch").expect("applies model patch");
        let build = script.find("--no-run").expect("builds");
        let test = script.find("--no-fail-fast").expect("tests");
        assert!(apply < build && build < test);
        assert!(script.contains("cargo test -p nasiko-compress --no-fail-fast"));
        assert_eq!(cargo_scope(&[]), vec!["--workspace"]);
    }
}
