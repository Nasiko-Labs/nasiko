use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};

use crate::util::container_bin;

pub fn build(directory: &str, tag: Option<&str>, platform: Option<&str>) -> Result<()> {
    let root = Path::new(directory).canonicalize().unwrap_or_else(|_| Path::new(directory).to_path_buf());

    if !root.join("Dockerfile").exists() {
        bail!("No Dockerfile found at {}. Run `nasiko new` first.", root.display());
    }

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

    let cmd_str = format!("{bin} build -t {resolved_tag}{} {}",
        platform.map(|p| format!(" --platform {p}")).unwrap_or_default(),
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

fn default_tag(root: &Path) -> String {
    let card_path = root.join("AgentCard.json");
    if card_path.exists()
        && let Ok(content) = fs::read_to_string(&card_path)
            && let Ok(card) = serde_json::from_str::<serde_json::Value>(&content) {
                let name = card.get("name").and_then(|n| n.as_str()).unwrap_or("agent");
                let version = card.get("version").and_then(|v| v.as_str()).unwrap_or("latest");
                return format!("{name}:{version}");
            }
    let dir_name = root.file_name().unwrap_or_default().to_string_lossy();
    format!("{dir_name}:latest")
}
