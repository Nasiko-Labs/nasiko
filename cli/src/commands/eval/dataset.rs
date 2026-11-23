//! `nasiko eval freeze` / `export`: a fixed, versioned set of verified tasks, written as a
//! SWE-bench `.jsonl` file that never changes once frozen.

use std::fs;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use nasiko_evals::{TaskInstance, TaskStatus};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::store::Store;

/// `manifest.json` next to `dataset.jsonl`.
#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub version: u32,
    pub created_at: DateTime<Utc>,
    pub instance_ids: Vec<String>,
    /// sha256 of `dataset.jsonl`.
    pub sha256: String,
}

/// `name@3` → (`name`, Some(3)); `name` → (`name`, None) meaning the latest.
pub fn parse_ref(reference: &str) -> Result<(String, Option<u32>)> {
    match reference.split_once('@') {
        Some((name, v)) => Ok((
            name.to_string(),
            Some(
                v.trim_start_matches('v')
                    .parse()
                    .context("version must be a number")?,
            ),
        )),
        None => Ok((reference.to_string(), None)),
    }
}

fn versions(store: &Store, name: &str) -> Vec<u32> {
    let dir = store.layout().root().join("datasets").join(name);
    let mut out: Vec<u32> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_prefix('v')?.parse().ok())
        .collect();
    out.sort();
    out
}

/// Freezes every verified task into the next version of `name`.
pub fn freeze(store: &Store, name: &str) -> Result<Manifest> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        bail!("a dataset name is letters, digits, '-', '_' or '.'");
    }
    let tasks: Vec<TaskInstance> = store
        .tasks()?
        .into_iter()
        .filter(|t| t.x_nasiko.status == TaskStatus::Verified)
        .collect();
    if tasks.is_empty() {
        bail!("no verified tasks yet: run `nasiko eval build`, then `screen`");
    }
    let version = versions(store, name).last().map_or(1, |v| v + 1);
    let mut jsonl = String::new();
    for t in &tasks {
        jsonl.push_str(&t.to_swebench_line()?);
        jsonl.push('\n');
    }
    let dir = store.layout().dataset_dir(name, version);
    let manifest = Manifest {
        name: name.to_string(),
        version,
        created_at: Utc::now(),
        instance_ids: tasks.iter().map(|t| t.instance_id.clone()).collect(),
        sha256: Sha256::digest(jsonl.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    };
    store.save_text(&dir.join("dataset.jsonl"), &jsonl)?;
    store.save_json(&dir.join("manifest.json"), &manifest)?;
    Ok(manifest)
}

/// The tasks of a frozen dataset (latest version when none is given), checked against the
/// manifest's hash so an edited file is refused.
pub fn load(store: &Store, reference: &str) -> Result<(Manifest, Vec<TaskInstance>)> {
    let (name, version) = parse_ref(reference)?;
    let version = match version {
        Some(v) => v,
        None => *versions(store, &name)
            .last()
            .with_context(|| format!("no dataset named {name:?}; run `nasiko eval freeze`"))?,
    };
    let dir = store.layout().dataset_dir(&name, version);
    let jsonl = fs::read_to_string(dir.join("dataset.jsonl"))
        .with_context(|| format!("no dataset {name}@{version}"))?;
    let manifest: Manifest = serde_json::from_str(&fs::read_to_string(dir.join("manifest.json"))?)?;
    let hash: String = Sha256::digest(jsonl.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if hash != manifest.sha256 {
        bail!("{name}@{version} was changed after it was frozen (hash mismatch)");
    }
    let tasks = jsonl
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<std::result::Result<Vec<TaskInstance>, _>>()?;
    Ok((manifest, tasks))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nasiko_evals::EvalLayout;

    #[test]
    fn freezing_makes_new_versions_and_loading_checks_the_hash() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Store::new(EvalLayout::new(dir.path()));
        assert!(freeze(&store, "ds").is_err(), "nothing verified yet");
        let mut t: TaskInstance =
            serde_json::from_str(include_str!("testdata/task.json")).expect("task");
        t.x_nasiko.status = TaskStatus::Verified;
        store.save_task(&t).expect("saves");
        assert_eq!(freeze(&store, "ds").expect("v1").version, 1);
        assert_eq!(freeze(&store, "ds").expect("v2").version, 2);
        let (m, tasks) = load(&store, "ds").expect("latest");
        assert_eq!((m.version, tasks.len()), (2, 1));
        let file = store.layout().dataset_dir("ds", 1).join("dataset.jsonl");
        fs::write(&file, "{}\n").expect("tamper");
        assert!(load(&store, "ds@1").is_err());
        assert_eq!(parse_ref("ds@v3").expect("ref"), ("ds".into(), Some(3)));
    }
}
