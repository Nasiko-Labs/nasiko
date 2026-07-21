use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

use anyhow::{Result, bail};
use tabled::settings::{Alignment, Style};
use tabled::{Table, Tabled};

use crate::config;
use crate::util::container_bin;

const COMPOSE_YAML: &str = include_str!("../../../docker-compose.infra.yml");
const CP_IMAGE_DEFAULT: &str = "nasiko/cp:latest";
const CP_BINARY_NAME: &str = "nasiko-cp";
const CP_BINARY_PATH_IN_IMAGE: &str = "/nasiko-cp-cloud";

// ─── DevEnv ────────────────────────────────────────────────────────────────

struct EnvVar {
    key: &'static str,
    description: &'static str,
    default: Option<&'static str>,
    secret: bool,
}

const DEV_ENV_VARS: &[EnvVar] = &[
    EnvVar {
        key: "DOCKERHUB_USER",
        description: "DockerHub org for CP image",
        default: Some("nasiko"),
        secret: false,
    },
    EnvVar {
        key: "OPENAI_API_KEY",
        description: "AI routing + card generation",
        default: None,
        secret: true,
    },
    EnvVar {
        key: "OPENAI_BASE_URL",
        description: "Custom LLM endpoint",
        default: None,
        secret: false,
    },
    EnvVar {
        key: "OPENAI_MODEL",
        description: "Default model for agents",
        default: Some("deepseek-v4-flash"),
        secret: false,
    },
    EnvVar {
        key: "ROUTER_MODEL",
        description: "Model for smart routing",
        default: Some("deepseek-v4-pro"),
        secret: false,
    },
    EnvVar {
        key: "GITHUB_CLIENT_ID",
        description: "GitHub OAuth (login with GitHub)",
        default: None,
        secret: false,
    },
    EnvVar {
        key: "GITHUB_CLIENT_SECRET",
        description: "GitHub OAuth secret",
        default: None,
        secret: true,
    },
    EnvVar {
        key: "SEED_AGENTS",
        description: "Auto-deploy these images on start",
        default: None,
        secret: false,
    },
];

#[derive(Tabled)]
struct EnvVarTableRow {
    #[tabled(rename = "")]
    num: String,
    #[tabled(rename = "KEY")]
    key: String,
    #[tabled(rename = "VALUE")]
    value: String,
    #[tabled(rename = "DESCRIPTION")]
    description: String,
}

struct DevEnv {
    vars: HashMap<String, String>,
}

impl DevEnv {
    fn load() -> Self {
        let file_vars = load_env_file(&dev_env_path());
        let mut vars = HashMap::new();

        for ev in DEV_ENV_VARS {
            let value = std::env::var(ev.key)
                .ok()
                .filter(|s| !s.is_empty())
                .or_else(|| file_vars.get(ev.key).cloned())
                .or_else(|| ev.default.map(String::from));

            if let Some(v) = value {
                vars.insert(ev.key.to_string(), v);
            }
        }

        Self { vars }
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(|s| s.as_str())
    }

    fn print(&self) {
        let path = dev_env_path();
        println!("\nConfiguration ({}):", path.display());
        let rows: Vec<EnvVarTableRow> = DEV_ENV_VARS
            .iter()
            .enumerate()
            .map(|(i, ev)| {
                let display = match self.vars.get(ev.key) {
                    Some(v) if ev.secret && v.len() > 8 => {
                        format!("{}...{}", &v[..4], &v[v.len() - 4..])
                    }
                    Some(_v) if ev.secret => "****".to_string(),
                    Some(v) => v.clone(),
                    None => "(not set)".to_string(),
                };
                EnvVarTableRow {
                    num: format!("{}.", i + 1),
                    key: ev.key.to_string(),
                    value: display,
                    description: ev.description.to_string(),
                }
            })
            .collect();
        println!(
            "{}",
            Table::new(rows)
                .with(Style::blank())
                .with(Alignment::left())
        );
        println!();
    }

