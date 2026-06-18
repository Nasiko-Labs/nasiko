use anyhow::Result;

use crate::api::Client;
use crate::config;

/// Show cluster health and metrics.
pub fn status() -> Result<()> {
    let (name, entry) = config::active_cluster()?;
    let registry_url = config::artifact_registry_url();
    println!("Cluster:  {}", name);
    println!("CP URL:   {}", entry.url);
    println!("Registry: {}", registry_url.as_deref().unwrap_or("(not set — export NASIKO_REGISTRY_URL)"));
    println!();

    let client = Client::from_active_cluster()?;

    let readiness: serde_json::Value = client.get_public_json("/readiness")?;
    let check = |key: &str| -> &str {
        if readiness.get(key).and_then(|v| v.as_bool()).unwrap_or(false) {
            "ok"
        } else {
            "FAIL"
        }
    };
    println!("Health:");
    println!("  Postgres:     {}", check("postgres"));
    println!("  Redis:        {}", check("redis"));
    println!("  Orchestrator: {}", check("orchestrator"));

    let metrics: serde_json::Value = client.get_public_json("/metrics")?;
    println!("\nMetrics:");
    let i = |key: &str| metrics.get(key).and_then(|v| v.as_i64()).unwrap_or(0);
    println!(
        "  Agents:     {} running / {} total",
        i("agents_running"),
        i("agents_total")
    );
    println!("  Containers: {}", i("containers_total"));
    println!("  Builds:     {} pending", i("builds_pending"));
    Ok(())
}
