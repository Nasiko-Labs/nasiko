//! `/api/me/coding-agents/sync*` — progress for bringing a user's coding agents in.
//!
//! A browser cannot read the user's local files, so the work (discover the coding agents, install
//! their hooks, import past sessions) is always done by a local client: the desktop app or the
//! `nasiko` CLI. Consent belongs to the client — running `nasiko agents setup` is the answer, and
//! the desktop app keeps its own — so the server only stores the progress each local client
//! reports, and the web UI shows it without talking to the client directly.
//!
//! Not superuser-gated — a user owns their own devices' runs.
//!
//! Responses are **bare JSON**, like the neighbouring `/api/me/*` routes in `context_selection.rs`.
//! Every malformed or out-of-range body is a plain-text `400` (not Axum's default `422`), because
//! the local clients and the web UI share one wire contract that says so.

use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::FromRow;

use crate::auth::Claims;
use crate::state::AppState;

/// Most agents one report may carry. The catalog has four; the headroom is for new integrations.
const MAX_AGENTS: usize = 10;
const MAX_DEVICE_ID_CHARS: usize = 64;
const MAX_DEVICE_NAME_CHARS: usize = 128;
const MAX_AGENT_ID_CHARS: usize = 32;
const MAX_AGENT_NAME_CHARS: usize = 64;
const MAX_ERROR_CHARS: usize = 500;
/// Most read windows one report may carry (e.g. "Last 8 hours", "Last day", …).
const MAX_WINDOWS: usize = 40;
const MAX_WINDOW_LABEL_CHARS: usize = 64;
/// Most distinct quarantine reasons one agent may report; the client sends its top few.
const MAX_REJECTED_REASONS: usize = 5;
const MAX_REJECTED_REASON_CHARS: usize = 200;
/// How far back `GET /sync` looks: a run nobody has reported on for a day is not "in progress".
const SYNC_RUN_WINDOW_HOURS: i32 = 24;
/// A bound on the read, not a product limit — no user has this many devices reporting in a day.
const MAX_SYNC_RUNS_RETURNED: i64 = 100;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/me/coding-agents/sync", get(list_sync_runs))
        .route("/me/coding-agents/sync/{device_id}", put(report_sync))
        .route(
            "/me/coding-agents/sync/{device_id}/retry",
            post(request_sync_retry),
        )
}

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SyncClient {
    Desktop,
    Cli,
}

impl SyncClient {
    fn as_str(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Cli => "cli",
        }
    }
}

/// Where a device's run is. Clients no longer send `awaiting_consent` (consent is local to the
/// client); it is still accepted so a report from an older client is not a 400.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SyncPhase {
    AwaitingConsent,
    Discovering,
    Connecting,
    Importing,
    /// Every window has been read; queued turns are still being delivered. Undelivered turns
    /// keep a run here, never in `failed`.
    Sending,
    Done,
    Failed,
}

impl SyncPhase {
    /// The phase that opens a run: moving into it from a resting phase restamps `started_at`.
    const RUN_START: Self = Self::Discovering;
    /// Phases a device rests in between runs.
    const RESTING: [Self; 3] = [Self::AwaitingConsent, Self::Done, Self::Failed];

    fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingConsent => "awaiting_consent",
            Self::Discovering => "discovering",
            Self::Connecting => "connecting",
            Self::Importing => "importing",
            Self::Sending => "sending",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportStatus {
    Waiting,
    Running,
    Done,
    Failed,
    /// The agent is not on this computer, or is not connected.
    Skipped,
}

/// One agent's past-session import.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ImportProgress {
    status: ImportStatus,
    /// Sessions processed so far.
    done: u32,
    /// Sessions to process.
    total: u32,
    /// Sessions that had new turns.
    sessions: u32,
    /// Turns queued.
    turns: u32,
    /// Sessions that could not be read.
    failed: u32,
    #[serde(default)]
    error: Option<String>,
    /// Turns queued locally that the server has not accepted yet.
    #[serde(default)]
    pending: u32,
    /// Turns quarantined by the client.
    #[serde(default)]
    rejected: u32,
    /// Why turns were quarantined, most frequent first.
    #[serde(default)]
    rejected_reasons: Vec<RejectedReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RejectedReason {
    reason: String,
    count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WindowStatus {
    Waiting,
    Running,
    /// Read, and every turn in it accepted by the server.
    Done,
}

/// One slice of history the client reads, newest first (e.g. "Last 8 hours").
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SyncWindow {
    label: String,
    status: WindowStatus,
    sessions: u32,
    turns: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SyncAgent {
    /// Integration id, e.g. `claude`, `codex`, `opencode`, `cursor`.
    id: String,
    /// Display name, e.g. `Claude Code`.
    name: String,
    detected: bool,
    connected: bool,
    import: ImportProgress,
}

/// The body a local client sends to `PUT /api/me/coding-agents/sync/{device_id}`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SyncReport {
    device_name: String,
    client: SyncClient,
    phase: SyncPhase,
    agents: Vec<SyncAgent>,
    #[serde(default)]
    error: Option<String>,
    /// Read windows, newest first. Absent from v1 clients.
    #[serde(default)]
    windows: Vec<SyncWindow>,
}

/// One device's latest run, as stored.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SyncRun {
    device_id: String,
    device_name: String,
    client: SyncClient,
    phase: SyncPhase,
    agents: Vec<SyncAgent>,
    error: Option<String>,
    /// When the current run entered `discovering`; `null` if this device never started one.
    started_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
    windows: Vec<SyncWindow>,
    /// When the user last asked this device to re-send its quarantined turns; `null` if never.
    retry_requested_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SyncRunsResponse {
    runs: Vec<SyncRun>,
}

// ── Boundary validation ───────────────────────────────────────────────────────

/// Enums are already checked by serde; this checks what serde cannot: lengths and charsets.
fn validate_device_id(device_id: &str) -> Result<(), String> {
    let is_allowed = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    if device_id.is_empty() || device_id.chars().count() > MAX_DEVICE_ID_CHARS {
        return Err(format!(
            "device_id must be 1 to {MAX_DEVICE_ID_CHARS} characters"
        ));
    }
    if !device_id.chars().all(is_allowed) {
        return Err("device_id may contain only A-Z, a-z, 0-9, '_' and '-'".into());
    }
    Ok(())
}

fn validate_len(field: &str, value: &str, max: usize) -> Result<(), String> {
    match value.chars().count() {
        0 => Err(format!("{field} must not be empty")),
        n if n > max => Err(format!("{field} must be at most {max} characters")),
        _ => Ok(()),
    }
}

fn validate_error(field: &str, value: Option<&str>) -> Result<(), String> {
    match value {
        Some(text) if text.chars().count() > MAX_ERROR_CHARS => Err(format!(
            "{field} must be at most {MAX_ERROR_CHARS} characters"
        )),
        _ => Ok(()),
    }
}

impl SyncAgent {
    fn validate(&self, index: usize) -> Result<(), String> {
        let is_allowed =
            |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-';
        validate_len(&format!("agents[{index}].id"), &self.id, MAX_AGENT_ID_CHARS)?;
        if !self.id.chars().all(is_allowed) {
            return Err(format!(
                "agents[{index}].id may contain only a-z, 0-9, '_' and '-'"
            ));
        }
        validate_len(
            &format!("agents[{index}].name"),
            &self.name,
            MAX_AGENT_NAME_CHARS,
        )?;
        validate_error(
            &format!("agents[{index}].import.error"),
            self.import.error.as_deref(),
        )?;
        self.import.validate_rejected_reasons(index)
    }
}

impl ImportProgress {
    fn validate_rejected_reasons(&self, agent_index: usize) -> Result<(), String> {
        let field = format!("agents[{agent_index}].import.rejected_reasons");
        if self.rejected_reasons.len() > MAX_REJECTED_REASONS {
            return Err(format!(
                "{field} must hold at most {MAX_REJECTED_REASONS} items"
            ));
        }
        self.rejected_reasons
            .iter()
            .enumerate()
            .try_for_each(|(index, rejected)| {
                validate_len(
                    &format!("{field}[{index}].reason"),
                    &rejected.reason,
                    MAX_REJECTED_REASON_CHARS,
                )
            })
    }
}

fn validate_windows(windows: &[SyncWindow]) -> Result<(), String> {
    if windows.len() > MAX_WINDOWS {
        return Err(format!("windows must hold at most {MAX_WINDOWS} items"));
    }
    windows.iter().enumerate().try_for_each(|(index, window)| {
        validate_len(
            &format!("windows[{index}].label"),
            &window.label,
            MAX_WINDOW_LABEL_CHARS,
        )
    })
}

impl SyncReport {
    fn validate(&self) -> Result<(), String> {
        validate_len("device_name", &self.device_name, MAX_DEVICE_NAME_CHARS)?;
        if self.agents.len() > MAX_AGENTS {
            return Err(format!("agents must hold at most {MAX_AGENTS} items"));
        }
        for (index, agent) in self.agents.iter().enumerate() {
            agent.validate(index)?;
        }
        validate_windows(&self.windows)?;
        validate_error("error", self.error.as_deref())
    }
}

// ── Responses ─────────────────────────────────────────────────────────────────

fn bad_request(message: impl Into<String>) -> Response {
    (StatusCode::BAD_REQUEST, message.into()).into_response()
}

fn internal_error(context: &str, error: impl std::fmt::Display) -> Response {
    tracing::error!(error = %error, "{context} failed");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
}

/// Decode a stored enum. The column holds the wire string, so serde reads it back; `as_str` on
/// each enum writes it, and `enums_round_trip_through_their_db_strings` keeps the two in step.
fn from_db<T: DeserializeOwned>(text: String) -> Result<T, serde_json::Error> {
    serde_json::from_value(serde_json::Value::String(text))
}

// ── Sync progress ─────────────────────────────────────────────────────────────

#[derive(Debug, FromRow)]
struct SyncRunRow {
    device_id: String,
    device_name: String,
    client: String,
    phase: String,
    agents: sqlx::types::Json<Vec<SyncAgent>>,
    error: Option<String>,
    started_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
    windows: sqlx::types::Json<Vec<SyncWindow>>,
    retry_requested_at: Option<DateTime<Utc>>,
}

impl TryFrom<SyncRunRow> for SyncRun {
    type Error = serde_json::Error;

    fn try_from(row: SyncRunRow) -> Result<Self, Self::Error> {
        Ok(Self {
            device_id: row.device_id,
            device_name: row.device_name,
            client: from_db(row.client)?,
            phase: from_db(row.phase)?,
            agents: row.agents.0,
            error: row.error,
            started_at: row.started_at,
            updated_at: row.updated_at,
            windows: row.windows.0,
            retry_requested_at: row.retry_requested_at,
        })
    }
}

const SYNC_RUN_COLUMNS: &str = "device_id, device_name, client, phase, agents, error, \
     started_at, updated_at, windows, retry_requested_at";

#[utoipa::path(
    put,
    path = "/api/me/coding-agents/sync/{device_id}",
    tag = "coding-agents",
    params(("device_id" = String, Path, description = "The reporting device: 1 to 64 characters of [A-Za-z0-9_-]")),
    request_body = SyncReport,
    responses(
        (status = 200, description = "The stored run", body = SyncRun),
        (status = 400, description = "Plain text: which field failed validation"),
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn report_sync(
    State(state): State<AppState>,
    claims: Claims,
    Path(device_id): Path<String>,
    body: Result<Json<SyncReport>, JsonRejection>,
) -> Response {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    if let Err(message) = validate_device_id(&device_id) {
        return bad_request(message);
    }
    let report = match body {
        Ok(Json(report)) => report,
        Err(e) => return bad_request(format!("invalid sync report: {}", e.body_text())),
    };
    if let Err(message) = report.validate() {
        return bad_request(message);
    }

    // `started_at` restamps when the phase moves into `discovering` from a resting phase (or on
    // the device's first row); otherwise the run in progress keeps its start. Decided in the upsert
    // itself so two concurrent reports cannot both read the old phase and disagree.
    let resting: Vec<&str> = SyncPhase::RESTING
        .into_iter()
        .map(SyncPhase::as_str)
        .collect();
    let sql = format!(
        "INSERT INTO coding_agent_sync_runs AS run \
             (user_id, device_id, device_name, client, phase, agents, error, started_at, \
              updated_at, windows) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, CASE WHEN $5 = $8 THEN now() END, now(), $10) \
         ON CONFLICT (user_id, device_id) DO UPDATE SET \
             device_name = EXCLUDED.device_name, \
             client = EXCLUDED.client, \
             phase = EXCLUDED.phase, \
             agents = EXCLUDED.agents, \
             error = EXCLUDED.error, \
             windows = EXCLUDED.windows, \
             started_at = CASE WHEN EXCLUDED.phase = $8 AND run.phase = ANY($9) \
                               THEN now() ELSE run.started_at END, \
             updated_at = now() \
         RETURNING {SYNC_RUN_COLUMNS}"
    );
    let row: SyncRunRow = match sqlx::query_as(&sql)
        .bind(user_id)
        .bind(&device_id)
        .bind(&report.device_name)
        .bind(report.client.as_str())
        .bind(report.phase.as_str())
        .bind(sqlx::types::Json(&report.agents))
        .bind(&report.error)
        .bind(SyncPhase::RUN_START.as_str())
        .bind(&resting)
        .bind(sqlx::types::Json(&report.windows))
        .fetch_one(&state.db)
        .await
    {
        Ok(row) => row,
        Err(e) => return internal_error("report_sync: write", e),
    };
    match SyncRun::try_from(row) {
        Ok(run) => Json(run).into_response(),
        Err(e) => internal_error("report_sync: decode", e),
    }
}

/// Ask a device to re-send its quarantined turns. The server only records the request; the
/// client sees a `retry_requested_at` newer than the last one it handled on its next report.
#[utoipa::path(
    post,
    path = "/api/me/coding-agents/sync/{device_id}/retry",
    tag = "coding-agents",
    params(("device_id" = String, Path, description = "The device whose quarantined turns to re-send")),
    responses(
        (status = 200, description = "The run, with `retry_requested_at` set to now", body = SyncRun),
        (status = 400, description = "Plain text: the device_id is malformed"),
        (status = 404, description = "Plain text: this device has no run for the caller"),
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn request_sync_retry(
    State(state): State<AppState>,
    claims: Claims,
    Path(device_id): Path<String>,
) -> Response {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    if let Err(message) = validate_device_id(&device_id) {
        return bad_request(message);
    }
    let sql = format!(
        "UPDATE coding_agent_sync_runs SET retry_requested_at = now() \
          WHERE user_id = $1 AND device_id = $2 \
          RETURNING {SYNC_RUN_COLUMNS}"
    );
    let row: Option<SyncRunRow> = match sqlx::query_as(&sql)
        .bind(user_id)
        .bind(&device_id)
        .fetch_optional(&state.db)
        .await
    {
        Ok(row) => row,
        Err(e) => return internal_error("request_sync_retry: write", e),
    };
    let Some(row) = row else {
        return (StatusCode::NOT_FOUND, "no sync run for this device").into_response();
    };
    match SyncRun::try_from(row) {
        Ok(run) => Json(run).into_response(),
        Err(e) => internal_error("request_sync_retry: decode", e),
    }
}

#[utoipa::path(
    get,
    path = "/api/me/coding-agents/sync",
    tag = "coding-agents",
    responses(
        (status = 200, description = "The caller's runs updated in the last 24 hours, newest first", body = SyncRunsResponse),
        (status = 500, description = "Read failed"),
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn list_sync_runs(State(state): State<AppState>, claims: Claims) -> Response {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    let sql = format!(
        "SELECT {SYNC_RUN_COLUMNS} FROM coding_agent_sync_runs \
          WHERE user_id = $1 AND updated_at > now() - make_interval(hours => $2) \
          ORDER BY updated_at DESC LIMIT $3"
    );
    let rows: Vec<SyncRunRow> = match sqlx::query_as(&sql)
        .bind(user_id)
        .bind(SYNC_RUN_WINDOW_HOURS)
        .bind(MAX_SYNC_RUNS_RETURNED)
        .fetch_all(&state.db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => return internal_error("list_sync_runs: read", e),
    };
    match rows.into_iter().map(SyncRun::try_from).collect() {
        Ok(runs) => Json(SyncRunsResponse { runs }).into_response(),
        Err(e) => internal_error("list_sync_runs: decode", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn agent(id: &str) -> serde_json::Value {
        json!({
            "id": id,
            "name": "Claude Code",
            "detected": true,
            "connected": true,
            "import": {
                "status": "running", "done": 40, "total": 123,
                "sessions": 98, "turns": 753, "failed": 0, "error": null
            }
        })
    }

    fn report(agents: Vec<serde_json::Value>) -> serde_json::Value {
        json!({
            "device_name": "Example MacBook",
            "client": "desktop",
            "phase": "importing",
            "agents": agents,
            "error": null
        })
    }

    fn parse(value: serde_json::Value) -> Result<SyncReport, serde_json::Error> {
        serde_json::from_value(value)
    }

    #[test]
    fn accepts_contract_example_report() {
        let parsed = parse(report(vec![agent("claude")])).unwrap();
        assert_eq!(parsed.validate(), Ok(()));
    }

    #[test]
    fn accepts_report_without_optional_errors() {
        let mut body = report(vec![]);
        body.as_object_mut().unwrap().remove("error");
        assert_eq!(parse(body).unwrap().validate(), Ok(()));
    }

    #[test]
    fn rejects_unknown_phase_and_status() {
        let mut body = report(vec![]);
        body["phase"] = json!("paused");
        assert!(parse(body).is_err());

        let mut bad_agent = agent("claude");
        bad_agent["import"]["status"] = json!("queued");
        assert!(parse(report(vec![bad_agent])).is_err());
    }

    #[test]
    fn rejects_negative_counters() {
        let mut bad_agent = agent("claude");
        bad_agent["import"]["done"] = json!(-1);
        assert!(parse(report(vec![bad_agent])).is_err());
    }

    #[test]
    fn rejects_more_than_max_agents() {
        let agents = (0..=MAX_AGENTS).map(|_| agent("claude")).collect();
        let err = parse(report(agents)).unwrap().validate().unwrap_err();
        assert!(err.contains("at most 10"), "{err}");
    }

    #[test]
    fn accepts_exactly_max_agents() {
        let agents = (0..MAX_AGENTS).map(|_| agent("claude")).collect();
        assert_eq!(parse(report(agents)).unwrap().validate(), Ok(()));
    }

    #[test]
    fn rejects_bad_agent_id_charset_and_length() {
        for id in [
            "Claude",
            "claude code",
            "",
            &"a".repeat(MAX_AGENT_ID_CHARS + 1),
        ] {
            let err = parse(report(vec![agent(id)])).unwrap().validate();
            assert!(err.is_err(), "agent id {id:?} should be rejected");
        }
    }

    #[test]
    fn rejects_empty_or_long_device_name() {
        for name in [String::new(), "x".repeat(MAX_DEVICE_NAME_CHARS + 1)] {
            let mut body = report(vec![]);
            body["device_name"] = json!(name);
            assert!(parse(body).unwrap().validate().is_err());
        }
    }

    #[test]
    fn rejects_long_errors() {
        let long = "e".repeat(MAX_ERROR_CHARS + 1);
        let mut body = report(vec![]);
        body["error"] = json!(long);
        assert!(parse(body).unwrap().validate().is_err());

        let mut bad_agent = agent("claude");
        bad_agent["import"]["error"] = json!(long);
        let err = parse(report(vec![bad_agent]))
            .unwrap()
            .validate()
            .unwrap_err();
        assert!(err.contains("agents[0].import.error"), "{err}");
    }

    #[test]
    fn v1_report_defaults_the_v2_fields() {
        let parsed = parse(report(vec![agent("claude")])).unwrap();
        assert!(parsed.windows.is_empty());
        let import = &parsed.agents[0].import;
        assert_eq!((import.pending, import.rejected), (0, 0));
        assert!(import.rejected_reasons.is_empty());
    }

    fn window(label: &str) -> serde_json::Value {
        json!({"label": label, "status": "done", "sessions": 12, "turns": 140})
    }

    #[test]
    fn accepts_v2_report_with_sending_windows_and_rejections() {
        let mut body = report(vec![agent("claude")]);
        body["phase"] = json!("sending");
        body["windows"] = json!([window("Last 8 hours"), window("Last day")]);
        body["agents"][0]["import"]["pending"] = json!(7);
        body["agents"][0]["import"]["rejected"] = json!(3);
        body["agents"][0]["import"]["rejected_reasons"] =
            json!([{"reason": "too big", "count": 3}]);
        let parsed = parse(body).unwrap();
        assert_eq!(parsed.validate(), Ok(()));
        assert_eq!(parsed.phase, SyncPhase::Sending);
        assert_eq!(parsed.windows[0].status, WindowStatus::Done);
    }

    #[test]
    fn rejects_bad_windows() {
        let mut too_many = report(vec![]);
        too_many["windows"] = json!(vec![window("w"); MAX_WINDOWS + 1]);
        assert!(parse(too_many).unwrap().validate().is_err());

        let mut exactly_max = report(vec![]);
        exactly_max["windows"] = json!(vec![window("w"); MAX_WINDOWS]);
        assert_eq!(parse(exactly_max).unwrap().validate(), Ok(()));

        for label in [String::new(), "l".repeat(MAX_WINDOW_LABEL_CHARS + 1)] {
            let mut body = report(vec![]);
            body["windows"] = json!([window(&label)]);
            let err = parse(body).unwrap().validate().unwrap_err();
            assert!(err.contains("windows[0].label"), "{err}");
        }

        let mut bad_status = report(vec![]);
        bad_status["windows"] =
            json!([{"label": "x", "status": "failed", "sessions": 0, "turns": 0}]);
        assert!(parse(bad_status).is_err());

        let mut negative = report(vec![]);
        negative["windows"] = json!([{"label": "x", "status": "done", "sessions": -1, "turns": 0}]);
        assert!(parse(negative).is_err());
    }

    #[test]
    fn rejects_bad_rejected_reasons() {
        let reason = |text: String| json!({"reason": text, "count": 1});
        let mut too_many = agent("claude");
        too_many["import"]["rejected_reasons"] =
            json!(vec![reason("r".into()); MAX_REJECTED_REASONS + 1]);
        assert!(parse(report(vec![too_many])).unwrap().validate().is_err());

        for text in [String::new(), "r".repeat(MAX_REJECTED_REASON_CHARS + 1)] {
            let mut bad = agent("claude");
            bad["import"]["rejected_reasons"] = json!([reason(text)]);
            let err = parse(report(vec![bad])).unwrap().validate().unwrap_err();
            assert!(
                err.contains("agents[0].import.rejected_reasons[0].reason"),
                "{err}"
            );
        }
    }

    #[test]
    fn device_id_allows_only_the_contract_charset() {
        assert_eq!(validate_device_id("3f2a9c_Mac-1"), Ok(()));
        assert_eq!(validate_device_id(&"a".repeat(MAX_DEVICE_ID_CHARS)), Ok(()));
        for bad in ["", "has space", "dot.ted", "slash/x", "ünïcode"] {
            assert!(
                validate_device_id(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
        assert!(validate_device_id(&"a".repeat(MAX_DEVICE_ID_CHARS + 1)).is_err());
    }

    #[test]
    fn enums_round_trip_through_their_db_strings() {
        use SyncPhase::*;
        for phase in [
            AwaitingConsent,
            Discovering,
            Connecting,
            Importing,
            Sending,
            Done,
            Failed,
        ] {
            assert_eq!(from_db::<SyncPhase>(phase.as_str().into()).unwrap(), phase);
        }
        for client in [SyncClient::Desktop, SyncClient::Cli] {
            assert_eq!(
                from_db::<SyncClient>(client.as_str().into()).unwrap(),
                client
            );
        }
    }
}
