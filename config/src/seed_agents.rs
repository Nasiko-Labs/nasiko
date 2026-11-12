//! `SEED_AGENT_SETTINGS`: how each seeded agent (`SEED_AGENTS`) runs, by agent name.
//!
//! A JSON object keyed by the agent's name (the image's last path segment, without its tag):
//!
//! ```json
//! {"summarizer": {"min_replicas": 2, "max_replicas": 10, "replica_target": 4,
//!                 "cpu_limit_milli": 1000, "cpu_request_milli": 250, "memory_limit": "1Gi"}}
//! ```
//!
//! Every field is optional; a field left out is the platform's value, and an agent with no entry
//! runs as every seeded agent did before this existed. Replica bounds make the agent `stateless`
//! (it keeps no task state in memory), so an edition that autoscales may run it between them.
//! Unknown fields and impossible values fail startup: a typo would otherwise leave a shared agent
//! at one small replica without a word.

use std::collections::BTreeMap;

use serde::Deserialize;

/// What the platform's agents may not go below: under this, no agent pod is usable.
const MIN_MEMORY_MIB: u32 = 64;

/// The CPU limit an agent that sets none runs with; a request alone is checked against it.
/// Mirrors `nasiko_server::agent_scaling::AgentResources::DEFAULT_CPU_LIMIT_MILLI`.
const DEFAULT_CPU_LIMIT_MILLI: u32 = 500;

/// One seeded agent's settings, validated.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SeedAgentSettings {
    /// `(min, max)` replicas, `min <= max`; `None` keeps the agent at one replica.
    pub replicas: Option<(u32, u32)>,
    /// Requests one replica takes at a time, for the autoscaler; `None` is its estimate.
    pub replica_target: Option<u32>,
    pub cpu_limit_milli: Option<u32>,
    pub cpu_request_milli: Option<u32>,
    pub memory_limit_mib: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    min_replicas: Option<u32>,
    max_replicas: Option<u32>,
    replica_target: Option<u32>,
    cpu_limit_milli: Option<u32>,
    cpu_request_milli: Option<u32>,
    memory_limit: Option<String>,
}

/// `SEED_AGENT_SETTINGS` as read from `raw`; empty when unset or blank.
pub fn parse_seed_agent_settings(
    raw: Option<&str>,
) -> Result<BTreeMap<String, SeedAgentSettings>, String> {
    let Some(raw) = raw.map(str::trim).filter(|r| !r.is_empty()) else {
        return Ok(BTreeMap::new());
    };
    let entries: BTreeMap<String, serde_json::Value> = serde_json::from_str(raw)
        .map_err(|e| format!("SEED_AGENT_SETTINGS is not a JSON object of agent settings: {e}"))?;
    entries
        .into_iter()
        .map(|(name, entry)| {
            let settings = serde_json::from_value::<Raw>(entry)
                .map_err(|e| e.to_string())
                .and_then(validate)
                .map_err(|e| format!("SEED_AGENT_SETTINGS[{name:?}]: {e}"))?;
            Ok((name, settings))
        })
        .collect()
}

fn validate(raw: Raw) -> Result<SeedAgentSettings, String> {
    let replicas = match (raw.min_replicas, raw.max_replicas) {
        (None, None) => None,
        (min, max) => {
            let min = min.unwrap_or(1);
            let max = max.unwrap_or(min);
            if min == 0 || max < min {
                return Err(format!(
                    "min_replicas ({min}) must be at least 1 and at most max_replicas ({max})"
                ));
            }
            Some((min, max))
        }
    };
    if raw.replica_target == Some(0) {
        return Err("replica_target must be at least 1".to_owned());
    }
    if raw.replica_target.is_some() && replicas.is_none() {
        return Err("replica_target needs min_replicas or max_replicas".to_owned());
    }
    if raw.cpu_limit_milli == Some(0) {
        return Err("cpu_limit_milli must be at least 1".to_owned());
    }
    if let Some(request) = raw.cpu_request_milli {
        let limit = raw.cpu_limit_milli.unwrap_or(DEFAULT_CPU_LIMIT_MILLI);
        if request == 0 || request > limit {
            return Err(format!(
                "cpu_request_milli ({request}) must be at least 1 and at most the CPU limit ({limit})"
            ));
        }
    }
    let memory_limit_mib = raw.memory_limit.as_deref().map(memory_mib).transpose()?;
    Ok(SeedAgentSettings {
        replicas,
        replica_target: raw.replica_target,
        cpu_limit_milli: raw.cpu_limit_milli,
        cpu_request_milli: raw.cpu_request_milli,
        memory_limit_mib,
    })
}