    fn interactive_edit(&mut self) -> Result<()> {
        loop {
            let input: String = dialoguer::Input::new()
                .with_prompt("Edit? [number to change, Enter to proceed, q to quit]")
                .default(String::new())
                .allow_empty(true)
                .interact_text()?;

            let input = input.trim();
            if input.is_empty() {
                return Ok(());
            }
            if input == "q" || input == "quit" {
                bail!("aborted");
            }

            let idx: usize = match input.parse::<usize>() {
                Ok(n) if n >= 1 && n <= DEV_ENV_VARS.len() => n - 1,
                _ => {
                    println!("  Invalid. Enter 1-{}.", DEV_ENV_VARS.len());
                    continue;
                }
            };

            let ev = &DEV_ENV_VARS[idx];
            let current = self.vars.get(ev.key).cloned().unwrap_or_default();

            let new_value: String = if ev.secret {
                dialoguer::Password::new()
                    .with_prompt(ev.key)
                    .allow_empty_password(true)
                    .interact()?
            } else {
                dialoguer::Input::new()
                    .with_prompt(ev.key)
                    .default(current)
                    .allow_empty(true)
                    .interact_text()?
            };

            let new_value = new_value.trim().to_string();
            if new_value.is_empty() {
                self.vars.remove(ev.key);
            } else {
                self.vars.insert(ev.key.to_string(), new_value);
            }

            self.print();
        }
    }

    fn save_to_file(&self) -> Result<()> {
        let path = dev_env_path();
        let mut content = String::from("# ~/.nasiko/.env — nasiko configuration\n");
        content.push_str("# Shell env vars take precedence over this file.\n\n");

        for ev in DEV_ENV_VARS {
            content.push_str(&format!("# {}\n", ev.description));
            match self.vars.get(ev.key) {
                Some(v) => content.push_str(&format!("{}={}\n\n", ev.key, v)),
                None => content.push_str(&format!("# {}=\n\n", ev.key)),
            }
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &content)?;
        Ok(())
    }
}

// ─── Commands ──────────────────────────────────────────────────────────────

pub fn start(infra_only: bool) -> Result<()> {
    ensure_dev_env_file()?;
    let mut env = DevEnv::load();
    env.print();
    env.interactive_edit()?;
    env.save_to_file()?;

    let compose_path = compose_file_path()?;
    std::fs::write(&compose_path, COMPOSE_YAML)?;

    println!("Starting infrastructure...");
    let status = Command::new(container_bin())
        .args(["compose", "-f", &compose_path.to_string_lossy(), "up", "-d"])
        .status()?;
    if !status.success() {
        bail!("container compose up failed");
    }

    println!("  Postgres  localhost:5432");
    println!("  Redis     localhost:6379");
    println!("  RustFS    http://localhost:9000");

    if infra_only {
        println!("\nInfrastructure ready (infra-only mode).");
        println!("Run the CP yourself:");
        println!("  cargo run -p nasiko-cp");
        return Ok(());
    }

    // Extract and run CP binary
    let cp_bin = ensure_cp_binary(&env)?;

    println!("\nStarting CP...");
    let pid_file = pid_file_path()?;

    let mut cmd = Command::new(&cp_bin);
    cmd.env("CP_BIND", "0.0.0.0:8080")
        .env(
            "DATABASE_URL",
            "postgresql://nasiko:nasiko@localhost:5432/nasiko_dev",
        )
        .env("REDIS_URL", "redis://localhost:6379")
        .env("AGENT_RUNTIME", "local")
        .env("S3_ENDPOINT", "http://localhost:9000")
        .env("S3_BUCKET", "nasiko")
        .env("S3_ACCESS_KEY", "nasiko")
        .env("S3_SECRET_KEY", "nasiko123")
        .env("S3_REGION", "us-east-1")
        // Valid base64-encoded 32 bytes (32x 'D') — SecretsCrypto::from_key requires
        // exactly 32 decoded bytes, unlike a raw 32-character string.
        .env(
            "SECRETS_ENCRYPTION_KEY",
            "REREREREREREREREREREREREREREREREREREREREREQ=",
        )
        .env("RUST_LOG", "info,nasiko_cp_lib=debug");

    // Pass optional env vars from DevEnv
    for key in &[
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "OPENAI_MODEL",
        "ROUTER_MODEL",
        "GITHUB_CLIENT_ID",
        "GITHUB_CLIENT_SECRET",
        "SEED_AGENTS",
    ] {
        if let Some(val) = env.get(key) {
            cmd.env(key, val);
        }
    }

    let child = cmd.spawn()?;
    std::fs::write(&pid_file, child.id().to_string())?;

    // Wait for CP to be healthy
    print!("  Waiting for CP...");
    let http = ureq::Agent::new_with_defaults();
    let mut healthy = false;
    for _ in 0..30 {
        thread::sleep(Duration::from_secs(1));
        if let Ok(resp) = http.get("http://localhost:8080/health").call()
            && resp.status().as_u16() == 200
        {
            println!(" ok");
            healthy = true;
            break;
        }
        print!(".");
    }
    if !healthy {
        println!(" timeout");
        eprintln!("  Warning: CP did not become healthy in 30s");
    }

    println!("  CP        http://localhost:8080");

    // Auto-connect
    let _ = config::connect("local", "http://localhost:8080");
    println!("\nLocal platform running. Connected as 'local'.");
    println!("  nasiko auth login    # admin / admin (default)");
    Ok(())
}

