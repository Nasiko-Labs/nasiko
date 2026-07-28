//! Shared rules for which files count as agent source when feeding the
//! capability generator (AgentCard generation). Used by the server when
//! reading uploaded source archives and by the CLI when collecting a local
//! agent directory — the two must agree so `nasiko card` and server-side
//! builds see the same source, whatever language the agent is written in.

/// Extensions treated as readable agent source/config.
pub const SOURCE_EXTENSIONS: &[&str] = &[
    "py", "rs", "ts", "js", "go", "java", "rb", "ex", "exs", "toml", "yaml", "yml", "json", "md",
    "txt", "sh",
];

/// Files larger than this are skipped (generated/vendored blobs drown out the
/// actual agent code in the LLM prompt).
pub const MAX_SOURCE_FILE_BYTES: u64 = 50_000;

/// Directories never worth descending into when walking an agent source tree.
pub const SKIP_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "__pycache__",
    ".venv",
    "venv",
    "dist",
    "build",
    ".nasiko",
];

/// Whether a file (by name, e.g. `main.rs` or `Dockerfile.agent`) counts as
/// agent source for capability generation.
pub fn is_agent_source_file(file_name: &str) -> bool {
    let lower = file_name.to_lowercase();
    if lower.contains("dockerfile") {
        return true;
    }
    let ext = lower.rsplit('.').next().unwrap_or("");
    // A name without a '.' yields itself as "extension"; guard against that.
    ext != lower && SOURCE_EXTENSIONS.contains(&ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_source_files_across_languages() {
        let names = [
            "main.rs",
            "agent.py",
            "main.go",
            "index.ts",
            "Cargo.toml",
            "card.json",
        ];
        for name in names {
            assert!(is_agent_source_file(name), "{name} should match");
        }
    }

    #[test]
    fn recognizes_dockerfiles() {
        assert!(is_agent_source_file("Dockerfile"));
        assert!(is_agent_source_file("dockerfile.agent"));
    }

    #[test]
    fn rejects_non_source_files() {
        for name in ["image.png", "binary", "archive.tar.gz", "lib.so"] {
            assert!(!is_agent_source_file(name), "{name} should not match");
        }
    }
}