/// `memory` (`"512Mi"`, `"2Gi"`) in MiB: whole MiB or GiB only, at least [`MIN_MEMORY_MIB`].
fn memory_mib(memory: &str) -> Result<u32, String> {
    let invalid = || {
        format!(
            "memory_limit {memory:?} must be whole Mi or Gi (e.g. \"512Mi\", \"2Gi\"), at least \
             {MIN_MEMORY_MIB}Mi"
        )
    };
    let (digits, per_unit) = if let Some(n) = memory.strip_suffix("Gi") {
        (n, 1024)
    } else if let Some(n) = memory.strip_suffix("Mi") {
        (n, 1)
    } else {
        return Err(invalid());
    };
    digits
        .parse::<u32>()
        .ok()
        .and_then(|n| n.checked_mul(per_unit))
        .filter(|mib| *mib >= MIN_MEMORY_MIB)
        .ok_or_else(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(json: &str) -> Result<SeedAgentSettings, String> {
        parse_seed_agent_settings(Some(&format!(r#"{{"a": {json}}}"#)))
            .map(|mut all| all.remove("a").unwrap())
    }

    #[test]
    fn unset_or_blank_is_no_settings() {
        assert!(parse_seed_agent_settings(None).unwrap().is_empty());
        assert!(parse_seed_agent_settings(Some("  ")).unwrap().is_empty());
    }

    #[test]
    fn every_field_is_read() {
        assert_eq!(
            one(
                r#"{"min_replicas": 2, "max_replicas": 10, "replica_target": 4,
                    "cpu_limit_milli": 1000, "cpu_request_milli": 250, "memory_limit": "1Gi"}"#
            ),
            Ok(SeedAgentSettings {
                replicas: Some((2, 10)),
                replica_target: Some(4),
                cpu_limit_milli: Some(1000),
                cpu_request_milli: Some(250),
                memory_limit_mib: Some(1024),
            })
        );
    }

    #[test]
    fn one_replica_bound_fills_the_other() {
        assert_eq!(
            one(r#"{"max_replicas": 5}"#).unwrap().replicas,
            Some((1, 5))
        );
        assert_eq!(
            one(r#"{"min_replicas": 3}"#).unwrap().replicas,
            Some((3, 3))
        );
        assert_eq!(one("{}").unwrap(), SeedAgentSettings::default());
    }

    #[test]
    fn impossible_values_are_refused() {
        for bad in [
            r#"{"min_replicas": 0}"#,
            r#"{"min_replicas": 4, "max_replicas": 2}"#,
            r#"{"max_replicas": 4, "replica_target": 0}"#,
            r#"{"replica_target": 4}"#,
            r#"{"cpu_limit_milli": 0}"#,
            r#"{"cpu_request_milli": 600}"#,
            r#"{"cpu_limit_milli": 1000, "cpu_request_milli": 1001}"#,
            r#"{"memory_limit": "1G"}"#,
            r#"{"memory_limit": "32Mi"}"#,
            r#"{"memory_limit": "1.5Gi"}"#,
            r#"{"max_replica": 4}"#,
        ] {
            assert!(one(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_error_names_the_agent() {
        for bad in [r#"{"min_replicas": 0}"#, r#"{"max_replica": 4}"#] {
            let err = parse_seed_agent_settings(Some(&format!(r#"{{"summarizer": {bad}}}"#)))
                .unwrap_err();
            assert!(err.contains("\"summarizer\""), "{err}");
        }
        assert!(parse_seed_agent_settings(Some("[1]")).is_err());
    }
}
