//! Export a stored image as a `docker load`–compatible archive.
//!
//! Single-node deployments run the Docker daemon next to this embedded
//! registry, so instead of making the daemon pull over the network (which
//! needs a reachable `OCI_REGISTRY_HOST` plus insecure-registry setup on dev
//! machines), the server hands it the bytes it already stores. This is the
//! [`nasiko_runtime::ImageSource`] implementation wired into
//! `DockerRuntime` at the composition root.

use std::io::Read;

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use crate::OciState;
use crate::error::{OciError, Result};
use crate::ops::{blobs, manifests};

#[async_trait]
impl nasiko_runtime::ImageSource for OciState {
    async fn docker_archive(&self, image: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let Some((repository, tag)) = split_image_ref(image) else {
            return Ok(None);
        };
        let manifest = match manifests::get_manifest(self, &repository, &tag).await {
            Ok(m) => m,
            Err(OciError::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let image_manifest: serde_json::Value = serde_json::from_str(&manifest.content)
            .map_err(|e| OciError::Storage(format!("manifest {repository}:{tag}: {e}")))?;

        let config_digest = digest_at(&image_manifest, "/config/digest", &repository, &tag)?;
        let config = blobs::get_blob_bytes(self, &repository, &config_digest).await?;

        let layer_entries = image_manifest
            .pointer("/layers")
            .and_then(|l| l.as_array())
            .ok_or_else(|| {
                OciError::Storage(format!("manifest {repository}:{tag} has no layers"))
            })?;
        let mut layers = Vec::with_capacity(layer_entries.len());
        for (i, layer) in layer_entries.iter().enumerate() {
            let digest = layer
                .pointer("/digest")
                .and_then(|d| d.as_str())
                .ok_or_else(|| {
                    OciError::Storage(format!(
                        "manifest {repository}:{tag}: layer {i} has no digest"
                    ))
                })?;
            layers.push(
                blobs::get_blob_bytes(self, &repository, digest)
                    .await?
                    .to_vec(),
            );
        }

        Ok(Some(build_docker_archive(image, &config, layers)?))
    }
}

fn digest_at(
    manifest: &serde_json::Value,
    pointer: &str,
    repository: &str,
    tag: &str,
) -> Result<String> {
    manifest
        .pointer(pointer)
        .and_then(|d| d.as_str())
        .map(String::from)
        .ok_or_else(|| OciError::Storage(format!("manifest {repository}:{tag} missing {pointer}")))
}

/// Splits an image reference into the repository this registry stores it
/// under and its tag. A leading registry host (first segment containing `.`
/// or `:`, or `localhost`) is stripped — the daemon may hand us a qualified
/// ref while manifests are keyed by bare repository. Digest references
/// return `None`: `docker load` needs a tag to name what it imports.
fn split_image_ref(image: &str) -> Option<(String, String)> {
    if image.contains('@') {
        return None;
    }
    let unqualified = match image.split_once('/') {
        Some((first, rest))
            if first.contains('.') || first.contains(':') || first == "localhost" =>
        {
            rest
        }
        _ => image,
    };
    let (repository, tag) = unqualified.rsplit_once(':')?;
    if repository.is_empty() || tag.is_empty() {
        return None;
    }
    Some((repository.to_owned(), tag.to_owned()))
}

/// Assembles a `docker load`–compatible tar: the image config, one tar file
/// per layer, and a `manifest.json` tagging the result as `repo_tag`.
///
/// Layer blobs are stored however the pusher sent them — gzip-compressed by
/// BuildKit, plain tar from `docker save` (whatever the declared media type
/// says) — while the config's `rootfs.diff_ids` always name the uncompressed
/// bytes. Sniffing the gzip magic and decompressing keeps the archive
/// consistent with the config for both.
fn build_docker_archive(repo_tag: &str, config: &[u8], layers: Vec<Vec<u8>>) -> Result<Vec<u8>> {
    let config_name = format!("{}.json", sha256_hex(config));

    let mut layer_names = Vec::with_capacity(layers.len());
    let mut layer_tars = Vec::with_capacity(layers.len());
    for (i, layer) in layers.into_iter().enumerate() {
        layer_names.push(format!("layers/{i}.tar"));
        layer_tars.push(gunzip_if_gzipped(layer)?);
    }

    let manifest = serde_json::json!([{
        "Config": config_name,
        "RepoTags": [repo_tag],
        "Layers": layer_names,
    }]);
    let manifest_bytes = serde_json::to_vec(&manifest)
        .map_err(|e| OciError::Storage(format!("docker archive manifest: {e}")))?;

    let mut archive = tar::Builder::new(Vec::new());
    append_file(&mut archive, &config_name, config)?;
    for (name, layer) in layer_names.iter().zip(&layer_tars) {
        append_file(&mut archive, name, layer)?;
    }
    append_file(&mut archive, "manifest.json", &manifest_bytes)?;
    archive
        .into_inner()
        .map_err(|e| OciError::Storage(format!("docker archive: {e}")))
}

fn append_file(archive: &mut tar::Builder<Vec<u8>>, path: &str, data: &[u8]) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    archive
        .append_data(&mut header, path, data)
        .map_err(|e| OciError::Storage(format!("docker archive entry {path}: {e}")))
}

fn gunzip_if_gzipped(data: Vec<u8>) -> Result<Vec<u8>> {
    const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
    if !data.starts_with(&GZIP_MAGIC) {
        return Ok(data);
    }
    let mut decompressed = Vec::new();
    flate2::read::GzDecoder::new(data.as_slice())
        .read_to_end(&mut decompressed)
        .map_err(|e| OciError::Storage(format!("layer gunzip: {e}")))?;
    Ok(decompressed)
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn split_strips_registry_host_and_keeps_tag() {
        assert_eq!(
            split_image_ref("nasiko/my-agent:1.0.0"),
            Some(("nasiko/my-agent".into(), "1.0.0".into()))
        );
        assert_eq!(
            split_image_ref("registry.example.com/nasiko/my-agent:1.0.0"),
            Some(("nasiko/my-agent".into(), "1.0.0".into()))
        );
        assert_eq!(
            split_image_ref("localhost:8080/nasiko/my-agent:1.0.0"),
            Some(("nasiko/my-agent".into(), "1.0.0".into()))
        );
    }

    #[test]
    fn split_rejects_untagged_and_digest_refs() {
        assert_eq!(split_image_ref("nasiko/my-agent"), None);
        assert_eq!(
            split_image_ref(&format!("nasiko/my-agent@sha256:{}", "a".repeat(64))),
            None
        );
    }

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn archive_round_trips_with_mixed_layer_compression() {
        let config = br#"{"rootfs":{"diff_ids":[]}}"#;
        let plain_layer = b"plain layer tar bytes".to_vec();
        let gzipped_layer = gzip(b"gzipped layer tar bytes");

        let archive =
            build_docker_archive("nasiko/x:1.0.0", config, vec![plain_layer, gzipped_layer])
                .unwrap();

        let mut entries = std::collections::HashMap::new();
        let mut reader = tar::Archive::new(archive.as_slice());
        for entry in reader.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().to_string_lossy().into_owned();
            let mut content = Vec::new();
            entry.read_to_end(&mut content).unwrap();
            entries.insert(path, content);
        }

        let manifest: serde_json::Value =
            serde_json::from_slice(&entries["manifest.json"]).unwrap();
        assert_eq!(manifest[0]["RepoTags"][0], "nasiko/x:1.0.0");
        let config_name = manifest[0]["Config"].as_str().unwrap();
        assert_eq!(entries[config_name], config.to_vec());
        assert_eq!(entries["layers/0.tar"], b"plain layer tar bytes".to_vec());
        // The gzipped layer arrives decompressed, matching config diff_ids.
        assert_eq!(entries["layers/1.tar"], b"gzipped layer tar bytes".to_vec());
    }
}
