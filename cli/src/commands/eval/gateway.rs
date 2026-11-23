//! The eval gateway: an nginx container that is the replayed agents' only way out.
//!
//! Agent containers sit on an `--internal` Docker network with no route to the internet; the
//! gateway is on that network and on the normal bridge. It forwards `/v1/*` to the Nasiko
//! router only, replacing `Authorization` with the eval identity's router token and setting
//! `x-nasiko-eval-attempt` from a table of per-attempt secrets. So an agent can neither read
//! the token nor tag its calls as another attempt. Router tokens last 10 minutes; the run
//! rewrites the config with a fresh one and reloads nginx well before that.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::containers::{Containers, Mount, ServiceSpec};

/// Pinned nginx image.
pub const NGINX_IMAGE: &str = "nginx:1.27-alpine";
/// Port the gateway listens on inside the network.
pub const GATEWAY_PORT: u16 = 8080;
/// Header the agent sends its per-attempt secret in (set in its `opencode.json`).
pub const SECRET_HEADER: &str = "x-eval-secret";

pub struct GatewayPlan {
    /// The router's base, as seen from inside Docker (e.g. `http://host.docker.internal:8080`).
    pub upstream: String,
    /// (secret, attempt tag) per attempt.
    pub attempts: Vec<(String, String)>,
    pub container: String,
    pub networks: Vec<String>,
    pub config_dir: PathBuf,
    pub label: String,
}

/// The nginx config: pure text, unit-tested.
pub fn nginx_conf(plan: &GatewayPlan, token: &str) -> String {
    let map: String = plan
        .attempts
        .iter()
        .map(|(secret, tag)| format!("        \"{secret}\" \"{tag}\";\n"))
        .collect();
    let upstream = plan.upstream.trim_end_matches('/');
    let host = upstream
        .split("://")
        .nth(1)
        .unwrap_or(upstream)
        .split('/')
        .next()
        .unwrap_or_default();
    format!(
        "worker_processes 1;\n\
         events {{ worker_connections 1024; }}\n\
         http {{\n\
         \x20   map $http_x_eval_secret $eval_attempt {{\n\
         \x20       default \"\";\n\
         {map}\
         \x20   }}\n\
         \x20   server {{\n\
         \x20       listen {GATEWAY_PORT};\n\
         \x20       client_max_body_size 64m;\n\
         \x20       location /v1/ {{\n\
         \x20           if ($eval_attempt = \"\") {{ return 403; }}\n\
         \x20           proxy_set_header Authorization \"Bearer {token}\";\n\
         \x20           proxy_set_header x-nasiko-eval-attempt $eval_attempt;\n\
         \x20           proxy_set_header {SECRET_HEADER} \"\";\n\
         \x20           proxy_set_header Host {host};\n\
         \x20           proxy_http_version 1.1;\n\
         \x20           proxy_buffering off;\n\
         \x20           proxy_read_timeout 900s;\n\
         \x20           proxy_send_timeout 900s;\n\
         \x20           proxy_pass {upstream}/v1/;\n\
         \x20       }}\n\
         \x20       location / {{ return 404; }}\n\
         \x20   }}\n\
         }}\n"
    )
}

/// Turns `localhost` / `127.0.0.1` in the cluster URL into the host as seen from containers.
pub fn upstream_from_container(cluster_url: &str) -> String {
    cluster_url
        .replace("://localhost", "://host.docker.internal")
        .replace("://127.0.0.1", "://host.docker.internal")
}

/// A started gateway; stopped when dropped.
pub struct GatewayHandle<'a> {
    containers: &'a dyn Containers,
    plan: GatewayPlan,
}

pub fn start<'a>(
    containers: &'a dyn Containers,
    plan: GatewayPlan,
    token: &str,
) -> Result<GatewayHandle<'a>> {
    write_conf(&plan.config_dir, &nginx_conf(&plan, token))?;
    containers.start_service(&ServiceSpec {
        name: plan.container.clone(),
        image: NGINX_IMAGE.to_string(),
        cmd: vec![
            "nginx".into(),
            "-c".into(),
            "/etc/nginx/eval/nginx.conf".into(),
            "-g".into(),
            "daemon off;".into(),
        ],
        networks: plan.networks.clone(),
        mounts: vec![Mount {
            host: plan.config_dir.canonicalize()?,
            container: "/etc/nginx/eval".into(),
            read_only: true,
        }],
        label: plan.label.clone(),
    })?;
    Ok(GatewayHandle { containers, plan })
}

fn write_conf(dir: &Path, conf: &str) -> Result<()> {
    fs::create_dir_all(dir)?;
    // Written in place (not replaced) so the read-only bind mount keeps seeing the same file.
    fs::write(dir.join("nginx.conf"), conf)?;
    Ok(())
}

impl GatewayHandle<'_> {
    /// Swaps in a fresh router token.
    pub fn refresh(&self, token: &str) -> Result<()> {
        write_conf(&self.plan.config_dir, &nginx_conf(&self.plan, token))?;
        self.containers
            .exec(&self.plan.container, &["nginx", "-s", "reload"])
    }

    /// The URL agents use as their OpenAI base.
    pub fn base_url(&self) -> String {
        format!("http://{}:{GATEWAY_PORT}/v1", self.plan.container)
    }
}

impl Drop for GatewayHandle<'_> {
    fn drop(&mut self) {
        let _ = self.containers.stop_service(&self.plan.container);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_maps_secrets_to_tags_and_sets_the_token() {
        let plan = GatewayPlan {
            upstream: "http://host.docker.internal:8080".into(),
            attempts: vec![("s1".into(), "run:a".into()), ("s2".into(), "run:b".into())],
            container: "gw".into(),
            networks: vec![],
            config_dir: PathBuf::from("/tmp"),
            label: "l".into(),
        };
        let conf = nginx_conf(&plan, "tok");
        assert!(conf.contains("\"s1\" \"run:a\";"));
        assert!(conf.contains("if ($eval_attempt = \"\") { return 403; }"));
        assert!(conf.contains("proxy_set_header Authorization \"Bearer tok\";"));
        assert!(conf.contains("proxy_pass http://host.docker.internal:8080/v1/;"));
        assert!(conf.contains("proxy_set_header Host host.docker.internal:8080;"));
    }

    #[test]
    fn localhost_becomes_the_docker_host() {
        assert_eq!(
            upstream_from_container("http://localhost:8080"),
            "http://host.docker.internal:8080"
        );
        assert_eq!(
            upstream_from_container("https://cp.example.com"),
            "https://cp.example.com"
        );
    }
}
