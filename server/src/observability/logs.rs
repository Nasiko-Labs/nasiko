//! Agent log query helpers.
//!
//! Two log sources are combined in priority order:
//!
//! 1. **Loki** (when `LOKI_URL` is configured) — structured logs shipped by the
//!    agent container via OpenTelemetry.  Richest source: includes level, trace_id,
//!    and any structured fields the agent emits.
//!
//! 2. **`proxy_logs` table** (always available) — A2A call audit entries written by
//!    the proxy middleware on every inter-agent request.  Coverage is complete even
//!    when Loki is not configured.
//!
//! 3. **Container stdout/stderr** (always available) — Raw output from the container
//!    runtime (`runtime.logs()`).  Unstructured but useful for startup errors and
//!    `print()` debugging.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

/// A single log line returned to the client.
#[derive(Debug, Serialize, Clone)]
pub struct LogLine {
    pub timestamp: DateTime<Utc>,
    /// "INFO" | "WARN" | "ERROR" | "DEBUG" — may be None for raw container output
    pub level: Option<String>,
    pub message: String,
    /// Present when the log was emitted in the context of an A2A flow
    pub trace_id: Option<String>,
    /// "proxy" | "container" | "loki"
    pub source: &'static str,
}

/// Query parameters for the logs endpoint.
#[derive(Debug)]
pub struct LogQuery {
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub limit: usize,
    pub level: Option<String>,
    pub search: Option<String>,
}

impl Default for LogQuery {
    fn default() -> Self {
        Self {
            since: None,
            until: None,
            limit: 200,
            level: None,
            search: None,
        }
    }
}

/// Fetch logs for `agent_id` from the `proxy_logs` DB table.
///
/// These are A2A-level call records: every proxied request through the CP is
/// logged here with caller, method, latency, and HTTP status.
pub async fn query_proxy_logs(db: &PgPool, agent_id: Uuid, q: &LogQuery) -> Vec<LogLine> {
    let since = q
        .since
        .unwrap_or_else(|| Utc::now() - chrono::Duration::hours(24));
    let limit = q.limit.min(500) as i64;

    // Postgres: status=i32, latency_ms=i64
    let rows: Vec<(DateTime<Utc>, i32, i64, Option<String>)> = sqlx::query_as(
        r#"
        SELECT timestamp, status, latency_ms, error
        FROM proxy_logs
        WHERE (caller_id = $1 OR target_agent_id = $1)
          AND timestamp >= $2
        ORDER BY timestamp DESC
        LIMIT $3
        "#,
    )
    .bind(agent_id)
    .bind(since)
    .bind(limit)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    rows.into_iter()
        .map(|(ts, status, latency_ms, error)| {
            let level = if status >= 500 {
                Some("ERROR".into())
            } else if status >= 400 {
                Some("WARN".into())
            } else {
                Some("INFO".into())
            };
            let message = match error {
                Some(e) => format!("A2A call → HTTP {status}  ({latency_ms}ms) — {e}"),
                None => format!("A2A call → HTTP {status}  ({latency_ms}ms)"),
            };
            LogLine {
                timestamp: ts,
                level,
                message,
                trace_id: None,
                source: "proxy",
            }
        })
        .collect()
}

/// Parse raw container log lines (from `runtime.logs()`) into `LogLine` structs.
///
/// Lines are attempted to be parsed as JSON (OTel log format). On failure the
/// raw text is kept as-is with a heuristic level.
pub fn parse_container_logs(raw_lines: Vec<String>) -> Vec<LogLine> {
    let now = Utc::now();
    raw_lines
        .into_iter()
        .filter(|l| !l.trim().is_empty())
        .map(|line| parse_log_line(line, now, "container"))
        .collect()
}

/// Parse Loki `(timestamp, line)` entries into `LogLine` structs.
///
/// Loki already provides correct timestamps; JSON structure is parsed the same
/// way as container logs, but `source` is set to `"loki"`.
pub fn parse_loki_logs(entries: Vec<(DateTime<Utc>, String)>) -> Vec<LogLine> {
    entries
        .into_iter()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(ts, line)| parse_log_line(line, ts, "loki"))
        .collect()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a single log line into a `LogLine`, using `fallback_ts` when no
/// structured timestamp is available.
fn parse_log_line(line: String, fallback_ts: DateTime<Utc>, source: &'static str) -> LogLine {
    // Try to parse as OTel/structured JSON: {"timestamp":"…","level":"…","body":"…","trace_id":"…"}
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
        let timestamp = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(|s| s.parse::<DateTime<Utc>>().ok())
            .unwrap_or(fallback_ts);
        let level = v
            .get("level")
            .or_else(|| v.get("severity"))
            .and_then(|l| l.as_str())
            .map(|s| s.to_uppercase());
        let message = v
            .get("body")
            .or_else(|| v.get("message").or_else(|| v.get("msg")))
            .and_then(|m| m.as_str())
            .unwrap_or(&line)
            .to_string();
        let trace_id = v
            .get("trace_id")
            .and_then(|t| t.as_str())
            .map(str::to_string);
        LogLine {
            timestamp,
            level,
            message,
            trace_id,
            source,
        }
    } else {
        // Raw text — detect common log level prefixes
        let level = Some(detect_level(&line).into());
        LogLine {
            timestamp: fallback_ts,
            level,
            message: line,
            trace_id: None,
            source,
        }
    }
}

fn detect_level(line: &str) -> &'static str {
    if line.contains("ERROR") || line.contains("CRITICAL") {
        "ERROR"
    } else if line.contains("WARN") {
        "WARN"
    } else if line.contains("DEBUG") {
        "DEBUG"
    } else {
        "INFO"
    }
}
