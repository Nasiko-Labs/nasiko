//! Specialized coding tools exposed to the LLM. `definitions()` returns OpenAI-style function
//! schemas; `execute()` dispatches a tool call against the active [`Sandbox`]. Mirrors the shape
//! of `agents/paper/src/tools.rs` but is parameterized by the sandbox backend.

use serde_json::{json, Value};

use crate::project;
use crate::sandbox::Sandbox;

/// OpenAI-style tool/function definitions advertised to the model.
pub fn definitions() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a file from the workspace. Returns content with 1-based line numbers. Read before editing.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path relative to the workspace root" },
                        "start_line": { "type": "integer", "description": "Optional 1-based first line to return" },
                        "end_line": { "type": "integer", "description": "Optional 1-based last line to return (inclusive)" }
                    },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "list_directory",
                "description": "List files/directories under a workspace path (recursive). Skips .git, target, node_modules.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path relative to the workspace root", "default": "." },
                        "depth": { "type": "integer", "description": "Max recursion depth (omit for unbounded)" }
                    }
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "search_code",
                "description": "Search the workspace for a pattern (uses ripgrep if available). Returns file:line: match lines.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Regex or literal text to search for" },
                        "path": { "type": "string", "description": "Subdirectory to limit the search to", "default": "." },
                        "glob": { "type": "string", "description": "Optional file glob filter, e.g. '*.rs'" }
                    },
                    "required": ["pattern"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Create or overwrite a file with the given content. Use edit_file for targeted changes to existing files.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path relative to the workspace root" },
                        "content": { "type": "string", "description": "Full file content" }
                    },
                    "required": ["path", "content"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Make a targeted edit using search/replace. The `search` block must appear EXACTLY ONCE in the file; it is replaced with `replace`. Provide multiple {search,replace} objects in `edits` to apply several edits in one call (applied in order).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path relative to the workspace root" },
                        "search": { "type": "string", "description": "Exact text to find (must be unique). Omit if using `edits`." },
                        "replace": { "type": "string", "description": "Replacement text. Omit if using `edits`." },
                        "edits": {
                            "type": "array",
                            "description": "Multiple edits applied in order",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "search": { "type": "string" },
                                    "replace": { "type": "string" }
                                },
                                "required": ["search", "replace"]
                            }
                        }
                    },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "run_command",
                "description": "Run a shell command in the workspace root (build, lint, format, etc.). Returns stdout, stderr, and exit code.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Shell command to run via `sh -c`" },
                        "timeout_s": { "type": "integer", "description": "Optional wall-clock timeout in seconds" }
                    },
                    "required": ["command"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "run_tests",
                "description": "Run the project's test suite. Auto-detects the command (cargo test / npm test / pytest / go test) unless `command` is given.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Optional explicit test command override" },
                        "timeout_s": { "type": "integer", "description": "Optional wall-clock timeout in seconds" }
                    }
                }
            }
        }),
    ]
}

/// Execute a tool call. `arguments` is the raw JSON string from the model. Always returns a
/// string suitable to feed back as the tool result (errors are formatted, never panic).
pub async fn execute(sandbox: &dyn Sandbox, name: &str, arguments: &str) -> String {
    let args: Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(e) => return format!("Error: invalid tool arguments JSON: {e}"),
    };

    let result = match name {
        "read_file" => read_file(sandbox, &args).await,
        "list_directory" => list_directory(sandbox, &args).await,
        "search_code" => search_code(sandbox, &args).await,
        "write_file" => write_file(sandbox, &args).await,
        "edit_file" => edit_file(sandbox, &args).await,
        "run_command" => run_command(sandbox, &args).await,
        "run_tests" => run_tests(sandbox, &args).await,
        other => Err(format!("unknown tool: {other}")),
    };

    match result {
        Ok(s) => s,
        Err(e) => format!("Error: {e}"),
    }
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args[key].as_str().ok_or_else(|| format!("missing or non-string '{key}'"))
}

