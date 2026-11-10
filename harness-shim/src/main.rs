//! `harness-shim` binary: CLI parsing and wiring only. See the library crate for behaviour.

use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Parser;
use harness_shim::handshake::{HARNESS_CMD, Mode, NO_COMMAND_MESSAGE, split_command};
use harness_shim::server::{Config, SHIM_TOKEN_ENV, run};
use tracing_subscriber::EnvFilter;

const WORKSPACE_ENV: &str = "HARNESS_WORKSPACE";
const MODE_ENV: &str = "HARNESS_SHIM_MODE";
const LISTEN_ENV: &str = "HARNESS_SHIM_LISTEN";
const DEFAULT_LISTEN: &str = "0.0.0.0:7000";

/// Runs a harness's ACP command and serves its stdio as a WebSocket at `/acp`.
#[derive(Parser)]
#[command(name = "harness-shim", version)]
struct Cli {
    /// Address to serve `/acp` and `/healthz` on.
    /// Falls back to `HARNESS_SHIM_LISTEN`, then `0.0.0.0:7000`.
    #[arg(long)]
    listen: Option<SocketAddr>,
    /// The session's working directory (and clone target). Falls back to `HARNESS_WORKSPACE`,
    /// then `/workspace`.
    #[arg(long)]
    workspace: Option<std::path::PathBuf>,
    /// Frames held while no client is attached.
    #[arg(long, default_value_t = 10_000)]
    buffer_lines: usize,
    /// The harness command, after `--`. Falls back to `HARNESS_CMD`.
    #[arg(last = true)]
    command: Vec<String>,
}

/// Picks the command from the CLI, else from `HARNESS_CMD` (whitespace-split).
fn resolve_command(cli_command: Vec<String>) -> Result<Vec<String>> {
    if !cli_command.is_empty() {
        return Ok(cli_command);
    }
    let from_env = std::env::var(HARNESS_CMD).unwrap_or_default();
    let argv = split_command(&from_env);
    if argv.is_empty() {
        bail!(NO_COMMAND_MESSAGE);
    }
    Ok(argv)
}

/// `HARNESS_SHIM_MODE`: `env` (the default) or `handshake`.
fn resolve_mode(raw: Option<&str>) -> Result<Mode> {
    match raw {
        Some("handshake") => Ok(Mode::Handshake),
        Some("env") | Some("") | None => Ok(Mode::Env),
        Some(other) => bail!("{MODE_ENV} must be `env` or `handshake`, got `{other}`"),
    }
}

/// The listen address: `--listen`, else `HARNESS_SHIM_LISTEN`, else the default.
fn resolve_listen(flag: Option<SocketAddr>, env: Option<&str>) -> Result<SocketAddr> {
    if let Some(addr) = flag {
        return Ok(addr);
    }
    let raw = env.filter(|v| !v.is_empty()).unwrap_or(DEFAULT_LISTEN);
    raw.parse()
        .map_err(|err| anyhow::anyhow!("invalid {LISTEN_ENV} `{raw}`: {err}"))
}

/// `--workspace`, else `HARNESS_WORKSPACE`, else `/workspace`. Must be absolute: the shim enters
/// it at startup, so a relative path would resolve against itself.
fn resolve_workspace(flag: Option<PathBuf>, env: Option<OsString>) -> Result<PathBuf> {
    let mount = flag
        .or_else(|| env.filter(|v| !v.is_empty()).map(PathBuf::from))
        .unwrap_or_else(|| harness_shim::workspace::WORKSPACE_MOUNT.into());
    if !mount.is_absolute() {
        bail!(
            "the workspace must be an absolute path, got `{}`",
            mount.display()
        );
    }
    Ok(mount)
}

