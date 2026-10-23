use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use nasiko_utils::term;

use crate::api::Client;
use crate::config;
use crate::util::container_bin;

/// How long each probe may take before it is reported as failed, so `doctor` never hangs on an
/// unreachable cluster or a stuck container daemon.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Preflight check-up: everything a real command would fail on partway through, checked up front
/// with the fix printed under each failure. Reuses the checks those commands already rely on
/// (`Client::health_check`, `config::token_expired`) rather than re-implementing them. Only reads
/// state — never changes config, starts anything, or logs in. The report is the command's output,
/// so all of it goes to stdout, each fix right under its problem; only the closing "N problem(s)
/// found" error goes to stderr.
pub fn doctor() -> Result<()> {
    let mut problems = 0;

    if docker_reachable() {
        term::print_success("Docker running");
    } else {
        problems += 1;
        failed("Docker not reachable");
        print_fix(&format!("start {} and try again", container_bin()));
    }

    match config::active_cluster() {
        Ok((name, entry)) => {
            // The reason tells a cluster that doesn't answer from one that answers with an error.
            match Client::health_check_with_timeout(&entry.url, Some(PROBE_TIMEOUT)) {
                Ok(()) => term::print_success(&format!("Cluster '{name}' reachable")),
                Err(error) => {
                    problems += 1;
                    failed(&format!("Cluster '{name}' ({}): {error:#}", entry.url));
                    print_fix("nasiko status");
                }
            }

            match entry.token.as_deref() {
                None => {
                    problems += 1;
                    failed("Not logged in");
                    print_fix("nasiko auth login");
                }
                Some(token) => match config::token_expired(token) {
                    Some(true) => {
                        problems += 1;
                        failed("Auth token expired");
                        print_fix("nasiko auth login");
                    }
                    Some(false) => term::print_success("Auth token valid"),
                    None => caution("Auth token present but its expiry can't be read"),
                },
            }
        }
        Err(_) => {
            problems += 1;
            failed("No active cluster");
            print_fix("nasiko up");
            print_fix("nasiko connect <url>");
        }
    }

    match config::artifact_registry_url() {
        Some(_) => term::print_success("Registry URL set"),
        // Advisory only: most commands don't need the artifact registry.
        None => {
            caution("Registry URL not set");
            print_fix("export NASIKO_REGISTRY_URL=<your registry>");
        }
    }

    if problems > 0 {
        println!();
        bail!("{problems} problem(s) found");
    }
    Ok(())
}

fn failed(problem: &str) {
    println!("{} {problem}", term::red("✗"));
}

fn caution(problem: &str) {
    println!("{} {problem}", term::yellow("⚠"));
}

fn print_fix(hint: &str) {
    println!("    {}", term::cyan(&format!("→ {hint}")));
}

/// Whether the container runtime's daemon actually answers (`docker info` / `podman info`), not
/// just whether the binary is on PATH. Killed and reported unreachable after [`PROBE_TIMEOUT`].
fn docker_reachable() -> bool {
    let Ok(mut child) = Command::new(container_bin())
        .arg("info")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}
