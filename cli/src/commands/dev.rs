use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use nasiko_utils::term;

use crate::config;
use crate::util::container_bin;

/// Where `nasiko up` serves, as the repo's `docker-compose.yml` publishes it.
const LOCAL_URL: &str = "http://localhost:8080";
/// How long `nasiko up` waits for `/health` once Compose has started the stack: the server
/// runs its migrations before it answers.
const HEALTH_WAIT: Duration = Duration::from_secs(120);

// ─── Commands ──────────────────────────────────────────────────────────────

/// `nasiko up`: the README's quick start, run for you. Builds and starts the repo's Compose
/// stack (server, Postgres, Redis, object storage, telemetry) from the checkout this is run in,
/// creating `.env` from `.env.example` the first time, then registers it as the `local` cluster.
pub fn start() -> Result<()> {
    let root = find_checkout()?;
    let env = root.join(".env");
    if !env.exists() {
        std::fs::copy(root.join(".env.example"), &env)
            .with_context(|| format!("could not create {}", env.display()))?;
        println!(
            "Created {} from .env.example. Its defaults work on this machine; change them before \
             anyone else can reach the server.",
            env.display()
        );
    }

    println!(
        "Starting Nasiko from {} (the first build takes a few minutes)...",
        root.display()
    );
    compose(&root, &["up", "-d", "--build"])?;

    print!("  Waiting for the server...");
    let http = ureq::Agent::new_with_defaults();
    let mut waited = Duration::ZERO;
    let healthy = loop {
        if let Ok(resp) = http.get(&format!("{LOCAL_URL}/health")).call()
            && resp.status().as_u16() == 200
        {
            break true;
        }
        if waited >= HEALTH_WAIT {
            break false;
        }
        thread::sleep(Duration::from_secs(2));
        waited += Duration::from_secs(2);
        print!(".");
    };
    if healthy {
        println!(" ok");
    } else {
        println!(" timeout");
        term::print_warning(&format!(
            "the server did not answer within {}s; see `docker compose logs -f server` in {}",
            HEALTH_WAIT.as_secs(),
            root.display()
        ));
    }

    config::connect("local", LOCAL_URL)?;
    println!("\nNasiko is running at {LOCAL_URL}, registered as cluster 'local'.");
    println!("  nasiko auth login    # admin / changeme, or the ADMIN_PASSWORD in .env");
    Ok(())
}

/// `nasiko down`: stops the Compose stack `nasiko up` started. Data stays in its volumes.
pub fn stop() -> Result<()> {
    let root = find_checkout()?;
    println!("Stopping Nasiko in {}...", root.display());
    compose(&root, &["down"])?;
    println!("Done. Data is kept; `docker compose down -v` in that directory deletes it.");
    Ok(())
}

/// Runs `docker compose <args>` (or Podman's) in `root`, so the project name and `.env` are the
/// ones a user running Compose there by hand would get.
fn compose(root: &Path, args: &[&str]) -> Result<()> {
    let bin = container_bin();
    let status = Command::new(&bin)
        .arg("compose")
        .args(args)
        .current_dir(root)
        .status()
        .with_context(|| format!("could not run `{bin} compose`; is Docker installed?"))?;
    if !status.success() {
        bail!("`{bin} compose {}` failed", args.join(" "));
    }
    Ok(())
}

/// The Nasiko checkout holding the current directory: the nearest directory, this one or above,
/// with the server's `docker-compose.yml` and `server/Dockerfile`, or that pair under `oss/`.
fn find_checkout() -> Result<PathBuf> {
    checkout_above(&std::env::current_dir()?).ok_or_else(|| {
        anyhow::anyhow!(
            "no Nasiko checkout here. `nasiko up` runs the server from a clone of the \
                 repository:\n  git clone https://github.com/Nasiko-Labs/nasiko.git && cd nasiko \
                 && nasiko up"
        )
    })
}

fn checkout_above(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .flat_map(|dir| [dir.to_path_buf(), dir.join("oss")])
        .find(|dir| is_checkout(dir))
}

fn is_checkout(dir: &Path) -> bool {
    dir.join("docker-compose.yml").is_file() && dir.join("server/Dockerfile").is_file()
}

