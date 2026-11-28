//! The two eval images, written as Dockerfile text.
//!
//! * **Env image**, one per repo spec: the repo's toolchain plus Node 22 and opencode, at the same
//!   pinned opencode version as the platform's OpenCode harness image
//!   (`oss/agents/harnesses/opencode/Dockerfile`, read at compile time, so the two never drift).
//! * **Task image**, one per task: the repo at the task's base commit as a fresh one-commit git
//!   repository in `/testbed` (no later history, so the real fix cannot be looked up), with
//!   dependencies fetched and the affected crates already built.

use std::fmt::Write as _;

use crate::spec::{self, EnvSpec};
use sha2::{Digest, Sha256};

/// The OpenCode harness image's Dockerfile; its `ARG OPENCODE_VERSION=` is the pin.
const HARNESS_DOCKERFILE: &str = include_str!("../../agents/harnesses/opencode/Dockerfile");

/// The opencode version the platform's OpenCode harness pins.
pub fn opencode_version() -> &'static str {
    HARNESS_DOCKERFILE
        .lines()
        .find_map(|l| l.trim().strip_prefix("ARG OPENCODE_VERSION="))
        .map(str::trim)
        .expect("the OpenCode harness Dockerfile pins ARG OPENCODE_VERSION")
}

/// Node image opencode's npm package is installed from (the harness image's base).
const NODE_IMAGE: &str = "node:22-bookworm-slim";

/// Cargo without debug info or incremental state: most of a `target/` directory is both, and
/// the warm build is baked into the task image, so this is what keeps the image, the builder's
/// memory and disk, and every test run's disk small. Set in the image, so the warm build and the
/// runs that reuse it build with the same profile.
const CARGO_LEAN_ENV: &str = "ENV CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 \
                              CARGO_PROFILE_TEST_DEBUG=0\n";

/// Python without bytecode files or pip's cache and version check: nothing a test run or the
/// agent does should leave files behind in `/testbed`.
const PYTHON_LEAN_ENV: &str = "ENV PYTHONDONTWRITEBYTECODE=1 PIP_NO_CACHE_DIR=1 \
                               PIP_DISABLE_PIP_VERSION_CHECK=1\n";

/// Files Python tooling writes into a checkout (bytecode, an editable install's metadata, tool
/// caches, virtualenvs). The agent's patch is `git add -A` of `/testbed`, so without this an
/// `.egg-info` the image's own `pip install -e .` wrote would land in the patch, and the grader,
/// whose image already has it, could not apply a correct fix.
const PYTHON_GIT_EXCLUDES: [&str; 10] = [
    "__pycache__/",
    "*.py[cod]",
    "*.egg-info/",
    ".eggs/",
    "build/",
    ".pytest_cache/",
    ".mypy_cache/",
    ".ruff_cache/",
    ".tox/",
    ".venv/",
];

/// Compile jobs for the warm build: it runs on a shared image builder with little memory, where
/// a large workspace at full parallelism runs out of it. Test runs keep their own CPUs.
const WARM_BUILD_JOBS: u32 = 2;

pub fn env_dockerfile(spec: &EnvSpec) -> String {
    let mut packages = vec!["git", "ca-certificates", "curl", "ripgrep"];
    packages.extend(spec.system_packages.iter().map(String::as_str));
    let lean = match spec.language {
        spec::Language::Rust => CARGO_LEAN_ENV,
        spec::Language::Python => PYTHON_LEAN_ENV,
    };
    format!(
        "FROM {toolchain}\n\
         RUN apt-get update \\\n    && apt-get install -y --no-install-recommends {packages} \\\n    && rm -rf /var/lib/apt/lists/*\n\
         # Node and opencode, as in the platform's OpenCode harness image.\n\
         COPY --from={NODE_IMAGE} /usr/local/bin/node /usr/local/bin/node\n\
         COPY --from={NODE_IMAGE} /usr/local/lib/node_modules /usr/local/lib/node_modules\n\
         RUN ln -sf ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \\\n    && npm install -g opencode-ai@{version} && npm cache clean --force\n\
         ENV OPENCODE_DISABLE_AUTOUPDATE=1 OPENCODE_DISABLE_MODELS_FETCH=1 \\\n    OPENCODE_DISABLE_DEFAULT_PLUGINS=1 OPENCODE_DISABLE_LSP_DOWNLOAD=1 \\\n    OPENCODE_DISABLE_PROJECT_CONFIG=1\n\
         {lean}",
        toolchain = spec.toolchain_image,
        packages = packages.join(" "),
        version = opencode_version(),
    )
}

