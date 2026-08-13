//! OCI image push: `docker save` tar → parse → push blobs + manifest to registry.

use std::collections::HashMap;
use std::io::Read;
use std::process::Command;

use anyhow::{Context, Result, bail};
use nasiko_utils::version::parse_plain_version;
use sha2::{Digest, Sha256};

use crate::api::OciClient;
use crate::util::container_bin;

/// Push a local Docker image to the CP's OCI registry.
/// Returns the repo and tag used (e.g., "nasiko/my-agent", "v1").
pub fn push_image(image: &str, repo: &str, tag: &str) -> Result<()> {
    let oci = OciClient::for_cp()?;
    let tar_data = docker_save(image)?;
    let entries = parse_docker_tar(&tar_data)?;
    // `entries` holds its own copies of the config/layer bytes extracted from the tar —
    // the raw `docker save` output is no longer needed. Drop it now (rather than at the
    // end of the function) so we're not holding ~2x the image size in memory while
    // pushing blobs.
    // TODO(perf follow-up): stream `docker save` output directly into the tar parser and
    // straight through to the registry PUT instead of buffering the whole image twice.
    drop(tar_data);

    // Push layer blobs. Reuse the digests already computed in `parse_docker_tar` — the
    // manifest builder needs them too, so hashing again here would be wasted work.
    for (layer, digest) in entries.layers.iter().zip(&entries.layer_digests) {
        eprint!("  pushing layer {}... ", &digest[7..19]);
        oci.push_blob(repo, digest, layer)?;
        eprintln!("done ({} bytes)", layer.len());
    }

    // Push config blob
    let config_digest = sha256_digest(&entries.config);
    eprint!("  pushing config {}... ", &config_digest[7..19]);
    oci.push_blob(repo, &config_digest, &entries.config)?;
    eprintln!("done");

    // Build OCI manifest
    let manifest = build_oci_manifest(&entries, &config_digest);
    let manifest_bytes = serde_json::to_vec(&manifest)?;

    eprint!("  pushing manifest as {tag}... ");
    oci.push_manifest(
        repo,
        tag,
        "application/vnd.oci.image.manifest.v1+json",
        &manifest_bytes,
    )?;
    eprintln!("done");

    Ok(())
}

/// Pull a template source archive from the artifact registry.
///
/// Bare template names resolve to the `nasiko`-owned artifact of that name —
/// exactly what `nasiko-ee registry publish` creates (`nasiko/<name>`, tagged
/// with its semver version).
/// Returns Err if registry is not configured or unreachable.
pub fn pull_template(template_name: &str) -> Result<Vec<u8>> {
    pull_artifact_tarball(&format!("nasiko/{template_name}"), None)
}

/// Pull a source-archive artifact (`owner/name`) from the artifact registry and
/// return its tar.gz bytes.
///
/// `version` is the exact tag to pull — pass the version the catalog reported
/// for the artifact. With `None` the newest **semver** tag is resolved from
/// `tags/list`.
pub fn pull_artifact_tarball(repo: &str, version: Option<&str>) -> Result<Vec<u8>> {
    let oci = OciClient::for_artifact_registry()?
        .ok_or_else(|| anyhow::anyhow!("no artifact registry configured"))?;

    let tag = match version {
        Some(v) => v.to_string(),
        None => {
            let tags_json = oci
                .list_tags(repo)
                .with_context(|| format!("artifact '{repo}' not found in registry"))?;
            newest_source_tag(&serde_json::from_str(&tags_json)?, repo)?
        }
    };

    let manifest_json = oci
        .pull_manifest(repo, &tag)
        .with_context(|| format!("failed to pull manifest for '{repo}:{tag}'"))?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_json)?;
    let layer_digest = source_layer_digest(&manifest, repo, &tag)?;

    oci.pull_blob(repo, &layer_digest)
}

