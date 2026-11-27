//! A per-server cap on dashboard reads: the Overview's estate, the spend pages and the session
//! list. Each is many reads over the usage tables, up to six at a time, so a burst of page opens
//! used to take every connection in the pool and stall chat with it. Past the cap a request waits
//! up to [`ADMISSION_WAIT`] for a turn, then gets `503` + `Retry-After`; nothing ran, so a retry is
//! the whole recovery. `DASHBOARD_MAX_CONCURRENT` sets the cap.
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rand::Rng;
use tokio::sync::Semaphore;

use crate::state::AppState;

/// How long a dashboard read waits for a turn before it is turned away.
const ADMISSION_WAIT: Duration = Duration::from_secs(5);

/// Bounds of the `Retry-After` a `503` asks for, jittered so a crowd turned away together does
/// not return together.
const OVERLOAD_RETRY_SECS: std::ops::RangeInclusive<u64> = 2..=5;

/// The dashboard reads' permits. One per process, held on `AppState`.
#[derive(Clone)]
pub struct DashboardLimit {
    permits: Arc<Semaphore>,
}

impl DashboardLimit {
    pub fn new(max_concurrent: u32) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(max_concurrent as usize)),
        }
    }
}

/// Route middleware: hold one permit for the whole request, waiting up to [`ADMISSION_WAIT`] for
/// it, or answer `503`.
pub(crate) async fn limit(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let permits = state.dashboard_limit.permits.clone();
    let Ok(Ok(_permit)) = tokio::time::timeout(ADMISSION_WAIT, permits.acquire_owned()).await
    else {
        let retry_after = rand::rng().random_range(OVERLOAD_RETRY_SECS);
        tracing::info!(path = %req.uri().path(), retry_after, "dashboard reads at capacity");
        let mut response = (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("too many dashboard requests at once; retry after {retry_after}s"),
        )
            .into_response();
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(retry_after));
        return response;
    };
    next.run(req).await
}
