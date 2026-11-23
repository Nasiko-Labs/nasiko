//! Where every eval file lives under the eval root (default `~/.nasiko/eval/`).
//!
//! Only path building; nothing here touches the disk. Keeping every path in one place means the
//! folder layout is defined once.

use std::path::{Path, PathBuf};

/// The eval root and the paths below it.
#[derive(Debug, Clone)]
pub struct EvalLayout {
    root: PathBuf,
}

impl EvalLayout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.json")
    }

    pub fn tasks_dir(&self) -> PathBuf {
        self.root.join("tasks")
    }

    pub fn task_dir(&self, instance_id: &str) -> PathBuf {
        self.tasks_dir().join(instance_id)
    }

    pub fn task_file(&self, instance_id: &str) -> PathBuf {
        self.task_dir(instance_id).join("instance.json")
    }

    /// The distilled session the task came from (typed messages, edited files, usage).
    pub fn source_file(&self, instance_id: &str) -> PathBuf {
        self.task_dir(instance_id).join("source.json")
    }

    /// Candidate commits and their scores.
    pub fn link_file(&self, instance_id: &str) -> PathBuf {
        self.task_dir(instance_id).join("link.json")
    }

    /// One reviewer verdict (prompt P3, or a person) per file.
    pub fn review_file(&self, instance_id: &str, reviewer: &str) -> PathBuf {
        let safe: String = reviewer
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.task_dir(instance_id)
            .join("reviews")
            .join(format!("{safe}.json"))
    }

    pub fn spec_file(&self, repo_slug: &str) -> PathBuf {
        self.root.join("specs").join(format!("{repo_slug}.json"))
    }

    pub fn dataset_dir(&self, name: &str, version: u32) -> PathBuf {
        self.root
            .join("datasets")
            .join(name)
            .join(format!("v{version}"))
    }

    pub fn run_dir(&self, run_id: &str) -> PathBuf {
        self.root.join("runs").join(run_id)
    }

    /// Sessions looked at and skipped, so a second harvest does not redo them.
    pub fn harvest_ledger(&self, repo_slug: &str) -> PathBuf {
        self.root.join("harvest").join(format!("{repo_slug}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_sit_under_root() {
        let l = EvalLayout::new("/e");
        assert_eq!(
            l.task_file("r__1"),
            PathBuf::from("/e/tasks/r__1/instance.json")
        );
        assert_eq!(l.dataset_dir("ds", 3), PathBuf::from("/e/datasets/ds/v3"));
        assert_eq!(l.spec_file("r"), PathBuf::from("/e/specs/r.json"));
    }
}