/// Pick the newest published version from an OCI `tags/list` body.
///
/// A repository can hold more than one kind of manifest under different tags —
/// a source archive at `0.1.0` alongside a runnable container image at
/// `latest`, say — and `tags/list` is lexically sorted, so "the last tag" is
/// whatever sorts highest, not the newest version. Only plain `x.y.z` tags are
/// candidates (`parse_plain_version` rejects `latest` and pre-release suffixes),
/// and the winner is the highest by semver order.
fn newest_source_tag(tags_body: &serde_json::Value, repo: &str) -> Result<String> {
    let tags: Vec<&str> = tags_body
        .get("tags")
        .and_then(|t| t.as_array())
        .map(|arr| arr.iter().filter_map(|t| t.as_str()).collect())
        .unwrap_or_default();

    if tags.is_empty() {
        bail!("no tags found for artifact '{repo}'");
    }

    let mut versions: Vec<_> = tags
        .iter()
        .filter_map(|t| parse_plain_version(t).map(|v| (v, *t)))
        .collect();
    versions.sort_by(|a, b| a.0.cmp(&b.0));

    match versions.pop() {
        Some((_, tag)) => Ok(tag.to_string()),
        None => bail!(
            "artifact '{repo}' has no versioned tags (found: {}) — \
             a source artifact is published with an x.y.z version",
            tags.join(", ")
        ),
    }
}

/// Find the source-archive layer in a manifest.
///
/// Nasiko source artifacts carry a single `application/vnd.nasiko.<kind>.v1.tar+gzip`
/// layer. A container image manifest also has `layers[0]`, so taking the first
/// layer unconditionally happily unpacks a base image's filesystem over the
/// user's project — match on the media type and refuse anything else.
fn source_layer_digest(manifest: &serde_json::Value, repo: &str, tag: &str) -> Result<String> {
    let layers = manifest
        .get("layers")
        .and_then(|l| l.as_array())
        .context("invalid manifest: no layers")?;

    let media_type = |layer: &serde_json::Value| -> Option<String> {
        layer
            .get("mediaType")
            .and_then(|m| m.as_str())
            .map(String::from)
    };

    if let Some(layer) = layers
        .iter()
        .find(|l| media_type(l).is_some_and(|mt| is_source_media_type(&mt)))
    {
        return layer
            .get("digest")
            .and_then(|d| d.as_str())
            .map(String::from)
            .context("invalid manifest: source layer has no digest");
    }

    let found = layers
        .iter()
        .filter_map(media_type)
        .collect::<Vec<_>>()
        .join(", ");
    bail!(
        "'{repo}:{tag}' is not a source artifact — its layers are [{found}]. \
         Scaffolding needs the published source archive, not a container image."
    )
}

fn is_source_media_type(media_type: &str) -> bool {
    media_type.starts_with("application/vnd.nasiko.") && media_type.ends_with(".tar+gzip")
}

// ─── Docker image → OCI conversion ──────────────────────────────────────────
//
// `docker_save`/`parse_docker_tar`/`build_oci_manifest`/`sha256_digest` are
// pure conversion logic with no dependency on where the result gets pushed —
// `ee/cli` reuses them (via `nasiko::oci::...`, since it already depends on
// this crate for `dispatch_agent_dev`/`dispatch_agent_ops`/`dispatch_registry`)
// to publish images to the artifact registry, while `push_image` above keeps
// pushing to this cluster's own OCI registry. Same conversion, different
// destination — don't fork this logic per destination.

pub struct DockerTarEntries {
    pub config: Vec<u8>,
    pub layers: Vec<Vec<u8>>,
    pub layer_digests: Vec<String>,
}