async fn read_file(sandbox: &dyn Sandbox, args: &Value) -> Result<String, String> {
    let path = arg_str(args, "path")?;
    let range = match (args["start_line"].as_u64(), args["end_line"].as_u64()) {
        (Some(s), Some(e)) => Some((s as usize, e as usize)),
        (Some(s), None) => Some((s as usize, usize::MAX)),
        _ => None,
    };
    sandbox.read_file(path, range).await
}

async fn list_directory(sandbox: &dyn Sandbox, args: &Value) -> Result<String, String> {
    let path = args["path"].as_str().unwrap_or(".");
    let depth = args["depth"].as_u64().map(|d| d as usize);
    sandbox.list_dir(path, depth).await
}

async fn search_code(sandbox: &dyn Sandbox, args: &Value) -> Result<String, String> {
    let pattern = arg_str(args, "pattern")?;
    let path = args["path"].as_str().unwrap_or(".");
    let glob = args["glob"].as_str();

    // Prefer ripgrep; fall back to grep -r. Both emit file:line:match. Single-quote-escape the
    // user inputs for safe interpolation into `sh -c`.
    let mut rg = format!(
        "rg --line-number --no-heading --color never {} {}",
        shell_quote(pattern),
        shell_quote(path)
    );
    if let Some(g) = glob {
        rg = format!(
            "rg --line-number --no-heading --color never --glob {} {} {}",
            shell_quote(g),
            shell_quote(pattern),
            shell_quote(path)
        );
    }
    // If rg is missing, retry with grep.
    let grep = format!(
        "grep -rn {} {} 2>/dev/null",
        shell_quote(pattern),
        shell_quote(path)
    );
    let command = format!("if command -v rg >/dev/null 2>&1; then {rg}; else {grep}; fi");

    let res = sandbox.exec(&command, Some(60)).await?;
    if res.stdout.trim().is_empty() {
        Ok(format!("No matches for '{pattern}'."))
    } else {
        Ok(res.stdout)
    }
}

async fn write_file(sandbox: &dyn Sandbox, args: &Value) -> Result<String, String> {
    let path = arg_str(args, "path")?;
    let content = arg_str(args, "content")?;
    sandbox.write_file(path, content).await?;
    Ok(format!("Wrote {} bytes to {path}", content.len()))
}

async fn edit_file(sandbox: &dyn Sandbox, args: &Value) -> Result<String, String> {
    let path = arg_str(args, "path")?;

    // Collect edits from either the single search/replace pair or the `edits` array.
    let mut edits: Vec<(String, String)> = Vec::new();
    if let Some(arr) = args["edits"].as_array() {
        for (i, e) in arr.iter().enumerate() {
            let s = e["search"]
                .as_str()
                .ok_or_else(|| format!("edits[{i}] missing 'search'"))?;
            let r = e["replace"]
                .as_str()
                .ok_or_else(|| format!("edits[{i}] missing 'replace'"))?;
            edits.push((s.to_string(), r.to_string()));
        }
    }
    if let (Some(s), Some(r)) = (args["search"].as_str(), args["replace"].as_str()) {
        edits.push((s.to_string(), r.to_string()));
    }
    if edits.is_empty() {
        return Err("provide either 'search'+'replace' or a non-empty 'edits' array".into());
    }

    // Read the raw file (without line numbers) so search blocks match verbatim.
    let mut content = sandbox.read_file_raw(path).await?;

    for (idx, (search, replace)) in edits.iter().enumerate() {
        let label = if edits.len() > 1 { format!("edit {} ", idx + 1) } else { String::new() };
        let matches = content.matches(search.as_str()).count();
        match matches {
            0 => return Err(format!("{label}search block not found in {path}")),
            1 => {
                content = content.replacen(search.as_str(), replace, 1);
            }
            n => {
                return Err(format!(
                    "{label}search block is not unique in {path} ({n} matches); add more surrounding context"
                ));
            }
        }
    }

    sandbox.write_file(path, &content).await?;
    Ok(format!("Applied {} edit(s) to {path}", edits.len()))
}

