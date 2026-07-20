use std::fs;
use std::path::Path;

use anyhow::Result;

const REQUIRED_FILES: &[&str] = &["Dockerfile", "AgentCard.json"];
const REQUIRED_CARD_FIELDS: &[&str] = &[
    "name",
    "description",
    "url",
    "version",
    "capabilities",
    "skills",
    "protocolVersion",
    "preferredTransport",
];

pub fn validate(directory: &str) -> Result<()> {
    let root = Path::new(directory)
        .canonicalize()
        .unwrap_or_else(|_| Path::new(directory).to_path_buf());
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    println!("\nValidating agent at {}\n", root.display());

    // Required files
    for &rel in REQUIRED_FILES {
        if root.join(rel).exists() {
            println!("  ✓ {rel}");
        } else {
            errors.push(format!("{rel} not found"));
            println!("  ✗ {rel} — missing");
        }
    }

    // src/ directory (Python) or cmd/ (Go) — at least one
    let has_src =
        root.join("src").is_dir() || root.join("cmd").is_dir() || root.join("main.go").exists();
    if has_src {
        println!("  ✓ source directory");
    } else {
        warnings.push("no src/ or cmd/ directory found".into());
        println!("  ! source directory — missing (expected src/ or cmd/)");
    }

    // AgentCard.json schema
    let card_path = root.join("AgentCard.json");
    if card_path.exists() {
        match fs::read_to_string(&card_path) {
            Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(card) => {
                    let missing: Vec<&str> = REQUIRED_CARD_FIELDS
                        .iter()
                        .filter(|&&f| card.get(f).is_none())
                        .copied()
                        .collect();
                    if missing.is_empty() {
                        println!("  ✓ AgentCard.json fields");
                    } else {
                        let msg = format!("missing fields: {}", missing.join(", "));
                        errors.push(format!("AgentCard.json {msg}"));
                        println!("  ✗ AgentCard.json — {msg}");
                    }

                    // Validate skills is non-empty array
                    if let Some(skills) = card.get("skills").and_then(|s| s.as_array()) {
                        if skills.is_empty() {
                            warnings.push("AgentCard.json has empty skills array".into());
                            println!("  ! skills — empty (add at least one)");
                        } else {
                            println!("  ✓ skills ({} defined)", skills.len());
                        }
                    }
                }
                Err(e) => {
                    errors.push(format!("AgentCard.json is not valid JSON: {e}"));
                    println!("  ✗ AgentCard.json — invalid JSON");
                }
            },
            Err(e) => {
                errors.push(format!("cannot read AgentCard.json: {e}"));
                println!("  ✗ AgentCard.json — unreadable");
            }
        }
    }

    // Optional recommended files
    for rel in &["docker-compose.yml", ".env.example"] {
        if root.join(rel).exists() {
            println!("  ✓ {rel}");
        } else {
            warnings.push(format!("{rel} not found"));
            println!("  ! {rel} — missing (recommended)");
        }
    }

    // Summary
    println!();
    if !errors.is_empty() {
        println!(
            "✗ {} error(s){}",
            errors.len(),
            if warnings.is_empty() {
                String::new()
            } else {
                format!(", {} warning(s)", warnings.len())
            }
        );
        for e in &errors {
            println!("  • {e}");
        }
        anyhow::bail!("validation failed");
    }

    if !warnings.is_empty() {
        println!("✓ Valid ({} warning(s))", warnings.len());
    } else {
        println!("✓ Valid");
    }

    Ok(())
}