/// `source.tar` (the repo at the base commit: a `git archive`, or the forge's tarball, either
/// unpacked into the build root) sits next to this Dockerfile.
pub fn task_dockerfile(env_image: &str, spec: &EnvSpec, scope: &[String]) -> String {
    let mut df = format!(
        "FROM {env_image}\n\
         WORKDIR /testbed\n\
         COPY source.tar /tmp/source.tar\n\
         # Unpacked by tar, not ADD: ADD follows symlinks in the archive.\n\
         RUN tar -xf /tmp/source.tar -C /testbed && rm /tmp/source.tar\n\
         RUN git init -q && git add -A \\\n    && git -c user.name=eval -c user.email=eval@nasiko.local commit -qm base \\\n    && git config --global --add safe.directory /testbed\n"
    );
    if matches!(spec.language, spec::Language::Python) {
        // Ignored, not deleted: a tracked file that matches is still part of the repo.
        let _ = writeln!(
            df,
            "RUN printf '%s\\n' {} >> .git/info/exclude",
            PYTHON_GIT_EXCLUDES
                .iter()
                .map(|p| format!("'{p}'"))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    for cmd in &spec.setup {
        let _ = writeln!(df, "RUN {cmd}");
    }
    // Warm the build of the affected crates at the base commit; a failure here is fine (the
    // gate will report it), it only means the cache is colder.
    let jobs = if is_cargo(spec) {
        format!("CARGO_BUILD_JOBS={WARM_BUILD_JOBS} ")
    } else {
        String::new()
    };
    let _ = writeln!(
        df,
        "RUN {jobs}{} || true",
        spec::with_scope(&spec.build_cmd, scope)
    );
    // No `CARGO_NET_OFFLINE` here: the containers that use this image already have no network,
    // and changing that variable after the warm build makes build scripts that watch it (such as
    // `ort-sys`, which downloads ONNX Runtime) run again offline and fail to link.
    df.push_str("ENV HOME=/root\n");
    df
}

/// The env and task images as one Dockerfile, for builders that cannot base one local build
/// on another (a cluster's BuildKit pulls `FROM` images from a registry). The env steps come
/// first, so the builder's layer cache still shares them between a repo's tasks.
pub fn combined_dockerfile(spec: &EnvSpec, scope: &[String]) -> String {
    let task = task_dockerfile("env", spec, scope);
    let steps = task.split_once('\n').map_or("", |(_, rest)| rest);
    format!("{}{steps}", env_dockerfile(spec))
}

pub fn env_image_tag(spec: &EnvSpec) -> String {
    format!(
        "nasiko-eval/env-{}:{}",
        slug(&spec.repo),
        short_hash(&env_dockerfile(spec))
    )
}

pub fn task_image_tag(spec: &EnvSpec, base_commit: &str, scope: &[String]) -> String {
    let base = base_commit.get(..12).unwrap_or(base_commit);
    format!(
        "nasiko-eval/task-{}:{base}-{}",
        slug(&spec.repo),
        // The whole recipe, so a change to it (or to the env image) never reuses a stale image.
        short_hash(&task_dockerfile(&env_image_tag(spec), spec, scope))
    )
}

fn is_cargo(spec: &EnvSpec) -> bool {
    matches!(spec.language, spec::Language::Rust)
}

fn slug(repo: &str) -> String {
    repo.rsplit('/')
        .next()
        .unwrap_or(repo)
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn short_hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rust_spec() -> EnvSpec {
        spec::propose("Nasiko-Labs/nasiko-cloud-rs", &["Cargo.toml".into()], None).expect("rust")
    }

    #[test]
    fn opencode_pin_comes_from_the_harness_dockerfile() {
        let v = opencode_version();
        assert!(v.chars().next().is_some_and(|c| c.is_ascii_digit()), "{v}");
        assert!(env_dockerfile(&rust_spec()).contains(&format!("opencode-ai@{v}")));
    }

    #[test]
    fn task_image_is_a_fresh_repo_with_warm_build() {
        let df = task_dockerfile("env:1", &rust_spec(), &["-p nasiko-compress".into()]);
        assert!(df.starts_with("FROM env:1\n"));
        assert!(df.contains("git init -q"));
        assert!(df.contains("RUN cargo fetch"));
        assert!(
            df.contains("RUN CARGO_BUILD_JOBS=2 cargo test -p nasiko-compress --no-run || true")
        );
        // Changing it after the warm build would re-run build scripts that watch it, offline.
        assert!(!df.contains("CARGO_NET_OFFLINE"));
    }

    #[test]
    fn rust_images_build_without_debug_info_or_incremental_state() {
        let env = env_dockerfile(&rust_spec());
        assert!(env.contains("CARGO_INCREMENTAL=0"));
        assert!(env.contains("CARGO_PROFILE_TEST_DEBUG=0"));
    }

    #[test]
    fn python_images_keep_tool_output_out_of_the_agent_patch() {
        let spec = spec::propose("o/dur", &["pyproject.toml".into()], None).expect("python");
        let env = env_dockerfile(&spec);
        assert!(env.starts_with("FROM python:3.12-slim-bookworm\n"));
        assert!(env.contains("PYTHONDONTWRITEBYTECODE=1"));
        assert!(!env.contains("CARGO_INCREMENTAL"));
        let df = task_dockerfile("env:1", &spec, &["'tests/test_a.py'".into()]);
        let exclude = df.find(".git/info/exclude").expect("excludes tool output");
        let setup = df.find("pip install -e").expect("installs the package");
        // Excluded before the editable install writes its `.egg-info`.
        assert!(df.find("git init -q").unwrap() < exclude && exclude < setup);
        assert!(df.contains("'*.egg-info/'") && df.contains("'__pycache__/'"));
        assert!(df.contains("for f in 'tests/test_a.py'; do") && df.contains(") || true"));
        assert!(!df.contains("CARGO_BUILD_JOBS"));
        let rust = task_dockerfile("env:1", &rust_spec(), &["-p a".into()]);
        assert!(!rust.contains(".git/info/exclude"));
    }

    #[test]
    fn the_combined_recipe_is_env_then_task_steps() {
        let s = rust_spec();
        let df = combined_dockerfile(&s, &["-p a".into()]);
        assert!(df.starts_with(&env_dockerfile(&s)));
        assert_eq!(
            df.matches("FROM ").count(),
            env_dockerfile(&s).matches("FROM ").count()
        );
        assert!(df.contains("COPY source.tar /tmp/source.tar"));
        assert!(df.ends_with("ENV HOME=/root\n"));
    }

    #[test]
    fn tags_name_their_contents() {
        let s = rust_spec();
        assert!(env_image_tag(&s).starts_with("nasiko-eval/env-nasiko-cloud-rs:"));
        let a = task_image_tag(&s, "0123456789abcdef", &["-p a".into()]);
        let b = task_image_tag(&s, "0123456789abcdef", &["-p b".into()]);
        assert!(a.starts_with("nasiko-eval/task-nasiko-cloud-rs:0123456789ab-"));
        assert_ne!(a, b);
        let mut t = rust_spec();
        t.setup.push("cargo build".into());
        assert_ne!(a, task_image_tag(&t, "0123456789abcdef", &["-p a".into()]));
    }
}