async fn run_command(sandbox: &dyn Sandbox, args: &Value) -> Result<String, String> {
    let command = arg_str(args, "command")?;
    let timeout_s = args["timeout_s"].as_u64();
    let res = sandbox.exec(command, timeout_s).await?;
    Ok(format_exec(&res))
}

async fn run_tests(sandbox: &dyn Sandbox, args: &Value) -> Result<String, String> {
    let override_cmd = args["command"].as_str();
    let command = project::test_command(sandbox, override_cmd).await?;
    let timeout_s = args["timeout_s"].as_u64().or(Some(600));
    let res = sandbox.exec(&command, timeout_s).await?;
    Ok(format!("$ {command}\n{}", format_exec(&res)))
}

fn format_exec(res: &crate::sandbox::ExecResult) -> String {
    let mut out = format!("exit code: {}\n", res.exit_code);
    if !res.stdout.trim().is_empty() {
        out.push_str("--- stdout ---\n");
        out.push_str(&res.stdout);
        if !res.stdout.ends_with('\n') {
            out.push('\n');
        }
    }
    if !res.stderr.trim().is_empty() {
        out.push_str("--- stderr ---\n");
        out.push_str(&res.stderr);
        if !res.stderr.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// Wrap a string in single quotes for safe use in `sh -c`, escaping embedded single quotes.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::LocalSandbox;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("coding-agent-tools-{tag}"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::canonicalize(&base).unwrap()
    }

    #[tokio::test]
    async fn edit_file_unique_replace() {
        let root = temp_root("edit-unique");
        let sb = LocalSandbox::new(root.to_str().unwrap()).unwrap();
        sb.write_file("lib.rs", "fn a() {}\nfn b() {}\n").await.unwrap();
        let args = json!({"path": "lib.rs", "search": "fn b() {}", "replace": "fn b() { add(1,2); }"});
        let out = execute(&sb, "edit_file", &args.to_string()).await;
        assert!(out.contains("Applied 1 edit"), "got: {out}");
        let raw = sb.read_file_raw("lib.rs").await.unwrap();
        assert!(raw.contains("fn b() { add(1,2); }"));
    }

    #[tokio::test]
    async fn edit_file_not_found_errors() {
        let root = temp_root("edit-notfound");
        let sb = LocalSandbox::new(root.to_str().unwrap()).unwrap();
        sb.write_file("lib.rs", "fn a() {}\n").await.unwrap();
        let args = json!({"path": "lib.rs", "search": "fn zzz() {}", "replace": "x"});
        let out = execute(&sb, "edit_file", &args.to_string()).await;
        assert!(out.contains("not found"), "got: {out}");
    }

    #[tokio::test]
    async fn edit_file_not_unique_errors() {
        let root = temp_root("edit-dup");
        let sb = LocalSandbox::new(root.to_str().unwrap()).unwrap();
        sb.write_file("lib.rs", "x\nx\n").await.unwrap();
        let args = json!({"path": "lib.rs", "search": "x", "replace": "y"});
        let out = execute(&sb, "edit_file", &args.to_string()).await;
        assert!(out.contains("not unique"), "got: {out}");
    }

    #[tokio::test]
    async fn run_command_reports_exit() {
        let root = temp_root("runcmd");
        let sb = LocalSandbox::new(root.to_str().unwrap()).unwrap();
        let out = execute(&sb, "run_command", &json!({"command": "echo hi"}).to_string()).await;
        assert!(out.contains("exit code: 0"));
        assert!(out.contains("hi"));
    }

    #[tokio::test]
    async fn unknown_tool_errors() {
        let root = temp_root("unknown");
        let sb = LocalSandbox::new(root.to_str().unwrap()).unwrap();
        let out = execute(&sb, "nope", "{}").await;
        assert!(out.contains("unknown tool"));
    }
}
