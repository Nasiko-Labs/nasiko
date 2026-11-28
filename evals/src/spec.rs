//! How to build and test one repository inside the eval container.

use serde::{Deserialize, Serialize};

use crate::diff::{FileDiff, PYTHON_TEST_GLOBS, RUST_TEST_GLOBS};
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
    /// Memory limit of each container that builds the repo (gate, agent, grader), in MiB. A big
    /// workspace needs more than the default: linking two of its test binaries at once can take
    /// 10 GiB or more, and a linker killed for memory reads as code that does not compile. On
    /// Kubernetes the pod also requests this much, so raise it only for the repos that need it.
    #[serde(default = "default_memory_mib")]
    pub memory_mib: u32,
    /// Where the repository is checked out on this machine (images are built from it).
    #[serde(default)]
    pub local_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    Python,
}

/// Default Rust toolchain image when the repo pins none. Debian 13 (trixie), not 12: prebuilt
/// native libraries that crates download at build time (ONNX Runtime via `ort-sys`, for one)
/// need glibc 2.38+ and GCC 14's C++ runtime.
pub const DEFAULT_RUST_IMAGE: &str = "rust:1-trixie";

/// Default Python image: Debian 12 (bookworm), the same Debian as the Node image opencode is
/// copied from, so its `node` binary finds the glibc it was built against.
pub const DEFAULT_PYTHON_IMAGE: &str = "python:3.12-slim-bookworm";

/// Root files that mark a Python project whose tests pytest can run.
const PYTHON_MARKERS: [&str; 6] = [
    "pyproject.toml",
    "setup.py",
    "setup.cfg",
    "requirements.txt",
    "pytest.ini",
    "tox.ini",
];

/// A Python container builds nothing, so it needs far less memory than a Cargo link step.
pub const PYTHON_MEMORY_MIB: u32 = 3072;

/// Proposes a spec from the files at the repo root; `toolchain` is the content of
/// `rust-toolchain.toml` / `rust-toolchain` when present. Cargo wins when a repo has both, as
/// its Python files are then usually bindings or scripts. `None` for unsupported repos.
pub fn propose(repo: &str, root_files: &[String], toolchain: Option<&str>) -> Option<EnvSpec> {
    let has = |name: &str| root_files.iter().any(|f| f == name);
    if has("Cargo.toml") {
        return Some(rust_spec(repo, toolchain));
    }
    if PYTHON_MARKERS.iter().any(|m| has(m)) {
        return Some(python_spec(repo));
    }
    None
}

/// Installs pytest, the repo's requirement files and the package itself (editable, so the
/// tests import `/testbed`'s code, patched or not). Each optional step may fail without
/// stopping the image: the gate then reports a task whose tests cannot run.
const PYTHON_SETUP: &str = "python -m pip install pytest \
     && for f in requirements.txt requirements-dev.txt requirements-test.txt \
     requirements/test.txt requirements/dev.txt; do \
     if [ -f \"$f\" ]; then python -m pip install -r \"$f\" || true; fi; done \
     && if [ -f pyproject.toml ] || [ -f setup.py ]; then \
     python -m pip install -e '.[test,tests,testing,dev]' || python -m pip install -e . || true; fi";

/// Python has no build step; a syntax error in a scoped test file is the closest thing to code
/// that does not compile. Code the tests import but cannot load shows up as their own failures
/// instead. Files the run's set-up lacks (the pristine run has none of the new test files) are
/// skipped; the subshell keeps `exit` from ending the eval script with the wrong code.
const PYTHON_BUILD: &str =
    "( for f in {scope}; do [ ! -e \"$f\" ] || python -m compileall -q \"$f\" || exit 1; done )";

/// Runs pytest on the scoped test files present in the tree: pytest refuses to run at all when
/// one named file is missing, and with none named it would run the whole suite. `-rA` writes one
/// summary line per test, which `testlog` reads; no cache directory, so a run leaves nothing
/// behind in `/testbed`.
const PYTHON_TEST: &str = "( set --; for f in {scope}; do [ ! -e \"$f\" ] || set -- \"$@\" \"$f\"; done; \
     [ $# -eq 0 ] || python -m pytest -rA --color=no --tb=short -p no:cacheprovider \"$@\" )";

/// What the replayed agent is told to run for Python: the whole suite, never the scope, which
/// names the hidden test files.
pub const PYTHON_AGENT_BUILD: &str = "python -m compileall -q .";
pub const PYTHON_AGENT_TEST: &str = "python -m pytest -q";

fn python_spec(repo: &str) -> EnvSpec {
    EnvSpec {
        repo: repo.to_string(),
        language: Language::Python,
        toolchain_image: DEFAULT_PYTHON_IMAGE.to_string(),
        // A C compiler for dependencies that ship no wheel for this Python.
        system_packages: vec!["build-essential".into()],
        setup: vec![PYTHON_SETUP.into()],
        build_cmd: PYTHON_BUILD.into(),
        test_cmd: PYTHON_TEST.into(),
        test_paths: PYTHON_TEST_GLOBS.iter().map(|g| g.to_string()).collect(),
        log_parser: LogParserKind::Pytest,
        timeout_secs: 1800,
        memory_mib: PYTHON_MEMORY_MIB,
        local_path: None,
    }
}

