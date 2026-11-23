//! The only eval module that runs `docker` (or `podman`, via `util::container_bin`).
//!
//! Behind the [`Containers`] trait so the build, gate and replay flows can be tested with a
//! fake, and so a Kubernetes-job implementation can be added later.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::util::container_bin;

/// A label every eval container, network and image carries, so `nasiko eval clean` finds them.
pub const LABEL_KEY: &str = "nasiko.eval";

/// One container run to completion.
#[derive(Debug, Clone)]
pub struct JobSpec {
    pub name: String,
    pub image: String,
    pub cmd: Vec<String>,
    pub network: Network,
    pub mounts: Vec<Mount>,
    pub env: Vec<(String, String)>,
    pub limits: JobLimits,
    /// Value of the [`LABEL_KEY`] label, e.g. the run id.
    pub label: String,
    /// Run inside the container (`sh -c`) before it is stopped for a limit, to keep its work
    /// (e.g. save the agent's diff); `None` for nothing.
    pub before_stop: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Network {
    None,
    Named(String),
}

#[derive(Debug, Clone)]
pub struct Mount {
    pub host: PathBuf,
    pub container: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct JobLimits {
    pub wall: Duration,
    pub cpus: f32,
    pub memory_mib: u32,
    pub pids: u32,
    /// Most bytes the container may write to its own filesystem layer.
    pub disk_mib: u32,
}

/// How a job ended.
#[derive(Debug, Clone, PartialEq)]
pub struct JobOutcome {
    /// `None` when the job was stopped (time or disk limit) before it exited on its own.
    pub exit_code: Option<i32>,
    pub stopped: Option<StopReason>,
    pub stdout: String,
    pub stderr: String,
    pub wall: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    TimeLimit,
    DiskLimit,
}

/// An image to build from a folder.
pub struct ImageBuild {
    pub tag: String,
    pub context: PathBuf,
}

/// A long-running helper container (the model gateway).
pub struct ServiceSpec {
    pub name: String,
    pub image: String,
    pub cmd: Vec<String>,
    pub networks: Vec<String>,
    pub mounts: Vec<Mount>,
    pub label: String,
}

pub trait Containers: Send + Sync {
    fn image_exists(&self, tag: &str) -> Result<bool>;
    fn build(&self, image: &ImageBuild) -> Result<()>;
    fn run_to_end(&self, job: &JobSpec) -> Result<JobOutcome>;
    /// Creates an `--internal` network (no route out) unless it exists.
    fn ensure_internal_network(&self, name: &str, label: &str) -> Result<()>;
    fn start_service(&self, svc: &ServiceSpec) -> Result<()>;
    fn exec(&self, container: &str, cmd: &[&str]) -> Result<()>;
    fn stop_service(&self, name: &str) -> Result<()>;
    /// Removes every container and network carrying `LABEL_KEY=<label>` (any label when
    /// `None`). Returns how many objects were removed.
    fn remove_labelled(&self, label: Option<&str>) -> Result<usize>;
}

/// The real implementation: the `docker` program.
pub struct DockerCli {
    bin: String,
}

/// How often a running job's time and disk use are checked.
const POLL: Duration = Duration::from_secs(5);
/// Disk use is read every this many polls (`docker ps --size` is slow).
const DISK_EVERY: u32 = 3;

impl DockerCli {
    pub fn new() -> Self {
        Self {
            bin: container_bin(),
        }
    }

