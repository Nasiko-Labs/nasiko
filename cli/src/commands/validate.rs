use std::fs;
use std::path::Path;

use anyhow::Result;

const REQUIRED_FILES: &[&str] = &["Dockerfile", "AgentCard.json"];

/// Every field an AgentCard must carry, in the order they are reported.
///
/// The second element is where A2A 1.0 moved the field to, as a key inside a
/// `supportedInterfaces[]` entry. `None` means the spec still keeps it at the
/// card root. Note the rename: 0.2.x called the transport `preferredTransport`
/// at the root, 1.0 calls it `protocolBinding` on the interface.
const REQUIRED_CARD_FIELDS: &[(&str, Option<&str>)] = &[
    ("name", None),
    ("description", None),
    ("url", Some("url")),
    ("version", None),
    ("capabilities", None),
    ("skills", None),
    ("protocolVersion", Some("protocolVersion")),
    ("preferredTransport", Some("protocolBinding")),
];

/// Returns the required fields the card carries in neither placement.
///
/// A2A 1.0 moved `url`, `protocolVersion` and the transport out of the card
/// root and into `supportedInterfaces[]`, so a card produced by a current SDK
/// has them nowhere near the root. Either placement satisfies the requirement;
/// only a card with neither is missing the field.
///
/// The preferred interface is the first entry, matching `fetch_agent_card` in
/// `commands::chat` and the reference SDKs' 1.0-to-0.2.x downgrade. Note that
/// `nasiko_types::a2a::extract_transport_path` instead prefers the JSONRPC
/// binding: it has to pick an endpoint to actually call, whereas this only
/// asks whether a field was declared at all, so the binding does not matter.
fn missing_card_fields(card: &serde_json::Value) -> Vec<&'static str> {
    let preferred_interface = card
        .get("supportedInterfaces")
        .and_then(serde_json::Value::as_array)
        .and_then(|interfaces| interfaces.first());

    REQUIRED_CARD_FIELDS
        .iter()
        .copied()
        .filter(|&(root_field, interface_field)| {
            if card.get(root_field).is_some() {
                return false;
            }
            match interface_field {
                None => true,
                Some(interface_field) => preferred_interface
                    .and_then(|interface| interface.get(interface_field))
                    .is_none(),
            }
        })
        .map(|(root_field, _)| root_field)
        .collect()
}

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
                    let missing = missing_card_fields(&card);
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

#[cfg(test)]
#[path = "tests/validate_tests.rs"]
mod tests;