/// The pytest scope of a task: the Python test files its test patch leaves in the tree, each
/// quoted for the shell. `conftest.py` and `__init__.py` hold fixtures, not tests, so they are
/// not named (pytest loads them anyway). A path with a quote or a newline is left out rather
/// than risk breaking the script. `None` when no test file is left: nothing could then show the
/// fix works.
/// Why a task with no test scope is rejected (Cargo: no package owns its files; Python: its
/// test patch leaves no test file).
pub const NO_SCOPE_REASON: &str =
    "no test file or Cargo package of the task's own, so it has no tests to run";

pub fn python_test_scope(test_files: &[FileDiff]) -> Option<Vec<String>> {
    let mut scope: Vec<String> = test_files
        .iter()
        .filter_map(|f| f.new_path.as_deref())
        .filter(|p| p.ends_with(".py"))
        .filter(|p| {
            let name = p.rsplit('/').next().unwrap_or(p);
            name != "conftest.py" && name != "__init__.py"
        })
        .filter(|p| !p.contains(['\'', '\n']))
        .map(|p| format!("'{p}'"))
        .collect();
    scope.sort();
    scope.dedup();
    (!scope.is_empty()).then_some(scope)
}

fn rust_spec(repo: &str, toolchain: Option<&str>) -> EnvSpec {
    let image = toolchain
        .and_then(rust_channel)
        .map(|v| format!("rust:{v}-trixie"))
        .unwrap_or_else(|| DEFAULT_RUST_IMAGE.to_string());
    EnvSpec {
        repo: repo.to_string(),
        language: Language::Rust,
        toolchain_image: image,
        // `libprotobuf-dev` ships protobuf's well-known types (`google/protobuf/*.proto`), which
        // build scripts compiling `.proto` files import (`lance-encoding`, for one).
        system_packages: vec![
            "pkg-config".into(),
            "libssl-dev".into(),
            "protobuf-compiler".into(),
            "libprotobuf-dev".into(),
        ],
        setup: vec!["cargo fetch".into()],
        build_cmd: "cargo test {scope} --no-run".into(),
        test_cmd: "cargo test {scope} --no-fail-fast".into(),
        test_paths: RUST_TEST_GLOBS.iter().map(|g| g.to_string()).collect(),
        log_parser: LogParserKind::Cargo,
        timeout_secs: 1800,
        memory_mib: DEFAULT_MEMORY_MIB,
        local_path: None,
    }
}

/// A build container's memory limit unless the spec says otherwise.
pub const DEFAULT_MEMORY_MIB: u32 = 6144;

fn default_memory_mib() -> u32 {
    DEFAULT_MEMORY_MIB
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

/// A file a task's fix or tests change, with the Cargo package that owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedFile {
    /// The file, from the repo root.
    pub path: String,
    /// The owning package's name.
    pub package: String,
    /// The owning package's `Cargo.toml`, from the repo root.
    pub manifest: String,
    /// Whether the change is in the task's tests (the test patch) rather than its fix.
    pub in_tests: bool,
}

/// The Cargo test scope of a task: the packages its fix and tests change ([`cargo_scope`]), or,
/// when they are one package and every test file is one of its integration tests
/// (`tests/<name>.rs` or `tests/<name>/main.rs`), just those test targets. A package's
/// integration tests are each a binary linking the whole package, so building all of them for a
/// task that changes one can outgrow the image builder; SWE-bench runs only the tests a task
/// names, too. `None` when no package owns any of the files: the task's tests would then be the
/// whole workspace's, which proves nothing about the task and can outgrow any builder.
pub fn task_test_scope(files: &[OwnedFile]) -> Option<Vec<String>> {
    let mut packages: Vec<String> = files.iter().map(|f| f.package.clone()).collect();
    packages.sort();
    packages.dedup();
    if packages.is_empty() {
        return None;
    }
    if let [package] = packages.as_slice() {
        let targets: Option<Vec<String>> = files
            .iter()
            .filter(|f| f.in_tests)
            .map(integration_test_target)
            .collect();
        if let Some(mut targets) = targets.filter(|t| !t.is_empty()) {
            targets.sort();
            targets.dedup();
            let mut scope = vec![format!("-p {package}")];
            scope.extend(targets.iter().map(|t| format!("--test {t}")));
            return Some(scope);
        }
    }
    Some(cargo_scope(&packages))
}

