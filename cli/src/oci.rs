//! OCI image push: `docker save` tar → parse → push blobs + manifest to registry.

use std::collections::HashMap;
use std::io::Read;
use std::process::Command;

use anyhow::{Context, Result, bail};
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
/// Templates are stored as single-layer OCI artifacts.
/// Returns Err if registry is not configured or unreachable.
pub fn pull_template(template_name: &str) -> Result<Vec<u8>> {
    let oci = OciClient::for_artifact_registry()?
        .ok_or_else(|| anyhow::anyhow!("no artifact registry configured"))?;

    let repo = format!("nasiko/templates/{template_name}");

    let manifest_json = oci.pull_manifest(&repo, "latest")?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_json)?;

    let layers = manifest
        .get("layers")
        .and_then(|l| l.as_array())
        .context("invalid manifest: no layers")?;

    let layer_digest = layers
        .first()
        .and_then(|l| l.get("digest"))
        .and_then(|d| d.as_str())
        .context("invalid manifest: no layer digest")?;

    oci.pull_blob(&repo, layer_digest)
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
