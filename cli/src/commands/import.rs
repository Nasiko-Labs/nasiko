use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::api::Client;

/// Server response from `POST /api/import/registry`
/// (`ImportResult` in `oss/server/src/catalog/import.rs`).
#[derive(Deserialize)]
struct ImportResult {
    agent_id: String,
    build_id: Option<String>,
    container_name: Option<String>,
    status: String,
}

/// Split a reference into `(repo, tag)`. The tag separator is the `:` *after*
/// the last `/`, so a ported host (`localhost:5000/owner/name`) isn't mistaken
/// for a tag.
fn split_tag(reference: &str) -> (&str, Option<&str>) {
    let last_slash = reference.rfind('/').map_or(0, |i| i + 1);
    match reference[last_slash..].find(':') {
        Some(rel) => {
            let at = last_slash + rel;
            (&reference[..at], Some(&reference[at + 1..]))
        }
        None => (reference, None),
    }
}

/// Whether the first path component is a registry host rather than an owner —
/// the standard OCI heuristic (a host carries a dot, a port, or is `localhost`).
fn is_host(component: &str) -> bool {
    component.contains('.') || component.contains(':') || component == "localhost"
}

/// Expand a short `owner/name[:tag]` reference using the connected artifact
/// registry, mirroring how `nasiko new` resolves the same shape
/// (`commands::scaffold::pull_artifact`). A reference that already names a host
/// is returned unchanged.
///
/// The server matches the host against `REGISTRY_IMPORT_ALLOWED_HOSTS`, so the
/// host has to be present by the time we POST — resolving it here means callers
/// get the same terse form they use for `nasiko new`.
fn qualify(reference: &str) -> Result<String> {
    let (repo, _) = split_tag(reference);
    let first = repo.split('/').next().unwrap_or("");

    if is_host(first) {
        // Already host-qualified; still needs an owner/name after the host.
        if repo.split('/').count() < 2 {
            bail!(
                "'{reference}' names a registry host but no artifact — expected \
                 registry.host/owner/name[:tag]"
            );
        }
        return Ok(reference.to_string());
    }

    if !repo.contains('/') {
        bail!(
            "'{reference}' is not a registry reference — expected owner/name[:tag] \
             (resolved against the connected registry) or registry.host/owner/name[:tag]"
        );
    }

    let url = crate::config::artifact_registry_url().with_context(|| {
        format!(
            "'{reference}' has no registry host and no registry is connected — run \
             `nasiko registry connect <url>` or pass the full \
             registry.host/owner/name[:tag]"
        )
    })?;
    let host = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    Ok(format!("{host}/{reference}"))
}

/// Deploy an agent to the active cluster straight from an OCI/artifact-registry
/// reference — the registry counterpart of `push` (local image) and `upload`
/// (local source).
///
/// The server pulls the image itself, so nothing is fetched locally and no
/// Docker daemon is required on this machine.
pub fn from_registry(reference: &str) -> Result<()> {
    let reference = reference.trim();
    if reference.is_empty() {
        bail!("reference is required: owner/name[:tag] or registry.host/owner/name[:tag]");
    }
    let reference = qualify(reference)?;

    let client = Client::from_active_cluster()?;
    let result: ImportResult = client
        .post_json(
            "/import/registry",
            &serde_json::json!({ "reference": reference }),
        )
        .map_err(|e| {
            // The server's own 403/422 text names the allowlist, so surface it
            // verbatim and only add the fix when it's the disabled case.
            let msg = e.to_string();
            if msg.contains("registry import is disabled") {
                anyhow::anyhow!(
                    "{msg}\n\nSet REGISTRY_IMPORT_ALLOWED_HOSTS on the control plane \
                     (comma-separated hosts) and restart it."
                )
            } else {
                e
            }
        })?;

    println!("Imported {reference}");
    println!("  agent_id:  {}", result.agent_id);
    println!("  status:    {}", result.status);
    if let Some(name) = &result.container_name {
        println!("  container: {name}");
    }
    if let Some(build) = &result.build_id {
        // A build id means the layer was source, not a runnable image, so the
        // server queued a build instead of deploying directly.
        println!("  build_id:  {build}");
        println!(
            "\nBuild queued — follow it with `nasiko logs {}`.",
            result.agent_id
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{is_host, qualify, split_tag};

    #[test]
    fn splits_tag_after_the_last_slash() {
        assert_eq!(
            split_tag("registry.nasiko.com/nasiko/a:1.0.1"),
            ("registry.nasiko.com/nasiko/a", Some("1.0.1"))
        );
        assert_eq!(
            split_tag("registry.nasiko.com/nasiko/a"),
            ("registry.nasiko.com/nasiko/a", None)
        );
    }

    #[test]
    fn a_ported_host_is_not_read_as_a_tag() {
        // The bug this guards: naive rsplit_once(':') turns
        // "localhost:5000/nasiko/a" into repo "localhost" + tag "5000/nasiko/a".
        assert_eq!(
            split_tag("localhost:5000/nasiko/a"),
            ("localhost:5000/nasiko/a", None)
        );
        assert_eq!(
            split_tag("localhost:5000/nasiko/a:2.0.0"),
            ("localhost:5000/nasiko/a", Some("2.0.0"))
        );
    }

    #[test]
    fn host_detection_matches_the_oci_heuristic() {
        assert!(is_host("registry.nasiko.com"));
        assert!(is_host("localhost"));
        assert!(is_host("localhost:5000"));
        assert!(!is_host("nasiko"));
    }

    #[test]
    fn host_qualified_references_pass_through_untouched() {
        assert_eq!(
            qualify("registry.nasiko.com/nasiko/infra-agent:1.0.1").unwrap(),
            "registry.nasiko.com/nasiko/infra-agent:1.0.1"
        );
    }

    #[test]
    fn a_host_with_no_artifact_is_rejected() {
        let err = qualify("registry.nasiko.com").unwrap_err().to_string();
        assert!(err.contains("no artifact"), "{err}");
    }

    #[test]
    fn a_bare_name_is_rejected() {
        // Not a reference at all — no owner, no host.
        let err = qualify("infra-agent:1.0.1").unwrap_err().to_string();
        assert!(err.contains("not a registry reference"), "{err}");
    }
}