/// Creates `$HOME` so the harness has somewhere to keep its state; failure only warns, because
/// the provider may mount a ready-made home.
fn ensure_home_dir() {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    if let Err(err) = std::fs::create_dir_all(&home) {
        tracing::warn!(%err, ?home, "could not create $HOME");
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    // Before anything is spawned: processes the harness leaves behind stay in the shim's tree.
    if let Err(err) = harness_shim::reaper::adopt_orphans() {
        tracing::warn!(%err, "processes the session leaves behind escape its stats and its stop");
    }
    let mode = resolve_mode(std::env::var(MODE_ENV).ok().as_deref())?;
    let argv = match mode {
        Mode::Env => Some(resolve_command(cli.command)?),
        // A handshake shim is configured later; a command given here is only the fallback.
        Mode::Handshake => resolve_command(cli.command).ok(),
    };
    let mount = resolve_workspace(cli.workspace, std::env::var_os(WORKSPACE_ENV))?;
    // A fresh standalone shim has neither directory yet; both must exist before entering.
    ensure_home_dir();
    if let Err(err) = std::fs::create_dir_all(&mount) {
        tracing::warn!(%err, ?mount, "could not create the workspace directory");
    }
    // Children inherit the cwd, so each session's harness starts in its own directory.
    if let Err(err) = std::env::set_current_dir(&mount) {
        tracing::warn!(%err, ?mount, "could not enter the workspace directory");
    }
    let config = Config {
        listen: resolve_listen(cli.listen, std::env::var(LISTEN_ENV).ok().as_deref())?,
        buffer_lines: cli.buffer_lines,
        argv,
        mode,
        token: std::env::var(SHIM_TOKEN_ENV).ok().filter(|t| !t.is_empty()),
        workspace: harness_shim::workspace::CloneSpec::from_env(mount.clone()),
        workspace_mount: mount,
        files: harness_shim::files::from_env()?,
    };
    let exit_code = run(config, None).await?;
    std::process::exit(exit_code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_defaults_to_env_and_rejects_unknown_values() {
        assert_eq!(resolve_mode(None).unwrap(), Mode::Env);
        assert_eq!(resolve_mode(Some("")).unwrap(), Mode::Env);
        assert_eq!(resolve_mode(Some("env")).unwrap(), Mode::Env);
        assert_eq!(resolve_mode(Some("handshake")).unwrap(), Mode::Handshake);
        assert!(resolve_mode(Some("other")).is_err());
    }

    #[test]
    fn listen_prefers_the_flag_then_the_env_then_the_default() {
        let flag: SocketAddr = "127.0.0.1:1".parse().unwrap();
        assert_eq!(
            resolve_listen(Some(flag), Some("127.0.0.1:2")).unwrap(),
            flag
        );
        assert_eq!(resolve_listen(None, Some("127.0.0.1:2")).unwrap().port(), 2);
        assert_eq!(resolve_listen(None, None).unwrap().port(), 7000);
        assert_eq!(resolve_listen(None, Some("")).unwrap().port(), 7000);
        assert!(resolve_listen(None, Some("nope")).is_err());
    }

    #[test]
    fn workspace_prefers_the_flag_then_the_env_then_the_default() {
        let flag = Some(PathBuf::from("/w/flag"));
        let env = || Some(OsString::from("/w/env"));
        assert_eq!(
            resolve_workspace(flag, env()).unwrap(),
            PathBuf::from("/w/flag")
        );
        assert_eq!(
            resolve_workspace(None, env()).unwrap(),
            PathBuf::from("/w/env")
        );
        assert_eq!(
            resolve_workspace(None, None).unwrap(),
            PathBuf::from("/workspace")
        );
        assert_eq!(
            resolve_workspace(None, Some(OsString::new())).unwrap(),
            PathBuf::from("/workspace")
        );
    }

    #[test]
    fn a_relative_workspace_is_refused() {
        assert!(resolve_workspace(Some("rel/dir".into()), None).is_err());
        assert!(resolve_workspace(Some("".into()), None).is_err());
        assert!(resolve_workspace(None, Some("rel".into())).is_err());
    }
}
