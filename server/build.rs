//! Resolves the UI asset root and exports it as `NASIKO_UI` for `rust_embed`.
//!
//! The UI lives in a single top-level `ui/` tree shared by both editions, but
//! this crate sits at a different depth in the private repo than in the public
//! one: `oss/server/` here, `server/` after `scripts/sync-oss.sh` strips the
//! `oss/` prefix. A literal `#[folder = "../../ui/oss/dist/"]` is therefore
//! correct in exactly one of the two layouts — it silently broke the public
//! repo once already, when the UI was consolidated under `ui/`.
//!
//! rust-embed hard-errors at compile time on a missing `folder`, so the public
//! build failed rather than shipping an empty binary. Resolving the root here
//! and interpolating it (`#[folder = "$NASIKO_UI/oss/dist/"]`, which needs
//! rust-embed's `interpolate-folder-path` feature) makes both layouts work off
//! one source of truth, and keeps working if the crate ever moves again.
//!
//! The same hard-error is why this file writes a placeholder: `ui/oss/dist` is
//! a Vite build output and is not committed, so a fresh clone does not have it.
//! A Rust-only contributor must still be able to `cargo check` without
//! installing Node, and `server/Dockerfile` builds the real bundle in a node
//! stage for anyone running the image. `just build-ui` overwrites the
//! placeholder locally.

use std::path::{Path, PathBuf};

/// Marker that identifies the real UI root: `ui/common/` is the shared React
/// core, present in every layout and in both editions.
const MARKER: &str = "common";

/// The edition bundle this binary serves, relative to the UI root.
const BUNDLE: &str = "oss/dist";

const PLACEHOLDER: &str = r#"<!doctype html>
<title>Nasiko UI — not built</title>
<body style="font-family: system-ui; padding: 3rem; line-height: 1.6">
<h1>UI not built</h1>
<p>This placeholder ships when the frontend hasn't been compiled. Build it:</p>
<pre>just build-ui</pre>
<p>then rebuild <code>nasiko-server</code>.</p>
</body>
"#;

fn find_ui_root(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        let candidate = dir.join("ui");
        if candidate.join(MARKER).is_dir() {
            return Some(candidate);
        }
    }
    None
}

fn main() {
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is always set by cargo"),
    );

    let ui_root = find_ui_root(&manifest_dir).unwrap_or_else(|| {
        panic!(
            "could not locate the UI root: walked up from {} looking for a `ui/{}/` directory. \
             The server embeds ui/{} at compile time, so the tree must be present. \
             If this is the public repo, scripts/sync-oss.sh did not publish ui/.",
            manifest_dir.display(),
            MARKER,
            BUNDLE,
        )
    });

    // Guarantee the embed target exists, so a Rust build never depends on a
    // Node toolchain. Only ever creates what is missing — a real build's
    // output is left alone.
    let bundle = ui_root.join(BUNDLE);
    let index = bundle.join("index.html");
    if !index.exists() {
        std::fs::create_dir_all(&bundle)
            .unwrap_or_else(|e| panic!("could not create {}: {e}", bundle.display()));
        std::fs::write(&index, PLACEHOLDER)
            .unwrap_or_else(|e| panic!("could not write {}: {e}", index.display()));
        println!(
            "cargo:warning=ui/{BUNDLE} was empty; embedded a placeholder. Run `just build-ui` for the real UI."
        );
    }

    // Absolute, so it does not inherit this crate's depth.
    println!("cargo:rustc-env=NASIKO_UI={}", ui_root.display());

    // rust-embed reads the tree at compile time in release builds, so changes
    // to the built bundle must invalidate this crate. Only the bundle matters
    // now — the React sources reach the binary only through it.
    println!("cargo:rerun-if-changed={}", bundle.display());
    println!("cargo:rerun-if-changed=build.rs");
}
