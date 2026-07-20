use std::fs;
use std::io::Cursor;
use std::path::Path;

use anyhow::Result;
use include_dir::Dir;

/// Resolves the container CLI binary to shell out to. Honors `NASIKO_CONTAINER_CLI`
/// if set, otherwise prefers `docker` and falls back to `podman` when `docker` isn't
/// on PATH (e.g. podman-only dev setups without the podman-docker compat shim).
pub fn container_bin() -> String {
    if let Ok(bin) = std::env::var("NASIKO_CONTAINER_CLI") {
        return bin;
    }
    if on_path("docker") {
        "docker".to_string()
    } else {
        "podman".to_string()
    }
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file()))
        .unwrap_or(false)
}

pub fn extract_tar_gz(data: &[u8], dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    let cursor = Cursor::new(data);
    let gz = flate2::read::GzDecoder::new(cursor);
    let mut archive = tar::Archive::new(gz);
    archive.unpack(dest)?;
    Ok(())
}

pub fn extract_embedded_dir(dir: &Dir, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for file in dir.files() {
        let file_name = file.path().file_name().unwrap_or_default();
        fs::write(dest.join(file_name), file.contents())?;
    }
    for sub in dir.dirs() {
        let sub_name = sub.path().file_name().unwrap_or_default();
        extract_embedded_dir(sub, &dest.join(sub_name))?;
    }
    Ok(())
}

pub fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(first) => first.to_uppercase().to_string() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