pub fn run(path: &str, port: u16) -> Result<()> {
    let agent_dir = Path::new(path).canonicalize()?;
    let name = agent_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("agent");
    let image = format!("nasiko/{name}:dev");

    if !agent_dir.join("Dockerfile").exists() {
        bail!("no Dockerfile found in {}", agent_dir.display());
    }

    check_rust_prebuild(&agent_dir)?;

    let bin = container_bin();

    println!("Building {image}...");
    let build = Command::new(&bin)
        .args(["build", "-t", &image, &agent_dir.to_string_lossy()])
        .status()?;
    if !build.success() {
        bail!("{bin} build failed");
    }

    let _ = Command::new(&bin).args(["rm", "-f", name]).output();
    println!("Running {name} on port {port}...");
    // Agent images serve on the canonical container port 8000 (the server's
    // DEFAULT_AGENT_PORT); `--port` only picks the host-side port. Pass the
    // project's .env so locally-run agents get their credentials, matching
    // what the scaffold's README and .env.example set up.
    let mut run_args: Vec<String> = vec![
        "run".into(),
        "-d".into(),
        "--name".into(),
        name.into(),
        "-p".into(),
        format!("{port}:8000"),
    ];
    let env_file = agent_dir.join(".env");
    let local_env = load_env_file(&env_file);
    if env_file.exists() {
        run_args.push("--env-file".into());
        run_args.push(env_file.to_string_lossy().into_owned());
    }
    // Same platform fallbacks the CP injects at deploy time (agent_env),
    // fetched from the connected CP so local runs behave like deployed ones.
    // Explicit `-e` beats `--env-file` in docker/podman, so only pass keys
    // the local .env doesn't already set — local values stay authoritative.
    for (key, value) in fetch_cp_fallback_env() {
        if !local_env.contains_key(&key) {
            run_args.push("-e".into());
            run_args.push(format!("{key}={value}"));
        }
    }
    run_args.push(image.clone());
    let run = Command::new(&bin).args(&run_args).status()?;
    if !run.success() {
        bail!("{bin} run failed");
    }

    // `docker run -d` returning success only means the create+start API call
    // was accepted — it does not guarantee the container is actually up.
    // Under contention (a container by this name mid-removal, a port still
    // held by the previous instance) it can come back stuck in `Created`,
    // never reaching `running`, while still reporting success here — the
    // caller would otherwise see a false "ready" message and then get
    // connection-refused from every subsequent `nasiko chat`. Poll briefly
    // and surface that failure now, with the known fix, instead of later.
    if !wait_for_running(&bin, name, Duration::from_secs(5)) {
        bail!(
            "{name} did not reach a running state (stuck in Created/exited) — \
             run `{bin} rm -f {name}` and try `nasiko run` again"
        );
    }
    // Reaching `running` only means the process launched, not that it's
    // actually serving yet — a separate, non-fatal check for that.
    wait_for_agent_healthy(&bin, name, port);

    println!("{name} → http://localhost:{port}");
    Ok(())
}

/// Poll `docker inspect` for a container's `.State.Status` until it reports
/// `running` or the timeout elapses.
fn wait_for_running(bin: &str, name: &str, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        let output = Command::new(bin)
            .args(["inspect", "--format", "{{.State.Status}}", name])
            .output();
        if let Ok(output) = output
            && output.status.success()
            && String::from_utf8_lossy(&output.stdout).trim() == "running"
        {
            return true;
        }
        thread::sleep(Duration::from_millis(200));
    }
    false
}

/// Poll the freshly-started container for a few seconds — `docker run -d`
/// only confirms the process launched, not that it stayed up or is actually
/// serving. Unlike `start()`'s CP health check, this doesn't fail the
/// command: it's a locally-run dev container the user can inspect directly,
/// so a warning (with the crash reason when detectable) is more useful than
/// blocking `nasiko run` on it.
fn wait_for_agent_healthy(bin: &str, name: &str, port: u16) {
    // Any HTTP response — not specifically a 200, and not specifically on
    // one well-known path — is enough evidence the process is alive and
    // serving: the exact card path differs across a2a-sdk versions (e.g.
    // `/.well-known/agent.json` vs `/.well-known/agent-card.json`), so
    // requiring a match on one of them would false-negative on a perfectly
    // healthy agent using the other. `http_status_as_error(false)` makes a
    // 404 come back as `Ok` too — only a real connection failure (not
    // listening yet, or crashed) is `Err`.
    let http = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .http_status_as_error(false)
            .build(),
    );
    for _ in 0..10 {
        thread::sleep(Duration::from_millis(500));

        let running = Command::new(bin)
            .args(["inspect", "-f", "{{.State.Running}}", name])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "true")
            .unwrap_or(false);
        if !running {
            term::print_warning("container exited immediately — check logs:");
            eprintln!("    {bin} logs {name}");
            return;
        }

        if http.get(format!("http://localhost:{port}/")).call().is_ok() {
            return;
        }
    }
    term::print_warning(&format!(
        "container is running but isn't answering HTTP requests on port {port} \
         after 5s — it may still be starting, or it crashed after binding; check:"
    ));
    eprintln!("    {bin} logs {name}");
}

