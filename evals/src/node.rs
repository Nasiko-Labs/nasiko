//! JavaScript / TypeScript (Node) repositories: the build and test settings, the test scope of a
//! task, and which files are test files.
//!
//! A task's scope is one package (the folder of the nearest `package.json` above its test files)
//! and the test files it changes, relative to that folder: `['server', 'test/finch.test.ts']`.
//! The commands read it back with `set -- {scope}`. Each test file runs on its own, each writing
//! its own JUnit report: node:test's report names no file, so two files' tests of the same name
//! would otherwise be one. The reports are printed after every test has run, each under a
//! [`JUNIT_FILE`] line, and read by [`crate::testlog`].
//!
//! The runner is chosen from the package's `test` script *at the base commit* (`git show
//! HEAD:./package.json`; the task image's only commit), never the working tree, so a patch that
//! rewrites the script cannot pick how it is graded: vitest, tsx's `--test`, or `node --test`.
//!
//! There is no type-check step. tsx and vitest strip types without checking them, so a type
//! error that does not break the code at runtime fails no test; requiring `tsc --noEmit` would
//! fail correct fixes for an unrelated type error, and reject every task of a repo whose base
//! commit does not type-check. The build step only checks that the package's dependencies are
//! installed.

use crate::spec::{DEFAULT_MEMORY_MIB, EnvSpec, Language, OwnedFile};
use crate::testlog::{JUNIT_BEGIN, JUNIT_END, JUNIT_FILE, LogParserKind};

/// Node used for the toolchain image: the same major as the opencode harness's.
pub const NODE_IMAGE: &str = "node:22-bookworm";

/// Where the per-file JUnit reports go. Outside `/testbed`, so never part of a patch, and
/// emptied before every run, so a stale report is never read.
const JUNIT_DIR: &str = "/tmp/nasiko-junit";

/// Reads the scope: the package folder into `$pkg`, its test files into `"$@"`.
const READ_SCOPE: &str = "set -- {scope}; pkg=$1; shift; cd \"$pkg\"";

/// Installs the package's dependencies while the task image is built (the only step with
/// network), with the package manager its lockfile names. A lockfile at the repo root covers a
/// workspace package. Corepack fetches the version `packageManager` pins, now, so it is there
/// offline later.
const SETUP_INSTALL: &str = "set -- {scope}; cd \"$1\" || exit 1; \
     corepack enable >/dev/null 2>&1 || true; \
     if [ -f pnpm-lock.yaml ] || [ -f /testbed/pnpm-lock.yaml ]; then pnpm install --frozen-lockfile; \
     elif [ -f yarn.lock ] || [ -f /testbed/yarn.lock ]; then yarn install --frozen-lockfile; \
     elif [ -f package-lock.json ]; then npm ci; \
     else npm install; fi";

/// Defines `run_one <file> <report>` for the runner the base commit's `test` script uses.
const PICK_RUNNER: &str = "script=$(git show HEAD:./package.json 2>/dev/null | node -e \
     'let s=\"\";process.stdin.on(\"data\",d=>s+=d).on(\"end\",()=>{try{console.log(JSON.parse(s).scripts?.test??\"\")}catch{console.log(\"\")}})'); \
     case \"$script\" in \
     *vitest*) run_one() { ./node_modules/.bin/vitest run --reporter=default --reporter=junit --outputFile=\"$2\" \"$1\"; } ;; \
     *tsx*) run_one() { ./node_modules/.bin/tsx --test --test-reporter=spec --test-reporter-destination=stdout --test-reporter=junit --test-reporter-destination=\"$2\" \"$1\"; } ;; \
     *) run_one() { node --test --test-reporter=spec --test-reporter-destination=stdout --test-reporter=junit --test-reporter-destination=\"$2\" \"$1\"; } ;; \
     esac";

/// What the replayed agent is told builds and tests the code. Never the graded commands: those
/// name the hidden test files and the markers around the reports the grader reads.
pub const AGENT_BUILD_CMD: &str = "npm run --if-present build";
pub const AGENT_TEST_CMD: &str = "npm test";

