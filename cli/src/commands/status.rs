use anyhow::{Result, bail};
use nasiko_utils::term;
use serde_json::json;

use crate::api::Client;
use crate::config;

/// One reading of the control plane's `/readiness` and `/metrics` endpoints.
struct Health {
    postgres_ok: bool,
    redis_ok: bool,
    orchestrator_ok: bool,
    agents_running: i64,
    agents_total: i64,
    containers_total: i64,
    builds_pending: i64,
}

impl Health {
    fn fetch(client: &Client) -> Result<Self> {
        let readiness: serde_json::Value = client.get_public_json_any_status("/readiness")?;
        let ok = |key: &str| {
            readiness
                .get(key)
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        };
        let metrics: serde_json::Value = client.get_public_json("/metrics")?;
        let count = |key: &str| metrics.get(key).and_then(|v| v.as_i64()).unwrap_or(0);
        Ok(Self {
            postgres_ok: ok("postgres"),
            redis_ok: ok("redis"),
            orchestrator_ok: ok("orchestrator"),
            agents_running: count("agents_running"),
            agents_total: count("agents_total"),
            containers_total: count("containers_total"),
            builds_pending: count("builds_pending"),
        })
    }

    fn all_ok(&self) -> bool {
        self.postgres_ok && self.redis_ok && self.orchestrator_ok
    }
}

/// Show cluster health and metrics — as text, or as one JSON object with `json`. Fails (exit
/// code 1) when any health check fails, so `nasiko status && nasiko deploy .` stops on an
/// unhealthy cluster instead of carrying on.
pub fn status(json: bool) -> Result<()> {
    let (name, entry) = config::active_cluster()?;
    let registry_url = config::artifact_registry_url();
    // Named before the network calls, so a cluster that doesn't answer is still identified.
    if !json {
        print_header(&name, &entry.url, registry_url.as_deref());
    }
    let client = Client::from_active_cluster()?;
    let health = Health::fetch(&client)?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&status_json(
                &name,
                &entry.url,
                registry_url.as_deref(),
                &health
            ))?
        );
    } else {
        print_health(&health);
    }

    if !health.all_ok() {
        bail!("cluster '{name}' is unhealthy");
    }
    Ok(())
}

fn print_header(name: &str, url: &str, registry_url: Option<&str>) {
    println!("Cluster:  {name}");
    println!("CP URL:   {url}");
    println!(
        "Registry: {}",
        registry_url.unwrap_or("(not set — export NASIKO_REGISTRY_URL)")
    );
    println!();
}

fn print_health(health: &Health) {
    let check = |ok: bool| {
        if ok {
            term::green("ok")
        } else {
            term::red("FAIL")
        }
    };
    println!("Health:");
    println!("  Postgres:     {}", check(health.postgres_ok));
    println!("  Redis:        {}", check(health.redis_ok));
    println!("  Orchestrator: {}", check(health.orchestrator_ok));

    println!("\nMetrics:");
    println!(
        "  Agents:     {} running / {} total",
        health.agents_running, health.agents_total
    );
    println!("  Containers: {}", health.containers_total);
    println!("  Builds:     {} pending", health.builds_pending);
}

fn status_json(
    name: &str,
    url: &str,
    registry_url: Option<&str>,
    health: &Health,
) -> serde_json::Value {
    let check = |ok: bool| if ok { "ok" } else { "fail" };
    json!({
        "cluster": name,
        "url": url,
        "registry_url": registry_url,
        "healthy": health.all_ok(),
        "health": {
            "postgres": check(health.postgres_ok),
            "redis": check(health.redis_ok),
            "orchestrator": check(health.orchestrator_ok),
        },
        "metrics": {
            "agents_running": health.agents_running,
            "agents_total": health.agents_total,
            "containers_total": health.containers_total,
            "builds_pending": health.builds_pending,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn health(redis_ok: bool) -> Health {
        Health {
            postgres_ok: true,
            redis_ok,
            orchestrator_ok: true,
            agents_running: 2,
            agents_total: 3,
            containers_total: 2,
            builds_pending: 0,
        }
    }

    #[test]
    fn healthy_only_when_every_check_passes() {
        assert!(health(true).all_ok());
        assert!(!health(false).all_ok());
    }

    #[test]
    fn json_reports_each_check_and_the_metrics() {
        let out = status_json("local", "http://localhost:8080", None, &health(false));
        assert_eq!(out["cluster"], "local");
        assert_eq!(out["healthy"], false);
        assert_eq!(out["health"]["redis"], "fail");
        assert_eq!(out["health"]["postgres"], "ok");
        assert_eq!(out["metrics"]["agents_total"], 3);
        assert!(out["registry_url"].is_null());
    }
}