pub fn docker_save(image: &str) -> Result<Vec<u8>> {
    let bin = container_bin();
    let output = Command::new(&bin)
        .args(["save", image])
        .output()
        .with_context(|| {
            format!("failed to run `{bin} save` — is the container runtime running?")
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{bin} save failed: {stderr}");
    }
    Ok(output.stdout)
}

pub fn parse_docker_tar(tar_data: &[u8]) -> Result<DockerTarEntries> {
    let mut archive = tar::Archive::new(tar_data);
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().to_string();
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        files.insert(path, buf);
    }

    // Parse Docker manifest.json (array of image manifests)
    let docker_manifest_bytes = files
        .get("manifest.json")
        .context("no manifest.json in docker save output")?;
    let docker_manifests: Vec<DockerManifest> = serde_json::from_slice(docker_manifest_bytes)?;
    let dm = docker_manifests.first().context("empty manifest.json")?;

    // Config
    let config = files
        .remove(&dm.config)
        .with_context(|| format!("config blob {} not found in tar", dm.config))?;

    // Layers (in order)
    let mut layers = Vec::new();
    let mut layer_digests = Vec::new();
    for layer_path in &dm.layers {
        let data = files
            .remove(layer_path)
            .with_context(|| format!("layer {} not found in tar", layer_path))?;
        let digest = sha256_digest(&data);
        layer_digests.push(digest);
        layers.push(data);
    }

    Ok(DockerTarEntries {
        config,
        layers,
        layer_digests,
    })
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerManifest {
    config: String,
    layers: Vec<String>,
}

pub fn sha256_digest(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

pub fn build_oci_manifest(entries: &DockerTarEntries, config_digest: &str) -> serde_json::Value {
    let layers: Vec<serde_json::Value> = entries
        .layers
        .iter()
        .zip(&entries.layer_digests)
        .map(|(data, digest)| {
            serde_json::json!({
                "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
                "digest": digest,
                "size": data.len(),
            })
        })
        .collect();

    serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "digest": config_digest,
            "size": entries.config.len(),
        },
        "layers": layers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(list: &[&str]) -> serde_json::Value {
        serde_json::json!({ "name": "nasiko/books", "tags": list })
    }

    fn source_manifest() -> serde_json::Value {
        serde_json::json!({
            "artifactType": "application/vnd.nasiko.agent.v1",
            "layers": [{
                "mediaType": "application/vnd.nasiko.agent.v1.tar+gzip",
                "digest": "sha256:source",
            }],
        })
    }

    fn image_manifest() -> serde_json::Value {
        serde_json::json!({
            "config": { "mediaType": "application/vnd.oci.image.config.v1+json" },
            "layers": [
                { "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip", "digest": "sha256:certs" },
                { "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip", "digest": "sha256:app" },
            ],
        })
    }

    #[test]
    fn newest_tag_ignores_latest() {
        // `latest` sorts after `0.1.0` lexically, which is how a container
        // image tag used to win over the source archive.
        let tag = newest_source_tag(&tags(&["0.1.0", "latest"]), "nasiko/books").unwrap();
        assert_eq!(tag, "0.1.0");
    }

    #[test]
    fn newest_tag_orders_by_semver_not_lexically() {
        let tag = newest_source_tag(&tags(&["0.1.0", "0.10.0", "0.9.0"]), "nasiko/books").unwrap();
        assert_eq!(tag, "0.10.0");
    }

    #[test]
    fn newest_tag_errors_when_only_unversioned_tags_exist() {
        let err = newest_source_tag(&tags(&["latest", "dev"]), "nasiko/books").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no versioned tags"), "{msg}");
        assert!(msg.contains("latest, dev"), "{msg}");
    }

    #[test]
    fn newest_tag_errors_on_empty_tag_list() {
        let err = newest_source_tag(&tags(&[]), "nasiko/books").unwrap_err();
        assert!(err.to_string().contains("no tags found"), "{err}");
    }

    #[test]
    fn source_layer_matches_nasiko_media_type() {
        let digest = source_layer_digest(&source_manifest(), "nasiko/books", "0.1.0").unwrap();
        assert_eq!(digest, "sha256:source");
    }

    #[test]
    fn source_layer_rejects_container_image() {
        let err = source_layer_digest(&image_manifest(), "nasiko/books", "latest").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not a source artifact"), "{msg}");
        // Never silently unpacks the base image's filesystem.
        assert!(!msg.contains("sha256:certs"), "{msg}");
    }

    #[test]
    fn source_layer_found_behind_other_layers() {
        let manifest = serde_json::json!({
            "layers": [
                { "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip", "digest": "sha256:other" },
                { "mediaType": "application/vnd.nasiko.skill.v1.tar+gzip", "digest": "sha256:skill" },
            ],
        });
        let digest = source_layer_digest(&manifest, "nasiko/tracking-id", "1.0.0").unwrap();
        assert_eq!(digest, "sha256:skill");
    }
}