/// Files that count as test files: test modules and folders, plus the runner configs and setup
/// files that change how tests run or report. A model's edits to those are dropped before
/// grading, so a setup file cannot turn a failure into a pass.
pub fn test_paths() -> Vec<String> {
    [
        "**/*.test.*",
        "**/*.spec.*",
        "**/test/**",
        "**/tests/**",
        "**/__tests__/**",
        "**/vitest.config.*",
        "**/vitest.workspace.*",
        "**/jest.config.*",
        "**/jest.setup.*",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// The settings for a Node repo.
pub fn propose(repo: &str) -> EnvSpec {
    EnvSpec {
        repo: repo.to_string(),
        language: Language::Node,
        toolchain_image: NODE_IMAGE.to_string(),
        system_packages: Vec::new(),
        setup: vec![SETUP_INSTALL.into()],
        build_cmd: format!("{READ_SCOPE} && [ -d node_modules ]"),
        test_cmd: format!(
            "{READ_SCOPE} || exit 1; {PICK_RUNNER}; \
             rm -rf {JUNIT_DIR}; mkdir -p {JUNIT_DIR}; status=0; i=0; \
             for t in \"$@\"; do i=$((i+1)); [ -e \"$t\" ] || continue; \
             run_one \"$t\" \"{JUNIT_DIR}/$i.xml\" 2>&1 || status=1; done; \
             echo '{JUNIT_BEGIN}'; i=0; \
             for t in \"$@\"; do i=$((i+1)); echo \"{JUNIT_FILE}$t\"; \
             cat \"{JUNIT_DIR}/$i.xml\" 2>/dev/null; echo; done; \
             echo '{JUNIT_END}'; exit $status"
        ),
        test_paths: test_paths(),
        log_parser: LogParserKind::Junit,
        timeout_secs: 900,
        memory_mib: DEFAULT_MEMORY_MIB,
        local_path: None,
    }
}

/// A file a JS/TS test runner runs: `*.test.*`, `*.spec.*`, `*-test.*`, `*_test.*`, or any
/// script under `__tests__/`, with a JS or TS extension. Helpers under `test/` are not.
pub fn is_test_module(path: &str) -> bool {
    const EXTENSIONS: [&str; 8] = ["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];
    let name = path.rsplit('/').next().unwrap_or(path);
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    if !EXTENSIONS.contains(&extension) {
        return false;
    }
    let marked = [".test", ".spec", "-test", "_test"]
        .iter()
        .any(|m| stem.ends_with(m));
    marked || path.starts_with("__tests__/") || path.contains("/__tests__/")
}

/// A task's scope: the package owning its changed test files (`OwnedFile::manifest` is that
/// package's `package.json`) and those files relative to it, all shell-quoted. `None` when the
/// test patch changes no test file, or test files of several packages: one run covers one
/// package.
pub fn test_scope(files: &[OwnedFile]) -> Option<Vec<String>> {
    let tests: Vec<&OwnedFile> = files
        .iter()
        .filter(|f| f.in_tests && is_test_module(&f.path))
        .collect();
    let first = tests.first()?;
    if tests.iter().any(|f| f.manifest != first.manifest) {
        return None;
    }
    let dir = first
        .manifest
        .strip_suffix("package.json")?
        .trim_end_matches('/');
    let mut relative: Vec<String> = tests
        .iter()
        .filter_map(|f| match dir {
            "" => Some(f.path.as_str()),
            dir => f.path.strip_prefix(dir)?.strip_prefix('/'),
        })
        .map(shell_quote)
        .collect();
    relative.sort();
    relative.dedup();
    let mut scope = vec![shell_quote(if dir.is_empty() { "." } else { dir })];
    scope.extend(relative);
    Some(scope)
}

/// The package folder of a scope from [`test_scope`], unquoted: `server`, or `.`.
pub fn scope_package(scope: &[String]) -> Option<String> {
    let quoted = scope.first()?;
    Some(quoted.trim_matches('\'').replace("'\\''", "'").to_string())
}

/// `text` as one single-quoted shell word.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(path: &str, manifest: &str, in_tests: bool) -> OwnedFile {
        OwnedFile {
            path: path.into(),
            package: "tatva-work-server".into(),
            manifest: manifest.into(),
            in_tests,
        }
    }

    #[test]
    fn test_modules_are_told_from_helpers() {
        for path in [
            "server/test/finch.test.ts",
            "src/a.spec.tsx",
            "lib/b-test.mjs",
            "src/__tests__/c.js",
        ] {
            assert!(is_test_module(path), "{path}");
        }
        for path in [
            "server/test/helpers.ts",
            "src/testing.ts",
            "server/test/data.test.json",
            "README.test.md",
        ] {
            assert!(!is_test_module(path), "{path}");
        }
    }

    #[test]
    fn the_scope_is_the_package_and_its_changed_test_files() {
        let manifest = "server/package.json";
        let files = [
            owned("server/src/finch.ts", manifest, false),
            owned("server/test/finch.test.ts", manifest, true),
            owned("server/test/helpers.ts", manifest, true),
            owned("server/test/llm.test.ts", manifest, true),
        ];
        assert_eq!(
            test_scope(&files).expect("one package"),
            ["'server'", "'test/finch.test.ts'", "'test/llm.test.ts'"]
        );
        let root = [owned("test/a.test.js", "package.json", true)];
        assert_eq!(
            test_scope(&root).expect("root"),
            ["'.'", "'test/a.test.js'"]
        );
        assert_eq!(
            scope_package(&["'server'".into()]).as_deref(),
            Some("server")
        );
    }

    #[test]
    fn tests_of_several_packages_or_none_give_no_scope() {
        let two = [
            owned("server/test/a.test.ts", "server/package.json", true),
            owned("web/a.test.ts", "web/package.json", true),
        ];
        assert_eq!(test_scope(&two), None);
        let helpers_only = [owned("server/test/helpers.ts", "server/package.json", true)];
        assert_eq!(test_scope(&helpers_only), None);
    }

    #[test]
    fn runner_configs_and_test_folders_count_as_test_files() {
        let rules = crate::TestPathRules::from_globs(&test_paths());
        for path in [
            "server/test/finch.test.ts",
            "server/test/helpers.ts",
            "vitest.config.ts",
            "web/src/__tests__/a.tsx",
            "src/a.spec.ts",
        ] {
            assert!(rules.is_test(path), "{path}");
        }
        assert!(!rules.is_test("server/src/finch.ts"));
        assert!(!rules.is_test("server/package.json"));
    }

    #[test]
    fn the_test_command_runs_each_file_then_prints_every_report() {
        let spec = propose("o/r");
        let scope = ["'server'".to_string(), "'test/finch.test.ts'".to_string()];
        let test = crate::spec::with_scope(&spec.test_cmd, &scope);
        assert!(test.starts_with("set -- 'server' 'test/finch.test.ts'; pkg=$1; shift;"));
        // The runner comes from the base commit's script, not the working tree's.
        assert!(test.contains("git show HEAD:./package.json"));
        assert!(test.contains("--test-reporter=junit"));
        let last_run = test.rfind("run_one \"$t\"").expect("runs each file");
        let report = test.find(JUNIT_BEGIN).expect("prints the reports");
        assert!(last_run < report && report < test.find(JUNIT_END).expect("end"));
        assert!(test.trim_end().ends_with("exit $status"));
        assert!(!spec.build_cmd.contains("tsc"));
    }
}
