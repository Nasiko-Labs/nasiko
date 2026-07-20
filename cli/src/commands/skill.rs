use std::path::Path;

use anyhow::Result;

use crate::skill;

pub fn add(name: &str, directory: &str) -> Result<()> {
    let project_dir = Path::new(directory);

    let framework = skill::detect_framework(project_dir).ok_or_else(|| {
        anyhow::anyhow!(
            "cannot detect framework. Ensure AgentCard.json exists with agentFramework field."
        )
    })?;

    println!("Detected framework: {framework}");

    let (manifest, impl_code) = skill::resolve_skill(name)?;

    let project_lang = match framework.as_str() {
        "a2a-go" => "go",
        _ => "python",
    };
    if manifest.runtime.language != project_lang {
        anyhow::bail!(
            "skill '{name}' is written in {} but this project uses {framework} ({project_lang})",
            manifest.runtime.language
        );
    }

    println!("Adding skill: {name}...");

    skill::inject_skill(project_dir, &manifest, &impl_code, &framework)?;

    println!("✓ Added {name}");
    let module = name.replace('-', "_");
    if project_lang == "go" {
        println!("  → internal/skills/{module}.go");
    } else {
        println!("  → src/{module}.py");
    }
    if !manifest.runtime.dependencies.is_empty() {
        println!("  → deps: {}", manifest.runtime.dependencies.join(", "));
    }
    if !manifest.runtime.env_vars.is_empty() {
        println!(
            "  → env vars needed: {}",
            manifest.runtime.env_vars.join(", ")
        );
    }

    Ok(())
}

pub fn remove(name: &str, directory: &str) -> Result<()> {
    let project_dir = Path::new(directory);

    let framework = skill::detect_framework(project_dir).unwrap_or_else(|| "openai".into());

    println!("Removing skill: {name}...");
    skill::remove_skill(project_dir, name, &framework)?;
    println!("✓ Removed {name}");

    Ok(())
}

pub fn list(directory: &str) -> Result<()> {
    let project_dir = Path::new(directory);
    let framework = skill::detect_framework(project_dir);
    let is_go = framework.as_deref() == Some("a2a-go");

    let skill_dir = if is_go {
        project_dir.join("internal/skills")
    } else {
        project_dir.join("src")
    };

    if !skill_dir.is_dir() {
        println!("No skills found.");
        return Ok(());
    }

    let available = skill::list_available_skills();
    let mut found = Vec::new();

    let (ext, excludes): (&str, &[&str]) = if is_go {
        (".go", &[])
    } else {
        (
            ".py",
            &[
                "__init__.py",
                "__main__.py",
                "agent.py",
                "agent_executor.py",
            ],
        )
    };

    for entry in std::fs::read_dir(&skill_dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(ext) && !excludes.contains(&name.as_str()) {
            let skill_name = name.trim_end_matches(ext).replace('_', "-");
            let from_registry = available.contains(&skill_name);
            found.push((skill_name, from_registry));
        }
    }

    if found.is_empty() {
        println!("No skills installed in this project.");
    } else {
        println!("Installed skills:");
        for (name, is_known) in &found {
            let badge = if *is_known { " (registry)" } else { "" };
            println!("  • {name}{badge}");
        }
    }

    Ok(())
}

pub fn search(query: Option<&str>, framework: Option<&str>) -> Result<()> {
    // Try registry API first (fast, no blob pulls)
    if let Some(client) = crate::api::RegistryClient::new()
        && let Ok(results) = client.search(query, Some("skill"), framework)
        && !results.is_empty()
    {
        println!("Available skills (registry):");
        for artifact in &results {
            let desc = artifact.description.as_deref().unwrap_or("");
            println!("  • {} — {}", artifact.name, desc);
            if !artifact.tags.is_empty() {
                println!("    tags: {}", artifact.tags.join(", "));
            }
        }
        // Also show embedded-only skills not in registry
        print_embedded_only(query, &results);
        return Ok(());
    }

    // Fallback: embedded skills only
    let available = skill::list_available_skills();
    let filtered: Vec<&String> = available
        .iter()
        .filter(|name| {
            if let Some(q) = query {
                name.contains(q)
            } else {
                true
            }
        })
        .collect();

    if filtered.is_empty() {
        println!("No skills found.");
    } else {
        println!("Available skills:");
        for name in &filtered {
            if let Ok((manifest, _)) = skill::resolve_skill(name) {
                let tags = manifest.metadata.tags.join(", ");
                println!("  • {} — {}", name, manifest.metadata.description);
                if !tags.is_empty() {
                    println!("    tags: {tags}");
                }
            }
        }
    }

    Ok(())
}

fn print_embedded_only(query: Option<&str>, registry_results: &[crate::api::Artifact]) {
    let embedded = skill::list_available_skills();
    let registry_names: Vec<&str> = registry_results.iter().map(|a| a.name.as_str()).collect();

    let extra: Vec<&String> = embedded
        .iter()
        .filter(|name| {
            !registry_names.contains(&name.as_str())
                && query.map(|q| name.contains(q)).unwrap_or(true)
        })
        .collect();

    if !extra.is_empty() {
        println!("\nAvailable skills (built-in):");
        for name in &extra {
            if let Ok((manifest, _)) = skill::resolve_skill(name) {
                println!("  • {} — {}", name, manifest.metadata.description);
            }
        }
    }
}

pub fn info(name: &str) -> Result<()> {
    let (manifest, _) = skill::resolve_skill(name)?;

    println!("{} v{}", manifest.metadata.name, manifest.metadata.version);
    println!("  {}", manifest.metadata.description);
    println!();
    println!("  Language:     {}", manifest.runtime.language);
    println!("  Function:     {}", manifest.interface.function);
    println!("  Tags:         {}", manifest.metadata.tags.join(", "));
    if !manifest.runtime.dependencies.is_empty() {
        println!(
            "  Dependencies: {}",
            manifest.runtime.dependencies.join(", ")
        );
    }
    if !manifest.runtime.env_vars.is_empty() {
        println!("  Env vars:     {}", manifest.runtime.env_vars.join(", "));
    }

    Ok(())
}
