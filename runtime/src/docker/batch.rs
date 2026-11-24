//! [`BatchRunner`] over the local Docker daemon.
//!
//! A job is one container: created, given its inputs (`upload_to_container`), started, waited on
//! with its wall-clock and disk limits checked every few seconds, then its outputs copied out
//! (`download_from_container`), its log read and the container removed. Docker Desktop cannot
//! enforce `--storage-opt size`, so the disk limit is the container's writable layer size, read
//! with `inspect --size`.
//!
//! A [`BatchNetwork::RouterOnly`] job runs on its own `internal` network, which has no route
//! out. A gateway container (nginx) joins that network and the one the server is reachable on,
//! and forwards `/v1/*` to the server's model router and nothing else. The job authenticates
//! with its own scoped token; the gateway adds nothing to the request.

use std::collections::HashMap;
use std::io::Read;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bollard::Docker;
use bollard::container::{
    Config, CreateContainerOptions, DownloadFromContainerOptions, InspectContainerOptions,
    KillContainerOptions, ListContainersOptions, LogsOptions, RemoveContainerOptions,
    StartContainerOptions, UploadToContainerOptions, WaitContainerOptions,
};
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::models::HostConfig;
use bollard::network::{ConnectNetworkOptions, CreateNetworkOptions, ListNetworksOptions};
use futures_util::StreamExt;
use tracing::warn;

use super::{DockerRuntime, map_bollard_err};
use crate::batch::{
    BATCH_INPUT_DIR, BATCH_LOG_MAX_BYTES, BATCH_OUTPUT_DIR, BATCH_OUTPUT_MAX_BYTES,
    BATCH_ROUTER_URL_ENV, BatchFile, BatchJob, BatchNetwork, BatchOutcome, BatchRunner, BatchStop,
    tail,
};
use crate::error::{Result, RuntimeError};
use crate::{ContainerRuntime, validate_build_inputs};

/// Label every batch object carries, valued with the job's [`BatchJob::label`].
pub const BATCH_LABEL: &str = "nasiko.com/batch";
const POLL: Duration = Duration::from_secs(2);
/// Read the writable layer's size every this many polls (it is a slow call).
const DISK_EVERY: u32 = 5;
const GATEWAY_PORT: u16 = 8080;
const BEFORE_STOP_TIMEOUT: Duration = Duration::from_secs(120);

/// Where a [`BatchNetwork::RouterOnly`] job's gateway forwards to.
#[derive(Debug, Clone)]
pub struct DockerBatchConfig {
    /// The server's base URL as seen from a container on `upstream_network`, without `/v1`
    /// (e.g. `http://nasiko-server:8080`, or `http://host.docker.internal:8082` for a server
    /// running on the host). `None`: router-only jobs are refused.
    pub router_upstream: Option<String>,
    /// The Docker network the server is reachable on (`None` = the default bridge).
    pub upstream_network: Option<String>,
    /// The gateway's image.
    pub gateway_image: String,
}

impl Default for DockerBatchConfig {
    fn default() -> Self {
        Self {
            router_upstream: None,
            upstream_network: None,
            gateway_image: "nginx:1.27-alpine".to_owned(),
        }
    }
}

/// Runs batch jobs as containers on the local daemon.
pub struct DockerBatchRunner {
    runtime: DockerRuntime,
    config: DockerBatchConfig,
}

impl DockerBatchRunner {
    /// `runtime` supplies the daemon client and image build; `config` the router gateway.
    pub fn new(runtime: DockerRuntime, config: DockerBatchConfig) -> Self {
        Self { runtime, config }
    }

    fn client(&self) -> &Docker {
        &self.runtime.client
    }

    async fn image_exists(&self, tag: &str) -> bool {
        self.client().inspect_image(tag).await.is_ok()
    }

    async fn remove_container(&self, name: &str) {
        let _ = self
            .client()
            .remove_container(
                name,
                Some(RemoveContainerOptions {
                    force: true,
                    v: true,
                    ..Default::default()
                }),
            )
            .await;
    }