fn ensure_dev_env_file() -> Result<bool> {
    let path = dev_env_path();
    if path.exists() {
        return Ok(false);
    }

    let mut content = String::from("# ~/.nasiko/.env — nasiko configuration\n");
    content.push_str("# Shell env vars take precedence over this file.\n\n");

    for ev in DEV_ENV_VARS {
        content.push_str(&format!("# {}\n", ev.description));
        match ev.default {
            Some(d) => content.push_str(&format!("{}={}\n\n", ev.key, d)),
            None => content.push_str(&format!("# {}=\n\n", ev.key)),
        }
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, &content)?;
    Ok(true)
}

pub fn stop() -> Result<()> {
    // Stop CP process
    let pid_file = pid_file_path()?;
    if pid_file.exists() {
        if let Ok(pid_str) = std::fs::read_to_string(&pid_file)
            && let Ok(pid) = pid_str.trim().parse::<u32>()
        {
            println!("Stopping CP (pid {pid})...");
            let _ = Command::new("kill").arg(pid.to_string()).status();
        }
        let _ = std::fs::remove_file(&pid_file);
    }

    // Stop compose
    let compose_path = compose_file_path()?;
    if !compose_path.exists() {
        std::fs::write(&compose_path, COMPOSE_YAML)?;
    }

    println!("Stopping infrastructure...");
    let status = Command::new(container_bin())
        .args(["compose", "-f", &compose_path.to_string_lossy(), "down"])
        .status()?;
    if !status.success() {
        bail!("container compose down failed");
    }
    println!("Done.");
    Ok(())
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
    if env_file.exists() {
        run_args.push("--env-file".into());
        run_args.push(env_file.to_string_lossy().into_owned());
    }
    run_args.push(image.clone());
    let run = Command::new(&bin).args(&run_args).status()?;
    if !run.success() {
        bail!("{bin} run failed");
    }

    println!("{name} → http://localhost:{port}");
    Ok(())
}

// ─── Helpers ───────────────────────────────────────────────────────────────

fn ensure_cp_binary(env: &DevEnv) -> Result<std::path::PathBuf> {
    let bin_dir = nasiko_bin_dir()?;
    let bin_path = bin_dir.join(CP_BINARY_NAME);

    if bin_path.exists() {
        return Ok(bin_path);
    }

    let image = env
        .get("DOCKERHUB_USER")
        .map(|user| format!("{user}/cp:latest"))
        .unwrap_or_else(|| CP_IMAGE_DEFAULT.to_string());

    let bin = container_bin();

    println!("  Pulling CP image ({image})...");
    let status = Command::new(&bin).args(["pull", &image]).status()?;
    if !status.success() {
        bail!("failed to pull {image}");
    }

    println!("  Extracting CP binary...");
    let output = Command::new(&bin).args(["create", &image]).output()?;
    if !output.status.success() {
        bail!("{bin} create failed");
    }
    let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();

    let cp_status = Command::new(&bin)
        .args([
            "cp",
            &format!("{container_id}:{CP_BINARY_PATH_IN_IMAGE}"),
            &bin_path.to_string_lossy(),
        ])
        .status()?;

    let _ = Command::new(&bin).args(["rm", &container_id]).output();

    if !cp_status.success() {
        bail!("failed to extract CP binary from image");
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755))?;
    }

    println!("  Cached at {}", bin_path.display());
    Ok(bin_path)
}

fn dev_env_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".nasiko")
        .join(".env")
}

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

fn nasiko_bin_dir() -> Result<std::path::PathBuf> {
    let dir = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".nasiko")
        .join("bin");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn compose_file_path() -> Result<std::path::PathBuf> {
    let dir = dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"))
        .join("nasiko");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("docker-compose.dev.yml"))
}

fn pid_file_path() -> Result<std::path::PathBuf> {
    let dir = dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"))
        .join("nasiko");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("cp.pid"))
}
