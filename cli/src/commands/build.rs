use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};

use crate::util::container_bin;

pub fn build(directory: &str, tag: Option<&str>, platform: Option<&str>) -> Result<()> {
    let root = Path::new(directory)
        .canonicalize()
        .unwrap_or_else(|_| Path::new(directory).to_path_buf());

    if !root.join("Dockerfile").exists() {
        bail!(
            "No Dockerfile found at {}. Run `nasiko new` first.",
            root.display()
        );
    }
    check_prebuilt_binaries_exist(&root)?;

    let resolved_tag = match tag {
        Some(t) => t.to_string(),
        None => default_tag(&root),
    };

    println!("Building {resolved_tag}");

    let bin = container_bin();
    let mut cmd = Command::new(&bin);
    cmd.args(["build", "-t", &resolved_tag]);

    if let Some(p) = platform {
        cmd.args(["--platform", p]);
    }

    cmd.arg(root.to_str().unwrap_or("."));

    let cmd_str = format!(
        "{bin} build -t {resolved_tag}{} {}",
        platform
            .map(|p| format!(" --platform {p}"))
            .unwrap_or_default(),
        root.display(),
    );
    println!("$ {cmd_str}\n");

    let status = cmd.status()?;
    if !status.success() {
        bail!("Build failed (exit code: {})", status.code().unwrap_or(1));
    }

    println!("\n✓ Built {resolved_tag}");
    println!("  Run locally: nasiko run {directory}");
    println!("  Deploy:      nasiko deploy {resolved_tag}");

    Ok(())
}

/// Fail fast — with the actual fix — when the Dockerfile COPYs a compiled
/// binary out of `target/` that hasn't been built yet. The Rust templates
/// (`FROM scratch`) expect a `just build` (cargo zigbuild) first; without this
/// check docker fails mid-build with an opaque "not found" error.
fn check_prebuilt_binaries_exist(root: &Path) -> Result<()> {
    let Ok(dockerfile) = fs::read_to_string(root.join("Dockerfile")) else {
        return Ok(());
    };
    if let Some(missing) = missing_copy_sources(&dockerfile, root).first() {
        let fix = if root.join("justfile").exists() {
            "run `just build` in the project directory first (it compiles the binary, \
             then you can `nasiko build`)"
        } else {
            "build it first (e.g. cargo zigbuild --release for the Dockerfile's target)"
        };
        bail!("Dockerfile copies '{missing}' which does not exist — {fix}");
    }
    Ok(())
}

/// COPY sources under `target/` that are absent on disk. Only build-output
/// paths are checked — sources like `src/` always exist in a scaffold, and
/// multi-stage `COPY --from=` sources live in the image, not on disk.
fn missing_copy_sources(dockerfile: &str, root: &Path) -> Vec<String> {
    dockerfile
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("COPY ") && !l.contains("--from"))
        .filter_map(|l| l.split_whitespace().nth(1))
        .filter(|src| src.starts_with("target/") && !root.join(src).exists())
        .map(String::from)
        .collect()
}

fn default_tag(root: &Path) -> String {
    let card_path = root.join("AgentCard.json");
    if card_path.exists()
        && let Ok(content) = fs::read_to_string(&card_path)
        && let Ok(card) = serde_json::from_str::<serde_json::Value>(&content)
    {
        let name = card.get("name").and_then(|n| n.as_str()).unwrap_or("agent");
        let version = card
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("latest");
        return format!("{name}:{version}");
    }
    let dir_name = root.file_name().unwrap_or_default().to_string_lossy();
    format!("{dir_name}:latest")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_target_copy_source_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let dockerfile =
            "FROM scratch\nCOPY target/x86_64-unknown-linux-musl/release/agent /agent\n";
        assert_eq!(
            missing_copy_sources(dockerfile, dir.path()),
            vec!["target/x86_64-unknown-linux-musl/release/agent".to_string()]
        );
    }

    #[test]
    fn present_target_copy_source_passes() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("target/release");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("agent"), b"").unwrap();
        let dockerfile = "COPY target/release/agent /agent\n";
        assert!(missing_copy_sources(dockerfile, dir.path()).is_empty());
    }

    #[test]
    fn non_target_and_multistage_copies_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let dockerfile = "COPY src/ ./src/\nCOPY --from=builder target/release/agent /agent\n";
        assert!(missing_copy_sources(dockerfile, dir.path()).is_empty());
    }
}