    /// Creates the job's internal network and its router gateway; returns the router URL the
    /// job dials and the objects to remove afterwards.
    async fn start_gateway(&self, job: &BatchJob, base: &str) -> Result<Gateway> {
        let upstream = self.config.router_upstream.as_deref().ok_or_else(|| {
            RuntimeError::Unsupported(
                "router-only batch jobs need the router's address (EVAL_ROUTER_URL)".to_owned(),
            )
        })?;
        let network = format!("{base}-net");
        let gateway = format!("{base}-gw");
        let labels = HashMap::from([(BATCH_LABEL, job.label.as_str())]);
        self.client()
            .create_network(CreateNetworkOptions {
                name: network.as_str(),
                internal: true,
                labels,
                ..Default::default()
            })
            .await
            .map_err(map_bollard_err)?;
        let made = Gateway {
            network: network.clone(),
            container: gateway.clone(),
        };
        let result = async {
            super::ensure_image_present(
                self.client(),
                &self.config.gateway_image,
                None,
                None,
                None,
                false,
            )
            .await?;
            let config = Config {
                image: Some(self.config.gateway_image.clone()),
                labels: Some(HashMap::from([(BATCH_LABEL.to_owned(), job.label.clone())])),
                host_config: Some(HostConfig {
                    network_mode: Some(network.clone()),
                    // nginx starts as root to drop to its worker user.
                    cap_drop: Some(vec!["ALL".to_owned()]),
                    cap_add: Some(vec![
                        "CHOWN".to_owned(),
                        "SETUID".to_owned(),
                        "SETGID".to_owned(),
                    ]),
                    security_opt: Some(vec!["no-new-privileges:true".to_owned()]),
                    memory: Some(256 * 1024 * 1024),
                    pids_limit: Some(256),
                    ..Default::default()
                }),
                ..Default::default()
            };
            self.client()
                .create_container(
                    Some(CreateContainerOptions {
                        name: gateway.as_str(),
                        platform: None,
                    }),
                    config,
                )
                .await
                .map_err(map_bollard_err)?;
            let conf = tar_of(&[BatchFile {
                path: "etc/nginx/nginx.conf".to_owned(),
                contents: gateway_conf(upstream).into_bytes(),
            }])?;
            self.client()
                .upload_to_container(
                    &gateway,
                    Some(UploadToContainerOptions {
                        path: "/",
                        ..Default::default()
                    }),
                    conf.into(),
                )
                .await
                .map_err(map_bollard_err)?;
            if let Some(upstream_net) = &self.config.upstream_network {
                self.client()
                    .connect_network(
                        upstream_net,
                        ConnectNetworkOptions {
                            container: gateway.as_str(),
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(map_bollard_err)?;
            } else {
                self.client()
                    .connect_network(
                        "bridge",
                        ConnectNetworkOptions {
                            container: gateway.as_str(),
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(map_bollard_err)?;
            }
            self.client()
                .start_container(&gateway, None::<StartContainerOptions<String>>)
                .await
                .map_err(map_bollard_err)?;
            Ok::<_, RuntimeError>(())
        }
        .await;
        if let Err(e) = result {
            self.stop_gateway(&made).await;
            return Err(e);
        }
        Ok(made)
    }

    async fn stop_gateway(&self, gateway: &Gateway) {
        self.remove_container(&gateway.container).await;
        if let Err(e) = self.client().remove_network(&gateway.network).await {
            warn!(network = %gateway.network, error = %e, "batch: could not remove a job network");
        }
    }

    /// Runs the job's container; the gateway (if any) is already up.
    async fn run_container(
        &self,
        job: &BatchJob,
        name: &str,
        network: &str,
    ) -> Result<BatchOutcome> {
        let host = HostConfig {
            network_mode: Some(network.to_owned()),
            cap_drop: Some(vec!["ALL".to_owned()]),
            security_opt: Some(vec!["no-new-privileges:true".to_owned()]),
            nano_cpus: Some((job.limits.cpus * 1e9) as i64),
            memory: Some((job.limits.memory_mib * 1024 * 1024) as i64),
            pids_limit: Some(job.limits.pids),
            ..Default::default()
        };
        let config = Config {
            image: Some(job.image.clone()),
            cmd: Some(job.command.clone()),
            env: Some(job.env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
            labels: Some(HashMap::from([(BATCH_LABEL.to_owned(), job.label.clone())])),
            host_config: Some(host),
            ..Default::default()
        };
        self.client()
            .create_container(
                Some(CreateContainerOptions {
                    name,
                    platform: None,
                }),
                config,
            )
            .await
            .map_err(map_bollard_err)?;
        let inputs: Vec<BatchFile> = job
            .inputs
            .iter()
            .map(|f| BatchFile {
                path: format!("{}/{}", BATCH_INPUT_DIR.trim_start_matches('/'), f.path),
                contents: f.contents.clone(),
            })
            .chain(std::iter::once(BatchFile {
                // An empty marker so the output folder exists, writable by the job's user.
                path: format!("{}/.keep", BATCH_OUTPUT_DIR.trim_start_matches('/')),
                contents: Vec::new(),
            }))
            .collect();
        self.client()
            .upload_to_container(
                name,
                Some(UploadToContainerOptions {
                    path: "/",
                    ..Default::default()
                }),
                tar_of(&inputs)?.into(),
            )
            .await
            .map_err(map_bollard_err)?;
        let started = Instant::now();
        self.client()
            .start_container(name, None::<StartContainerOptions<String>>)
            .await
            .map_err(map_bollard_err)?;

        let disk_limit = job.limits.disk_mib * 1024 * 1024;
        let mut stopped = None;
        let mut polls = 0u32;
        let mut wait = Box::pin(
            self.client()
                .wait_container(name, None::<WaitContainerOptions<String>>),
        );
        let exit_code = loop {
            match tokio::time::timeout(POLL, wait.next()).await {
                Ok(Some(Ok(done))) => break Some(done.status_code as i32),
                // A non-zero exit arrives as an error carrying the code.
                Ok(Some(Err(bollard::errors::Error::DockerContainerWaitError {
                    code, ..
                }))) => {
                    break Some(code as i32);
                }
                Ok(Some(Err(e))) => return Err(map_bollard_err(e)),
                Ok(None) => break None,
                Err(_) => {}
            }
            if started.elapsed() > job.limits.wall {
                stopped = Some(BatchStop::TimeLimit);
            } else if polls.is_multiple_of(DISK_EVERY)
                && self
                    .writable_bytes(name)
                    .await
                    .is_some_and(|b| b > disk_limit)
            {
                stopped = Some(BatchStop::DiskLimit);
            }
            if stopped.is_some() {
                if let Some(cmd) = &job.before_stop {
                    self.exec(name, cmd).await;
                }
                let _ = self
                    .client()
                    .kill_container(name, Some(KillContainerOptions { signal: "KILL" }))
                    .await;
                break None;
            }
            polls += 1;
        };
        let wall = started.elapsed();
        let outputs = if job.collect_outputs {
            self.download_outputs(name).await.unwrap_or_else(|e| {
                warn!(job = %job.name, error = %e, "batch: could not copy the job's outputs");
                Vec::new()
            })
        } else {
            Vec::new()
        };
        let logs = self.logs(name).await;
        Ok(BatchOutcome {
            exit_code,
            stopped,
            logs,
            outputs,
            wall,
        })
    }

    async fn writable_bytes(&self, name: &str) -> Option<u64> {
        let info = self
            .client()
            .inspect_container(name, Some(InspectContainerOptions { size: true }))
            .await
            .ok()?;
        info.size_rw.and_then(|b| u64::try_from(b).ok())
    }

    /// Runs `cmd` in the container and waits for it, best effort.
    async fn exec(&self, name: &str, cmd: &str) {
        let run = async {
            let exec = self
                .client()
                .create_exec(
                    name,
                    CreateExecOptions {
                        attach_stdout: Some(true),
                        attach_stderr: Some(true),
                        cmd: Some(vec!["sh".to_owned(), "-c".to_owned(), cmd.to_owned()]),
                        ..Default::default()
                    },
                )
                .await
                .ok()?;
            if let Ok(StartExecResults::Attached { mut output, .. }) =
                self.client().start_exec(&exec.id, None).await
            {
                while output.next().await.is_some() {}
            }
            Some(())
        };
        let _ = tokio::time::timeout(BEFORE_STOP_TIMEOUT, run).await;
    }

    async fn download_outputs(&self, name: &str) -> Result<Vec<BatchFile>> {
        let mut stream = self.client().download_from_container(
            name,
            Some(DownloadFromContainerOptions {
                path: BATCH_OUTPUT_DIR,
            }),
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            bytes.extend_from_slice(&chunk.map_err(map_bollard_err)?);
            if bytes.len() > BATCH_OUTPUT_MAX_BYTES * 2 {
                return Err(RuntimeError::Internal(
                    "the job's outputs are larger than the limit".to_owned(),
                ));
            }
        }
        files_of(&bytes)
    }

    async fn logs(&self, name: &str) -> String {
        let mut stream = self.client().logs(
            name,
            Some(LogsOptions::<String> {
                stdout: true,
                stderr: true,
                ..Default::default()
            }),
        );
        let mut text = String::new();
        while let Some(Ok(chunk)) = stream.next().await {
            text.push_str(&chunk.to_string());
            if text.len() > BATCH_LOG_MAX_BYTES * 2 {
                text = tail(&text, BATCH_LOG_MAX_BYTES).to_owned();
            }
        }
        tail(&text, BATCH_LOG_MAX_BYTES).to_owned()
    }
}

struct Gateway {
    network: String,
    container: String,
}

#[async_trait]
impl BatchRunner for DockerBatchRunner {
    fn is_available(&self) -> bool {
        true
    }

    async fn build_image(&self, context_tar: &[u8], tag: &str) -> Result<String> {
        validate_build_inputs(context_tar, tag)?;
        if !self.image_exists(tag).await {
            ContainerRuntime::build(&self.runtime, context_tar, tag).await?;
        }
        Ok(tag.to_owned())
    }

    async fn run(&self, job: BatchJob) -> Result<BatchOutcome> {
        let base = container_name(&job.name);
        self.remove_container(&base).await;
        let gateway = match job.network {
            BatchNetwork::None => None,
            BatchNetwork::RouterOnly => Some(self.start_gateway(&job, &base).await?),
        };
        let mut job = job;
        let network = match &gateway {
            None => "none".to_owned(),
            Some(g) => {
                job.env.push((
                    BATCH_ROUTER_URL_ENV.to_owned(),
                    format!("http://{}:{GATEWAY_PORT}/v1", g.container),
                ));
                g.network.clone()
            }
        };
        let outcome = self.run_container(&job, &base, &network).await;
        self.remove_container(&base).await;
        if let Some(g) = &gateway {
            self.stop_gateway(g).await;
        }
        outcome
    }

    async fn cleanup(&self, label: Option<&str>) -> Result<usize> {
        let filter = match label {
            Some(l) => format!("{BATCH_LABEL}={l}"),
            None => BATCH_LABEL.to_owned(),
        };
        let filters = HashMap::from([("label".to_owned(), vec![filter])]);
        let containers = self
            .client()
            .list_containers(Some(ListContainersOptions {
                all: true,
                filters: filters.clone(),
                ..Default::default()
            }))
            .await
            .map_err(map_bollard_err)?;
        let mut removed = 0;
        for c in containers {
            if let Some(id) = c.id {
                self.remove_container(&id).await;
                removed += 1;
            }
        }
        let networks = self
            .client()
            .list_networks(Some(ListNetworksOptions { filters }))
            .await
            .map_err(map_bollard_err)?;
        for n in networks {
            if let Some(id) = n.id
                && self.client().remove_network(&id).await.is_ok()
            {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// A Docker-safe container name for a job (`[a-zA-Z0-9_.-]`, at most 60 characters, so the
/// `-net` / `-gw` suffixes still fit Docker's limits).
fn container_name(job: &str) -> String {
    let safe: String = job
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "_.-".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    let safe = safe.trim_start_matches(['-', '.', '_']);
    format!("nasiko-batch-{}", &safe[..safe.len().min(46)])
}

/// The gateway's nginx config: `/v1/*` to the router, everything else refused.
fn gateway_conf(upstream: &str) -> String {
    let upstream = upstream.trim_end_matches('/');
    format!(
        "worker_processes 1;\nerror_log /dev/stderr warn;\npid /tmp/nginx.pid;\n\
         events {{ worker_connections 256; }}\n\
         http {{\n  access_log off;\n  client_max_body_size 64m;\n\
           server {{\n    listen {GATEWAY_PORT};\n\
             location /v1/ {{\n      proxy_pass {upstream}/v1/;\n      proxy_http_version 1.1;\n\
               proxy_set_header Host $proxy_host;\n      proxy_buffering off;\n\
               proxy_read_timeout 900s;\n      proxy_send_timeout 900s;\n    }}\n\
             location / {{ return 403; }}\n  }}\n}}\n"
    )
}

/// A tar holding `files` at their paths (owner root, mode 0644; folders 0777 so a non-root
/// job can write its outputs).
fn tar_of(files: &[BatchFile]) -> Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut dirs: Vec<String> = Vec::new();
    for f in files {
        let mut parent = std::path::Path::new(&f.path).parent();
        let mut chain = Vec::new();
        while let Some(p) = parent.filter(|p| !p.as_os_str().is_empty()) {
            chain.push(p.to_string_lossy().into_owned());
            parent = p.parent();
        }
        for dir in chain.into_iter().rev() {
            if dirs.contains(&dir) {
                continue;
            }
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_mode(0o777);
            header.set_size(0);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("{dir}/"), std::io::empty())
                .map_err(io_err)?;
            dirs.push(dir);
        }
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(f.contents.len() as u64);
        header.set_cksum();
        builder
            .append_data(&mut header, &f.path, f.contents.as_slice())
            .map_err(io_err)?;
    }
    builder.into_inner().map_err(io_err)
}

/// The regular files in a tar Docker returned for [`BATCH_OUTPUT_DIR`] (its entries start with
/// the folder's own name, `out/…`), relative to that folder.
fn files_of(tar_bytes: &[u8]) -> Result<Vec<BatchFile>> {
    let mut archive = tar::Archive::new(tar_bytes);
    let mut files = Vec::new();
    let mut total = 0usize;
    for entry in archive.entries().map_err(io_err)? {
        let mut entry = entry.map_err(io_err)?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().map_err(io_err)?.to_string_lossy().into_owned();
        let Some((_, relative)) = path.split_once('/') else {
            continue;
        };
        if relative.is_empty() || relative == ".keep" {
            continue;
        }
        let mut contents = Vec::new();
        entry.read_to_end(&mut contents).map_err(io_err)?;
        total += contents.len();
        if total > BATCH_OUTPUT_MAX_BYTES {
            return Err(RuntimeError::Internal(
                "the job's outputs are larger than the limit".to_owned(),
            ));
        }
        files.push(BatchFile {
            path: relative.to_owned(),
            contents,
        });
    }
    Ok(files)
}

fn io_err(e: std::io::Error) -> RuntimeError {
    RuntimeError::Internal(format!("batch tar: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_docker_safe_and_short() {
        let name = container_name("eval/run 1:task:2");
        assert_eq!(name, "nasiko-batch-eval-run-1-task-2");
        assert!(container_name(&"x".repeat(200)).len() <= 60);
    }

    #[test]
    fn the_gateway_forwards_only_the_router() {
        let conf = gateway_conf("http://host.docker.internal:8082/");
        assert!(
            conf.contains(
                "location /v1/ {\n      proxy_pass http://host.docker.internal:8082/v1/;"
            )
        );
        assert!(conf.contains("location / { return 403; }"));
    }

    #[test]
    fn outputs_round_trip_through_a_tar() {
        let files = vec![
            BatchFile {
                path: "out/raw.patch".into(),
                contents: b"diff".to_vec(),
            },
            BatchFile {
                path: "out/.keep".into(),
                contents: Vec::new(),
            },
            BatchFile {
                path: "out/nested/log.txt".into(),
                contents: b"log".to_vec(),
            },
        ];
        let back = files_of(&tar_of(&files).unwrap()).unwrap();
        assert_eq!(
            back.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            vec!["raw.patch", "nested/log.txt"]
        );
        assert_eq!(back[0].contents, b"diff");
    }
}