    fn cmd(&self) -> Command {
        Command::new(&self.bin)
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let out = self
            .cmd()
            .args(args)
            .output()
            .with_context(|| format!("failed to run {}; is it installed?", self.bin))?;
        if !out.status.success() {
            bail!(
                "{} {} failed: {}",
                self.bin,
                args.first().unwrap_or(&""),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Bytes written to the container's own layer, from `docker ps --size` ("12.3MB (virtual …)").
    fn writable_bytes(&self, name: &str) -> Option<u64> {
        let filter = format!("name=^{name}$");
        let out = self
            .run(&[
                "ps",
                "--all",
                "--size",
                "--filter",
                &filter,
                "--format",
                "{{.Size}}",
            ])
            .ok()?;
        parse_size(out.split('(').next()?.trim())
    }

    fn state(&self, name: &str) -> Result<(bool, i32)> {
        let out = self.run(&[
            "inspect",
            "-f",
            "{{.State.Running}} {{.State.ExitCode}}",
            name,
        ])?;
        let mut parts = out.split_whitespace();
        let running = parts.next() == Some("true");
        let code = parts.next().and_then(|c| c.parse().ok()).unwrap_or(-1);
        Ok((running, code))
    }
}

impl Default for DockerCli {
    fn default() -> Self {
        Self::new()
    }
}

/// The `docker run` arguments for a job (without the binary), detached.
pub fn run_args(job: &JobSpec) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "-d".into(),
        "--name".into(),
        job.name.clone(),
        "--label".into(),
        format!("{LABEL_KEY}={}", job.label),
        "--cap-drop".into(),
        "ALL".into(),
        "--security-opt".into(),
        "no-new-privileges".into(),
        "--cpus".into(),
        format!("{}", job.limits.cpus),
        "--memory".into(),
        format!("{}m", job.limits.memory_mib),
        "--pids-limit".into(),
        job.limits.pids.to_string(),
        "--network".into(),
        match &job.network {
            Network::None => "none".into(),
            Network::Named(n) => n.clone(),
        },
    ];
    for m in &job.mounts {
        let mode = if m.read_only { ":ro" } else { "" };
        args.push("-v".into());
        args.push(format!("{}:{}{mode}", m.host.display(), m.container));
    }
    for (k, v) in &job.env {
        args.push("-e".into());
        args.push(format!("{k}={v}"));
    }
    args.push(job.image.clone());
    args.extend(job.cmd.iter().cloned());
    args
}

impl Containers for DockerCli {
    fn image_exists(&self, tag: &str) -> Result<bool> {
        Ok(self
            .cmd()
            .args(["image", "inspect", tag])
            .output()
            .context("failed to run docker image inspect")?
            .status
            .success())
    }

    fn build(&self, image: &ImageBuild) -> Result<()> {
        let status = self
            .cmd()
            .args([
                "build",
                "--label",
                &format!("{LABEL_KEY}=image"),
                "-t",
                &image.tag,
            ])
            .arg(&image.context)
            .status()
            .context("failed to run docker build")?;
        if !status.success() {
            bail!("docker build of {} failed", image.tag);
        }
        Ok(())
    }

    fn run_to_end(&self, job: &JobSpec) -> Result<JobOutcome> {
        let _ = self.run(&["rm", "-f", &job.name]);
        let args = run_args(job);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run(&refs)?;
        let started = Instant::now();
        let disk_limit = u64::from(job.limits.disk_mib) * 1024 * 1024;
        let mut stopped = None;
        let mut polls = 0u32;
        let exit_code = loop {
            let (running, code) = self.state(&job.name)?;
            if !running {
                break Some(code);
            }
            if started.elapsed() > job.limits.wall {
                stopped = Some(StopReason::TimeLimit);
            } else if polls.is_multiple_of(DISK_EVERY)
                && self
                    .writable_bytes(&job.name)
                    .is_some_and(|b| b > disk_limit)
            {
                stopped = Some(StopReason::DiskLimit);
            }
            if stopped.is_some() {
                if let Some(cmd) = &job.before_stop {
                    let _ = self.run(&["exec", &job.name, "sh", "-c", cmd]);
                }
                let _ = self.run(&["kill", &job.name]);
                break None;
            }
            polls += 1;
            std::thread::sleep(POLL);
        };
        let logs = self
            .cmd()
            .args(["logs", &job.name])
            .output()
            .context("failed to read container logs")?;
        let _ = self.run(&["rm", "-f", &job.name]);
        Ok(JobOutcome {
            exit_code,
            stopped,
            stdout: String::from_utf8_lossy(&logs.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&logs.stderr).into_owned(),
            wall: started.elapsed(),
        })
    }

    fn ensure_internal_network(&self, name: &str, label: &str) -> Result<()> {
        if self.run(&["network", "inspect", name]).is_ok() {
            return Ok(());
        }
        self.run(&[
            "network",
            "create",
            "--internal",
            "--label",
            &format!("{LABEL_KEY}={label}"),
            name,
        ])
        .map(|_| ())
    }

    fn start_service(&self, svc: &ServiceSpec) -> Result<()> {
        let _ = self.run(&["rm", "-f", &svc.name]);
        let mut args: Vec<String> = vec![
            "run".into(),
            "-d".into(),
            "--name".into(),
            svc.name.clone(),
            "--label".into(),
            format!("{LABEL_KEY}={}", svc.label),
        ];
        for m in &svc.mounts {
            let mode = if m.read_only { ":ro" } else { "" };
            args.push("-v".into());
            args.push(format!("{}:{}{mode}", m.host.display(), m.container));
        }
        args.push(svc.image.clone());
        args.extend(svc.cmd.iter().cloned());
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run(&refs)?;
        for net in &svc.networks {
            self.run(&["network", "connect", net, &svc.name])?;
        }
        Ok(())
    }

