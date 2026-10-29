//! What the operating system says about an agent's process: its working folder, its command
//! line, and the Python `site-packages` folders it has loaded libraries from. `connect` decides
//! from these, and only these, which file the agent reads its variables from.
//!
//! Read-only and limited to what the current user may see: Linux's `/proc`, `lsof` and `ps`
//! elsewhere (macOS). Anything that cannot be read is left empty, so a caller falls back to
//! showing the variables rather than guessing.

use std::path::{Path, PathBuf};

use super::endpoints::{COMMAND_TIMEOUT, CommandOutcome, run_command};

/// Facts about one process, each empty when it could not be read.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProcessFacts {
    pub cwd: Option<PathBuf>,
    pub args: Vec<String>,
    /// `site-packages` folders the process mapped a library from, in first-seen order.
    pub site_packages: Vec<PathBuf>,
}

/// Inspect `pid`.
pub fn inspect(pid: u32) -> ProcessFacts {
    let proc_dir = PathBuf::from(format!("/proc/{pid}"));
    if proc_dir.exists() {
        return ProcessFacts {
            cwd: working_folder(pid),
            args: std::fs::read(proc_dir.join("cmdline"))
                .map(|raw| split_nul(&raw))
                .unwrap_or_default(),
            site_packages: std::fs::read_to_string(proc_dir.join("maps"))
                .map(|maps| {
                    site_packages_in(
                        maps.lines()
                            .filter_map(|line| line.split_whitespace().nth(5)),
                    )
                })
                .unwrap_or_default(),
        };
    }
    let cwd = working_folder(pid);
    let pid = pid.to_string();
    ProcessFacts {
        cwd,
        args: match run_command(
            "ps",
            &["-ww", "-o", "command=", "-p", &pid],
            COMMAND_TIMEOUT,
        ) {
            CommandOutcome::Output(line) => line.split_whitespace().map(str::to_string).collect(),
            _ => Vec::new(),
        },
        site_packages: site_packages_in(
            lsof_names(&["-p", &pid, "-Fn"]).iter().map(String::as_str),
        ),
    }
}

/// `pid`'s working folder, when it can be read.
pub fn working_folder(pid: u32) -> Option<PathBuf> {
    let proc_cwd = PathBuf::from(format!("/proc/{pid}/cwd"));
    if proc_cwd.parent().is_some_and(Path::exists) {
        return std::fs::read_link(proc_cwd).ok();
    }
    lsof_names(&["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .into_iter()
        .next()
        .map(PathBuf::from)
}

/// The `n<name>` values of `lsof -F…n` output.
fn lsof_names(args: &[&str]) -> Vec<String> {
    match run_command("lsof", args, COMMAND_TIMEOUT) {
        CommandOutcome::Output(output) => output
            .lines()
            .filter_map(|line| line.strip_prefix('n'))
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn split_nul(raw: &[u8]) -> Vec<String> {
    raw.split(|byte| *byte == 0)
        .filter(|arg| !arg.is_empty())
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect()
}

/// The distinct `…/site-packages` folders the given file paths sit under.
fn site_packages_in<'a>(paths: impl Iterator<Item = &'a str>) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    for path in paths {
        let Some(end) = path.find("/site-packages/") else {
            continue;
        };
        let dir = PathBuf::from(&path[..end + "/site-packages".len()]);
        if !found.contains(&dir) {
            found.push(dir);
        }
    }
    found
}

/// Whether `path` is `folder` or inside it, compared as written (both absolute).
pub fn is_within(path: &Path, folder: &Path) -> bool {
    path.starts_with(folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_packages_are_read_from_mapped_library_paths() {
        let paths = [
            "/usr/lib/libc.so.6",
            "/work/crew/.venv/lib/python3.12/site-packages/pydantic_core/_core.so",
            "/work/crew/.venv/lib/python3.12/site-packages/yaml/_yaml.so",
            "/opt/other/lib/python3.12/site-packages/x.so",
        ];

        assert_eq!(
            site_packages_in(paths.into_iter()),
            vec![
                PathBuf::from("/work/crew/.venv/lib/python3.12/site-packages"),
                PathBuf::from("/opt/other/lib/python3.12/site-packages"),
            ]
        );
    }

    #[test]
    fn a_nul_separated_command_line_keeps_spaces_inside_arguments() {
        assert_eq!(
            split_nul(b"/my venv/bin/python\0serve.py\0--port\08765\0"),
            vec!["/my venv/bin/python", "serve.py", "--port", "8765"]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn this_test_process_can_be_inspected() {
        let facts = inspect(std::process::id());

        assert_eq!(facts.cwd, std::env::current_dir().ok());
        assert!(!facts.args.is_empty());
    }
}