/// The integration test target `file` is, when it is one: `tests/<name>.rs` or
/// `tests/<name>/main.rs` under its package's folder. A shared helper (`tests/common/mod.rs`)
/// or an inline test module is none.
fn integration_test_target(file: &OwnedFile) -> Option<String> {
    let dir = file
        .manifest
        .strip_suffix("Cargo.toml")?
        .trim_end_matches('/');
    let rel = if dir.is_empty() {
        file.path.as_str()
    } else {
        file.path.strip_prefix(dir)?.strip_prefix('/')?
    };
    let rest = rel.strip_prefix("tests/")?;
    let name = match rest.split('/').collect::<Vec<_>>().as_slice() {
        [file] => file.strip_suffix(".rs")?,
        [name, "main.rs"] => name,
        _ => return None,
    };
    (!name.is_empty()).then(|| name.to_string())
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

    fn owned(path: &str, package: &str, manifest: &str, in_tests: bool) -> OwnedFile {
        OwnedFile {
            path: path.into(),
            package: package.into(),
            manifest: manifest.into(),
            in_tests,
        }
    }

    #[test]
    fn a_task_tested_by_integration_tests_builds_only_those() {
        let server = "oss/server/Cargo.toml";
        let files = [
            owned("oss/server/src/pricing.rs", "nasiko-server", server, false),
            owned(
                "oss/server/tests/pricing_sync.rs",
                "nasiko-server",
                server,
                true,
            ),
            owned(
                "oss/server/tests/budgets/main.rs",
                "nasiko-server",
                server,
                true,
            ),
        ];
        assert_eq!(
            task_test_scope(&files).unwrap(),
            ["-p nasiko-server", "--test budgets", "--test pricing_sync"]
        );
    }

    #[test]
    fn inline_tests_shared_helpers_and_several_packages_keep_the_package_scope() {
        let server = "oss/server/Cargo.toml";
        let inline = [owned("oss/server/src/a.rs", "nasiko-server", server, true)];
        assert_eq!(task_test_scope(&inline).unwrap(), ["-p nasiko-server"]);
        let helper = [owned(
            "oss/server/tests/common/mod.rs",
            "nasiko-server",
            server,
            true,
        )];
        assert_eq!(task_test_scope(&helper).unwrap(), ["-p nasiko-server"]);
        let two = [
            owned("oss/server/tests/a.rs", "nasiko-server", server, true),
            owned("oss/cli/src/b.rs", "nasiko", "oss/cli/Cargo.toml", false),
        ];
        assert_eq!(
            task_test_scope(&two).unwrap(),
            ["-p nasiko", "-p nasiko-server"]
        );
        let root = [owned("tests/smoke.rs", "app", "Cargo.toml", true)];
        assert_eq!(task_test_scope(&root).unwrap(), ["-p app", "--test smoke"]);
    }

    #[test]
    fn a_task_no_package_owns_has_no_scope_not_the_whole_workspace() {
        assert_eq!(task_test_scope(&[]), None);
    }

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
    fn proposes_pytest_for_python_repos_and_cargo_when_both() {
        for marker in PYTHON_MARKERS {
            let spec = propose("o/r", &[marker.into(), "README.md".into()], None).expect(marker);
            assert_eq!(spec.language, Language::Python, "{marker}");
            assert_eq!(spec.log_parser, LogParserKind::Pytest);
            assert_eq!(spec.toolchain_image, DEFAULT_PYTHON_IMAGE);
            assert!(spec.test_cmd.contains("pytest -rA"));
        }
        let both = propose("o/r", &["Cargo.toml".into(), "pyproject.toml".into()], None);
        assert_eq!(both.expect("rust").language, Language::Rust);
        assert!(propose("o/r", &["main.py".into()], None).is_none());
    }

    fn file(old: Option<&str>, new: Option<&str>) -> FileDiff {
        FileDiff {
            old_path: old.map(Into::into),
            new_path: new.map(Into::into),
            text: String::new(),
            added: Vec::new(),
        }
    }

    #[test]
    fn python_scope_names_the_test_files_left_quoted_and_sorted() {
        let files = [
            file(None, Some("tests/test_b.py")),
            file(Some("tests/test_a.py"), Some("tests/test_a.py")),
            file(Some("tests/test_gone.py"), None),
            file(None, Some("tests/conftest.py")),
            file(None, Some("tests/__init__.py")),
            file(None, Some("tests/data/case.json")),
            file(None, Some("tests/it's.py")),
        ];
        assert_eq!(
            python_test_scope(&files).unwrap(),
            ["'tests/test_a.py'", "'tests/test_b.py'"]
        );
        let fixtures_only = [file(None, Some("tests/conftest.py"))];
        assert_eq!(python_test_scope(&fixtures_only), None);
    }

    #[test]
    fn python_script_checks_syntax_then_runs_pytest_on_the_scope() {
        let spec = propose("o/r", &["pyproject.toml".into()], None).expect("python");
        let script = eval_script(&spec, &["'tests/test_a.py'".into()]);
        let build = script.find("compileall -q \"$f\"").expect("builds");
        let test = script.find("pytest -rA").expect("tests");
        assert!(build < test);
        assert!(script.contains("for f in 'tests/test_a.py'; do"));
        assert!(script.contains("-p no:cacheprovider \"$@\""));
    }

    #[test]
    fn a_spec_saved_before_memory_mib_reads_the_default() {
        let mut saved = serde_json::to_value(propose("o/r", &["Cargo.toml".into()], None)).unwrap();
        saved.as_object_mut().unwrap().remove("memory_mib");
        let spec: EnvSpec = serde_json::from_value(saved).expect("old spec");
        assert_eq!(spec.memory_mib, DEFAULT_MEMORY_MIB);
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