/// Fails fast with an actionable message when a Rust (`a2a-server-lf`)
/// agent's Dockerfile expects an already cross-compiled binary that hasn't
/// been built yet — `nasiko run` only runs `docker build`, it doesn't invoke
/// `cargo zigbuild` itself, so the alternative is a confusing raw Docker
/// "failed to calculate checksum... not found" error at the COPY step.
fn check_rust_prebuild(agent_dir: &Path) -> Result<()> {
    let dockerfile = std::fs::read_to_string(agent_dir.join("Dockerfile")).unwrap_or_default();
    for line in dockerfile.lines() {
        let line = line.trim();
        if !line.starts_with("COPY ") {
            continue;
        }
        let Some(src) = line.split_whitespace().nth(1) else {
            continue;
        };
        if !src.contains("target/") || !src.contains("/release/") {
            continue;
        }
        if agent_dir.join(src).exists() {
            break;
        }
        let target_triple = src
            .split("target/")
            .nth(1)
            .and_then(|rest| rest.split("/release/").next())
            .unwrap_or("<target-triple>");
        bail!(
            "missing pre-built binary: {src}\n\n\
             This is a Rust agent — `nasiko run` builds the Docker image from an already \
             cross-compiled binary, it doesn't invoke `cargo zigbuild` itself. Build it first:\n\n\
             \x20\x20cargo zigbuild --release --target {target_triple}\n\n\
             (one-time setup: `cargo install cargo-zigbuild`, `brew install zig`, \
             `rustup target add {target_triple}`)"
        );
    }
    Ok(())
}

/// Fetch the platform fallback env vars the CP would inject at deploy time
/// (`GET /api/agents/dev-env`). Best-effort: local runs must keep working
/// with no cluster connected or a CP that predates the endpoint, but say why
/// the fallbacks were skipped so a missing key is diagnosable.
fn fetch_cp_fallback_env() -> HashMap<String, String> {
    #[derive(serde::Deserialize)]
    struct DevEnvResponse {
        env: HashMap<String, String>,
    }

    let client = match crate::api::Client::from_active_cluster() {
        Ok(c) => c,
        Err(_) => return HashMap::new(), // no cluster connected — purely local run
    };
    match client.get_json_optional::<DevEnvResponse>("/agents/dev-env") {
        Ok(Some(resp)) if !resp.env.is_empty() => {
            let mut keys: Vec<&str> = resp.env.keys().map(String::as_str).collect();
            keys.sort_unstable();
            println!("  cp env: {}", keys.join(", "));
            resp.env
        }
        Ok(_) => HashMap::new(), // older CP without the endpoint, or nothing configured
        Err(e) => {
            println!("  cp env: skipped ({e:#})");
            HashMap::new()
        }
    }
}

// ─── Helpers ───────────────────────────────────────────────────────────────

fn load_env_file(path: &std::path::Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return map,
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            map.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::checkout_above;

    fn checkout_in(dir: &std::path::Path) {
        std::fs::create_dir_all(dir.join("server")).unwrap();
        std::fs::write(dir.join("docker-compose.yml"), "").unwrap();
        std::fs::write(dir.join("server/Dockerfile"), "").unwrap();
    }

    #[test]
    fn finds_the_checkout_from_a_subdirectory() {
        let root = tempfile::tempdir().unwrap();
        checkout_in(root.path());
        let nested = root.path().join("agents/my-agent");
        std::fs::create_dir_all(&nested).unwrap();

        assert_eq!(checkout_above(&nested).as_deref(), Some(root.path()));
    }

    #[test]
    fn finds_a_checkout_kept_under_oss() {
        let root = tempfile::tempdir().unwrap();
        checkout_in(&root.path().join("oss"));

        assert_eq!(checkout_above(root.path()), Some(root.path().join("oss")));
    }

    #[test]
    fn a_compose_file_alone_is_not_a_checkout() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("docker-compose.yml"), "").unwrap();

        assert_eq!(checkout_above(root.path()), None);
    }
}
