//! Where a harness session's heavy, regenerable folders go: its package caches and build
//! output, pointed by env at the sandbox's scratch area ([`HARNESS_SCRATCH_DIR`], node-local
//! storage every provider mounts) instead of the durable workspace, which may be a network
//! filesystem that the small-file churn of installs and builds would saturate.
//!
//! Each session gets `<scratch>/<sandbox key name>/`, so sessions that share one pod never mix
//! their caches. Everything else stays where it is: the repository, `HOME` (the harness's own
//! configuration and conversation state) and anything inside the workspace, a project's own
//! `node_modules`, `.venv` or `vendor` included, which are the user's tree. What lives in
//! scratch is lost when the sandbox's pod or container goes, and comes back on the next install
//! or build.
//!
//! The env is the same for every provider: set on the sandbox in env mode, sent with
//! `POST /control/configure` in handshake mode.

use std::collections::HashMap;

use nasiko_runtime::{HARNESS_SCRATCH_DIR, SandboxKey};

/// Each variable a session's tools read for a heavy folder, and the folder under the session's
/// scratch directory it points at. The one list: add a tool here.
const SCRATCH_FOLDERS: [(&str, &str); 8] = [
    // The XDG cache: the harness's own downloads (OpenCode's provider packages), and every tool
    // that follows the spec without a variable of its own.
    ("XDG_CACHE_HOME", "cache"),
    // Node package managers' download caches (not a project's `node_modules`).
    ("npm_config_cache", "npm"),
    ("YARN_CACHE_FOLDER", "yarn"),
    // Python: pip's wheel and HTTP cache, uv's cache.
    ("PIP_CACHE_DIR", "pip"),
    ("UV_CACHE_DIR", "uv"),
    // Rust build output: every crate's `target/` lands here instead of in the repository.
    ("CARGO_TARGET_DIR", "cargo-target"),
    // Go: the build cache, and the module cache (otherwise `$HOME/go/pkg/mod`).
    ("GOCACHE", "go-build"),
    ("GOMODCACHE", "go-mod"),
];

/// Settings the scratch area needs besides the folders, each a default the env can override.
const SCRATCH_DEFAULTS: [(&str, &str); 1] = [
    // uv hard-links packages from its cache into a project's `.venv`, which cannot cross from
    // the scratch area to the workspace (another filesystem): copy, rather than warn and fall
    // back on every install.
    ("UV_LINK_MODE", "copy"),
];

/// The session's own directory in the scratch area: `<scratch>/<key name>`, the name the
/// shared runtime's supervisor gives the same session.
fn session_scratch_dir(key: &SandboxKey) -> String {
    format!("{HARNESS_SCRATCH_DIR}/{}", key.name())
}

/// Adds the [`SCRATCH_FOLDERS`] env to `env`, each under the session's scratch directory, and
/// the [`SCRATCH_DEFAULTS`]. A variable `env` already sets (the profile's, say) keeps its value.
pub fn add_scratch_env(env: &mut HashMap<String, String>, key: &SandboxKey) {
    let dir = session_scratch_dir(key);
    for (name, folder) in SCRATCH_FOLDERS {
        env.entry(name.to_owned())
            .or_insert_with(|| format!("{dir}/{folder}"));
    }
    for (name, value) in SCRATCH_DEFAULTS {
        env.entry(name.to_owned())
            .or_insert_with(|| value.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use nasiko_runtime::WORKSPACE_MOUNT;
    use uuid::Uuid;

    use super::*;

    fn key(n: u128) -> SandboxKey {
        SandboxKey::Session {
            session_id: Uuid::from_u128(n),
        }
    }

    fn scratch_env(key: &SandboxKey) -> HashMap<String, String> {
        let mut env = HashMap::new();
        add_scratch_env(&mut env, key);
        env
    }

    #[test]
    fn every_heavy_folder_points_into_the_sessions_scratch_directory() {
        let key = key(1);
        let env = scratch_env(&key);
        let dir = format!("/scratch/{}", key.name());
        for (name, folder) in [
            ("XDG_CACHE_HOME", "cache"),
            ("npm_config_cache", "npm"),
            ("YARN_CACHE_FOLDER", "yarn"),
            ("PIP_CACHE_DIR", "pip"),
            ("UV_CACHE_DIR", "uv"),
            ("CARGO_TARGET_DIR", "cargo-target"),
            ("GOCACHE", "go-build"),
            ("GOMODCACHE", "go-mod"),
        ] {
            assert_eq!(env[name], format!("{dir}/{folder}"), "{name}");
        }
        assert_eq!(env["UV_LINK_MODE"], "copy");
        assert_eq!(env.len(), SCRATCH_FOLDERS.len() + SCRATCH_DEFAULTS.len());
    }

    #[test]
    fn two_sessions_never_share_a_folder() {
        let (first, second) = (scratch_env(&key(1)), scratch_env(&key(2)));
        for (name, _) in SCRATCH_FOLDERS {
            assert_ne!(second[name], first[name], "{name}");
        }
    }

    #[test]
    fn nothing_moves_into_or_out_of_the_workspace() {
        let env = scratch_env(&key(1));
        for name in ["HOME", "PWD", "TMPDIR", "WORKSPACE_REPO", "NODE_PATH"] {
            assert!(!env.contains_key(name), "{name}");
        }
        for (name, _) in SCRATCH_FOLDERS {
            let path = &env[name];
            assert!(path.starts_with("/scratch/"), "{path}");
            assert!(!path.starts_with(WORKSPACE_MOUNT), "{path}");
        }
    }

    #[test]
    fn a_variable_already_set_keeps_its_value() {
        let mut env = HashMap::from([
            ("GOCACHE".to_owned(), "/elsewhere".to_owned()),
            ("UV_LINK_MODE".to_owned(), "hardlink".to_owned()),
        ]);
        add_scratch_env(&mut env, &key(1));
        assert_eq!(env["GOCACHE"], "/elsewhere");
        assert_eq!(env["UV_LINK_MODE"], "hardlink");
        assert!(env["GOMODCACHE"].starts_with("/scratch/"));
    }

    #[test]
    fn the_scratch_directory_is_named_by_the_sandbox_key() {
        let key = key(3);
        assert_eq!(
            session_scratch_dir(&key),
            format!("/scratch/s-{}", Uuid::from_u128(3))
        );
    }
}
