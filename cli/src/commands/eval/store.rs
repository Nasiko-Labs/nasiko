//! Reading and writing eval files under the eval root (`~/.nasiko/eval/` by default).
//!
//! Every JSON write goes through `coding_agent_router::atomic_write_json`, so a crash never
//! leaves a half-written file.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use nasiko_evals::{EvalLayout, TaskInstance};
use serde::{Deserialize, Serialize};

use crate::commands::coding_agent_router::{atomic_write, atomic_write_json, read_json};

/// Environment variable that moves the eval root.
pub const EVAL_HOME_ENV: &str = "NASIKO_EVAL_HOME";

/// The eval root: `$NASIKO_EVAL_HOME`, else `~/.nasiko/eval`.
pub fn eval_root() -> PathBuf {
    std::env::var_os(EVAL_HOME_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".nasiko")
                .join("eval")
        })
}

/// `config.json`: which models to compare and how to run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EvalConfig {
    /// Short name → router provider label + model id, e.g. `glm-5.3` →
    /// `{provider: "nebius-token-factory", model: "zai-org/GLM-5.3"}`.
    #[serde(default)]
    pub models: BTreeMap<String, ModelEntry>,
    /// Models that screen tasks (prompt P3). With none, every task needs a person's review.
    #[serde(default)]
    pub reviewers: Vec<String>,
    /// Model that rewrites session requests as issues (prompt P1).
    #[serde(default)]
    pub rewriter: Option<String>,
    /// The model the others are compared with (usually the one the team uses today).
    #[serde(default)]
    pub control: Option<String>,
    /// Context window and output limit given to the replayed agent.
    #[serde(default = "default_context_tokens")]
    pub context_tokens: u32,
    #[serde(default = "default_output_tokens")]
    pub output_tokens: u32,
    /// Agent steps before opencode tells the model to stop using tools.
    #[serde(default = "default_max_steps")]
    pub max_steps: u32,
    /// Wall-clock limit for one agent attempt, in minutes.
    #[serde(default = "default_attempt_minutes")]
    pub attempt_minutes: u32,
    #[serde(default = "default_workers")]
    pub workers: u32,
    /// Disk space each eval container may use, in MiB.
    #[serde(default = "default_storage_mib")]
    pub storage_mib: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    pub provider: String,
    pub model: String,
}

fn default_workers() -> u32 {
    4
}

fn default_context_tokens() -> u32 {
    32_768
}

fn default_output_tokens() -> u32 {
    8_192
}

fn default_max_steps() -> u32 {
    150
}

fn default_attempt_minutes() -> u32 {
    45
}

/// 2 GiB of disk per eval container unless configured otherwise.
pub const DEFAULT_STORAGE_MIB: u32 = 2048;

fn default_storage_mib() -> u32 {
    DEFAULT_STORAGE_MIB
}

/// What happened to a session the last time it was harvested.
pub type HarvestLedger = BTreeMap<String, String>;

/// File access for one eval root.
pub struct Store {
    layout: EvalLayout,
}

impl Store {
    pub fn new(layout: EvalLayout) -> Self {
        Self { layout }
    }

    pub fn layout(&self) -> &EvalLayout {
        &self.layout
    }

    pub fn load_config(&self) -> Result<Option<EvalConfig>> {
        read_json(&self.layout.config_file())
    }

    pub fn save_config(&self, config: &EvalConfig) -> Result<()> {
        atomic_write_json(&self.layout.config_file(), config)
    }

    pub fn save_task(&self, task: &TaskInstance) -> Result<()> {
        atomic_write_json(&self.layout.task_file(&task.instance_id), task)
    }

    pub fn save_json(&self, path: &Path, value: &impl Serialize) -> Result<()> {
        atomic_write_json(path, value)
    }

    pub fn save_text(&self, path: &Path, text: &str) -> Result<()> {
        atomic_write(path, text.as_bytes())
    }

    /// Every task on disk, sorted by id. Unreadable task files are an error, not skipped.
    pub fn tasks(&self) -> Result<Vec<TaskInstance>> {
        let dir = self.layout.tasks_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut tasks = Vec::new();
        for entry in
            fs::read_dir(&dir).with_context(|| format!("failed to list {}", dir.display()))?
        {
            let id = entry?.file_name().to_string_lossy().into_owned();
            if let Some(task) = read_json::<TaskInstance>(&self.layout.task_file(&id))? {
                tasks.push(task);
            }
        }
        tasks.sort_by(|a, b| a.instance_id.cmp(&b.instance_id));
        Ok(tasks)
    }

    pub fn load_spec(&self, repo_slug: &str) -> Result<Option<nasiko_evals::spec::EnvSpec>> {
        read_json(&self.layout.spec_file(repo_slug))
    }

    pub fn save_spec(&self, repo_slug: &str, spec: &nasiko_evals::spec::EnvSpec) -> Result<()> {
        atomic_write_json(&self.layout.spec_file(repo_slug), spec)
    }

    pub fn load_reviews(
        &self,
        instance_id: &str,
    ) -> Result<Vec<nasiko_evals::prompt::ScreenVerdict>> {
        let dir = self.layout.task_dir(instance_id).join("reviews");
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(&dir) else {
            return Ok(out);
        };
        for entry in entries.flatten() {
            if let Some(v) = read_json(&entry.path())? {
                out.push(v);
            }
        }
        Ok(out)
    }

    /// The config, or an error telling the user to run `nasiko eval init`.
    pub fn config(&self) -> Result<EvalConfig> {
        self.load_config()?
            .context("no eval config; run `nasiko eval init`")
    }

    pub fn load_ledger(&self, repo_slug: &str) -> Result<HarvestLedger> {
        Ok(read_json(&self.layout.harvest_ledger(repo_slug))?.unwrap_or_default())
    }

    pub fn save_ledger(&self, repo_slug: &str, ledger: &HarvestLedger) -> Result<()> {
        atomic_write_json(&self.layout.harvest_ledger(repo_slug), ledger)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_to_four_workers_and_two_gib() {
        let c: EvalConfig = serde_json::from_str("{}").expect("empty config parses");
        assert_eq!(c.workers, 4);
        assert_eq!(c.storage_mib, 2048);
    }

    #[test]
    fn tasks_round_trip_through_disk() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Store::new(EvalLayout::new(dir.path()));
        assert!(store.tasks().expect("empty root lists").is_empty());
        let mut ledger = HarvestLedger::new();
        ledger.insert("s1".into(), "no_match".into());
        store.save_ledger("repo", &ledger).expect("ledger saves");
        assert_eq!(store.load_ledger("repo").expect("ledger loads"), ledger);
    }
}
