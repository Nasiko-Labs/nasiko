//! Limits on the coding-agent telemetry ingest route
//! (`POST /api/telemetry/coding-agent/events/batch`).
//!
//! A history import sends thousands of turns at once, from many users at once.
//! These bound what one user, and the whole server, can push through the route
//! before it starts answering `429`/`503` and the client backs off.

use nasiko_utils::env_parse;

/// Default per-user ingest budget: thirty full batches a minute.
const DEFAULT_EVENTS_PER_MIN: u32 = 3_000;
/// Default cap on batches persisted at once, server-wide.
const DEFAULT_MAX_CONCURRENT: usize = 16;
/// Tempo's `ingestion_time_range_slack` (`oss/observability/config/tempo.yaml`):
/// 360h, 15 days. The two must agree — see `tempo_max_age_secs`.
const DEFAULT_TEMPO_MAX_AGE_SECS: u64 = 15 * 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodingAgentIngestConfig {
    /// Turns (events, not requests) one user may submit per minute.
    /// `CODING_AGENT_INGEST_EVENTS_PER_MIN`, default 3000, minimum 1.
    pub events_per_min: u32,
    /// Ingest requests persisted concurrently across all users; the next one is
    /// answered `503` instead of queueing on the database pool.
    /// `CODING_AGENT_INGEST_MAX_CONCURRENT`, default 16, minimum 1.
    pub max_concurrent: usize,
    /// Turns that started longer ago than this are never exported to Tempo or
    /// read back from it: Tempo drops spans outside its ingestion window, so the
    /// export would be lost work. Their spend comes from Postgres alone.
    /// `CODING_AGENT_TEMPO_MAX_AGE_SECS`, default 15 days — must not exceed
    /// Tempo's `ingestion_time_range_slack`.
    pub tempo_max_age_secs: u64,
}

impl Default for CodingAgentIngestConfig {
    fn default() -> Self {
        Self {
            events_per_min: DEFAULT_EVENTS_PER_MIN,
            max_concurrent: DEFAULT_MAX_CONCURRENT,
            tempo_max_age_secs: DEFAULT_TEMPO_MAX_AGE_SECS,
        }
    }
}

impl CodingAgentIngestConfig {
    pub fn from_env() -> Self {
        Self {
            events_per_min: env_parse("CODING_AGENT_INGEST_EVENTS_PER_MIN", DEFAULT_EVENTS_PER_MIN)
                .max(1),
            max_concurrent: env_parse("CODING_AGENT_INGEST_MAX_CONCURRENT", DEFAULT_MAX_CONCURRENT)
                .max(1),
            tempo_max_age_secs: env_parse(
                "CODING_AGENT_TEMPO_MAX_AGE_SECS",
                DEFAULT_TEMPO_MAX_AGE_SECS,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_tempo_age_matches_tempo_ingestion_slack() {
        let tempo = include_str!("../../observability/config/tempo.yaml");
        assert!(
            tempo.contains("ingestion_time_range_slack: 360h\n"),
            "tempo.yaml slack changed: update DEFAULT_TEMPO_MAX_AGE_SECS to match"
        );
        assert_eq!(
            CodingAgentIngestConfig::default().tempo_max_age_secs,
            360 * 60 * 60
        );
    }
}