    fn exec(&self, container: &str, cmd: &[&str]) -> Result<()> {
        let mut args = vec!["exec", container];
        args.extend_from_slice(cmd);
        self.run(&args).map(|_| ())
    }

    fn stop_service(&self, name: &str) -> Result<()> {
        self.run(&["rm", "-f", name]).map(|_| ())
    }

    fn remove_labelled(&self, label: Option<&str>) -> Result<usize> {
        let filter = match label {
            Some(l) => format!("label={LABEL_KEY}={l}"),
            None => format!("label={LABEL_KEY}"),
        };
        let containers: Vec<String> = self
            .run(&["ps", "-aq", "--filter", &filter])?
            .lines()
            .map(str::to_string)
            .collect();
        for id in &containers {
            let _ = self.run(&["rm", "-f", id]);
        }
        let networks: Vec<String> = self
            .run(&["network", "ls", "-q", "--filter", &filter])?
            .lines()
            .map(str::to_string)
            .collect();
        for id in &networks {
            let _ = self.run(&["network", "rm", id]);
        }
        Ok(containers.len() + networks.len())
    }
}

/// `"12.3MB"`, `"0B"`, `"1.2GB"`, `"512kB"` → bytes (decimal units, as Docker prints them).
fn parse_size(text: &str) -> Option<u64> {
    let split = text.find(|c: char| c.is_ascii_alphabetic())?;
    let (number, unit) = text.split_at(split);
    let number: f64 = number.trim().parse().ok()?;
    let factor = match unit.trim() {
        "B" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        _ => return None,
    };
    Some((number * factor) as u64)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A fake that records jobs and answers with canned outcomes, in order.
    #[derive(Default)]
    pub struct FakeContainers {
        pub jobs: Mutex<Vec<JobSpec>>,
        pub outcomes: Mutex<Vec<JobOutcome>>,
        pub built: Mutex<Vec<String>>,
    }

    impl Containers for FakeContainers {
        fn image_exists(&self, tag: &str) -> Result<bool> {
            Ok(self.built.lock().expect("lock").iter().any(|t| t == tag))
        }
        fn build(&self, image: &ImageBuild) -> Result<()> {
            self.built.lock().expect("lock").push(image.tag.clone());
            Ok(())
        }
        fn run_to_end(&self, job: &JobSpec) -> Result<JobOutcome> {
            self.jobs.lock().expect("lock").push(job.clone());
            let mut outcomes = self.outcomes.lock().expect("lock");
            if outcomes.is_empty() {
                bail!("no canned outcome left");
            }
            Ok(outcomes.remove(0))
        }
        fn ensure_internal_network(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        fn start_service(&self, _: &ServiceSpec) -> Result<()> {
            Ok(())
        }
        fn exec(&self, _: &str, _: &[&str]) -> Result<()> {
            Ok(())
        }
        fn stop_service(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn remove_labelled(&self, _: Option<&str>) -> Result<usize> {
            Ok(0)
        }
    }

    #[test]
    fn sizes_are_read_like_docker_prints_them() {
        assert_eq!(parse_size("0B"), Some(0));
        assert_eq!(parse_size("12.5MB"), Some(12_500_000));
        assert_eq!(parse_size("2.1GB"), Some(2_100_000_000));
        assert_eq!(parse_size("n/a"), None);
    }

    #[test]
    fn jobs_run_hardened_with_limits() {
        let job = JobSpec {
            name: "j".into(),
            image: "img".into(),
            cmd: vec!["bash".into(), "/eval/run.sh".into()],
            network: Network::None,
            mounts: vec![Mount {
                host: "/h".into(),
                container: "/eval".into(),
                read_only: true,
            }],
            env: vec![("A".into(), "1".into())],
            limits: JobLimits {
                wall: Duration::from_secs(60),
                cpus: 4.0,
                memory_mib: 4096,
                pids: 2048,
                disk_mib: 2048,
            },
            label: "run-1".into(),
            before_stop: None,
        };
        let args = run_args(&job).join(" ");
        for part in [
            "--cap-drop ALL",
            "--security-opt no-new-privileges",
            "--network none",
            "--memory 4096m",
            "-v /h:/eval:ro",
            "--label nasiko.eval=run-1",
            "img bash /eval/run.sh",
        ] {
            assert!(args.contains(part), "missing {part:?} in {args}");
        }
    }
}
