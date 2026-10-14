//! Chooses the UI bundle this binary embeds and exports its absolute path as
//! `NASIKO_SPA_BUNDLE`, which `main.rs` interpolates into `#[folder]` (rust-embed's
//! `interpolate-folder-path` feature).
//!
//! The UI is the React workspace at `ui/`; this binary serves its OSS app,
//! `ui/oss/app/dist`. Resolution, first match wins:
//!
//! 1. `NASIKO_REACT_DIST_OSS` — an explicitly selected release SPA (release and
//!    image pipelines). It must exist and be a release build of this edition.
//! 2. The workspace's own build, found by walking up from this crate to the
//!    directory holding `ui/pnpm-workspace.yaml`. Walking up, rather than a
//!    literal `../../ui/…`, is what keeps this correct at both depths: this
//!    crate is `oss/server/` here and `server/` in the public repo.
//! 3. The legacy layout (`ui/common/` beside `ui/oss/dist`). Only the public
//!    repo still has it: while React publication is closed, the OSS sync keeps
//!    the public `ui/` frozen at the last legacy UI it published
//!    (scripts/lib/oss-paths.sh). Remove this branch when that gate opens.
//!
//! A bundle that exists is checked: an embedded test, harness or mock-API
//! build would ship a UI that talks to no real server, so it fails the build.
//! A bundle that does not exist yet is replaced by a placeholder page written to
//! `OUT_DIR` — never into the source tree — so a Rust-only contributor can
//! `cargo check` without installing Node. `just build-ui` builds the real one.

use std::path::{Path, PathBuf};

const EDITION: &str = "oss";
const ENV_DIST: &str = "NASIKO_REACT_DIST_OSS";
/// The edition app's build output, relative to the React workspace root.
const APP_DIST: &str = "oss/app/dist";
/// The legacy bundle, relative to the legacy `ui/` root (public repo only).
const LEGACY_DIST: &str = "oss/dist";

const PLACEHOLDER: &str = r#"<!doctype html>
<title>Nasiko UI — not built</title>
<body style="font-family: system-ui; padding: 3rem; line-height: 1.6">
<h1>UI not built</h1>
<p>This placeholder ships when the frontend hasn't been compiled. Build it:</p>
<pre>just build-ui</pre>
<p>then rebuild <code>nasiko-server</code>.</p>
</body>
"#;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed={ENV_DIST}");
    let bundle = resolve_bundle();
    println!("cargo:rustc-env=NASIKO_SPA_BUNDLE={}", bundle.display());
}

fn resolve_bundle() -> PathBuf {
    if let Some(path) = std::env::var_os(ENV_DIST) {
        let dist = PathBuf::from(&path).canonicalize().unwrap_or_else(|e| {
            panic!("{ENV_DIST}={} does not exist: {e}", path.to_string_lossy())
        });
        check_release_spa(&dist);
        println!("cargo:rerun-if-changed={}", dist.display());
        return dist;
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    if let Some(workspace) = find_up(&manifest, |d| {
        d.join("ui/pnpm-workspace.yaml")
            .is_file()
            .then(|| d.join("ui"))
    }) {
        let app = workspace.join(APP_DIST);
        if app.join("index.html").is_file() {
            check_release_spa(&app);
            println!("cargo:rerun-if-changed={}", app.display());
            return app;
        }
        // Watch the app directory, so creating dist/ re-runs this script.
        println!(
            "cargo:rerun-if-changed={}",
            app.parent().expect("has a parent").display()
        );
        return placeholder(&format!("ui/{APP_DIST}"));
    }
    if let Some(legacy) = find_up(&manifest, |d| {
        let ui = d.join("ui");
        ui.join("common").is_dir().then_some(ui)
    }) {
        let dist = legacy.join(LEGACY_DIST);
        if dist.join("index.html").is_file() {
            println!("cargo:rerun-if-changed={}", dist.display());
            return dist;
        }
        println!("cargo:rerun-if-changed={}", legacy.display());
        return placeholder(&format!("ui/{LEGACY_DIST}"));
    }
    panic!(
        "could not locate the UI: walked up from {} looking for ui/pnpm-workspace.yaml \
         (or the legacy ui/common/). The server embeds a built UI, so the tree must be present.",
        manifest.display()
    );
}

fn find_up(start: &Path, probe: impl Fn(&Path) -> Option<PathBuf>) -> Option<PathBuf> {
    start.ancestors().find_map(probe)
}

/// Refuse anything but a complete release build of this edition with the live API.
fn check_release_spa(dist: &Path) {
    let read = |file: &str| {
        std::fs::read_to_string(dist.join(file)).unwrap_or_else(|_| {
            panic!(
                "{} is not a Nasiko release SPA (missing {file}). Build it with `just build-ui`.",
                dist.display()
            )
        })
    };
    let edition = read("nasiko-spa-edition");
    assert_eq!(
        edition.trim(),
        EDITION,
        "{} is a `{}` bundle; a test-only or wrong-edition SPA cannot be embedded. Build it with `just build-ui`.",
        dist.display(),
        edition.trim()
    );
    let meta = read("nasiko-spa.json");
    for required in [
        "\"testBuild\":false",
        "\"harness\":false",
        "\"apiMode\":\"live\"",
    ] {
        assert!(
            meta.contains(required),
            "{} is not a live-API release build ({required} missing from nasiko-spa.json). Build it with `just build-ui`.",
            dist.display()
        );
    }
    for file in [
        "index.html",
        "routes.json",
        "csp.json",
        "nasiko-prepaint.js",
    ] {
        assert!(
            dist.join(file).is_file(),
            "incomplete SPA {}: missing {file}",
            dist.display()
        );
    }
    assert!(
        !dist.join("mockServiceWorker.js").exists(),
        "{}: the mock service worker cannot be embedded",
        dist.display()
    );
}

/// An `index.html`-only bundle in `OUT_DIR`, so the build never writes into the source tree.
fn placeholder(expected: &str) -> PathBuf {
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("set by cargo")).join("ui-placeholder");
    std::fs::create_dir_all(&out)
        .unwrap_or_else(|e| panic!("could not create {}: {e}", out.display()));
    std::fs::write(out.join("index.html"), PLACEHOLDER)
        .unwrap_or_else(|e| panic!("could not write the placeholder: {e}"));
    println!(
        "cargo:warning={expected} is not built; embedded a placeholder page. Run `just build-ui` for the real UI."
    );
    out
}
