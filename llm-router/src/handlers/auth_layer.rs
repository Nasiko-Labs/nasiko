use std::sync::Arc;
use std::time::Duration;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Request, State};
use axum::http::header::{CONTENT_ENCODING, CONTENT_LENGTH};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::chat::authenticate_caller;
use crate::LlmRouterCtx;
use crate::error::GatewayError;
use crate::resolver::PgRegistry;

pub(crate) const PERMIT_BYTES: usize = 1024 * 1024;
const BODY_PERMIT_WAIT: Duration = Duration::from_secs(30);

pub(crate) fn body_permit_pool(max_inflight_body_bytes: usize) -> Arc<Semaphore> {
    Arc::new(Semaphore::new(
        (max_inflight_body_bytes / PERMIT_BYTES).max(1),
    ))
}

pub(crate) fn permits_for(headers: &HeaderMap, max_body_bytes: usize, pool_size: usize) -> u32 {
    let declared = headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    let bytes = match (declared, headers.contains_key(CONTENT_ENCODING)) {
        (Some(len), false) => len.min(max_body_bytes),
        _ => max_body_bytes,
    };
    bytes.div_ceil(PERMIT_BYTES).clamp(1, pool_size.max(1)) as u32
}

pub(crate) async fn reserve(
    pool: Arc<Semaphore>,
    permits: u32,
    wait: Duration,
) -> Result<OwnedSemaphorePermit, GatewayError> {
    match tokio::time::timeout(wait, pool.acquire_many_owned(permits)).await {
        Ok(Ok(permit)) => Ok(permit),
        _ => Err(GatewayError::Busy),
    }
}

pub async fn authenticate_first(
    State(ctx): State<LlmRouterCtx>,
    mut req: Request,
    next: Next,
) -> Response {
    let render = renderer_for(req.uri().path());
    let store = PgRegistry::new(ctx.db.clone());
    let caller = match authenticate_caller(&ctx, &store, req.headers()).await {
        Ok(caller) => caller,
        Err(e) => return render(e),
    };
    let permits = permits_for(
        req.headers(),
        ctx.cfg.max_body_bytes,
        ctx.cfg.max_inflight_body_bytes / PERMIT_BYTES,
    );
    let _permit = match reserve(ctx.body_permits.clone(), permits, BODY_PERMIT_WAIT).await {
        Ok(permit) => permit,
        Err(e) => return render(e),
    };
    req.extensions_mut().insert(caller);
    next.run(req).await
}

fn renderer_for(path: &str) -> fn(GatewayError) -> Response {
    if path == "/v1/responses" {
        super::responses::gateway_error_response
    } else {
        IntoResponse::into_response
    }
}

pub(crate) fn rejected_body(
    rejection: JsonRejection,
    max_body_bytes: usize,
    render: fn(GatewayError) -> Response,
) -> Response {
    if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return render(GatewayError::PayloadTooLarge {
            limit: max_body_bytes,
        });
    }
    rejection.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (name, value) in pairs {
            h.insert(*name, value.parse().unwrap());
        }
        h
    }

    #[test]
    fn a_request_reserves_its_declared_size_in_whole_mebibytes() {
        let max = 48 * PERMIT_BYTES;
        assert_eq!(
            permits_for(&headers(&[("content-length", "10")]), max, 256),
            1
        );
        assert_eq!(
            permits_for(&headers(&[("content-length", "3145729")]), max, 256),
            4
        );
    }

    #[test]
    fn an_unknown_or_compressed_size_reserves_the_full_limit() {
        let max = 48 * PERMIT_BYTES;
        assert_eq!(permits_for(&HeaderMap::new(), max, 256), 48);
        assert_eq!(
            permits_for(
                &headers(&[("content-length", "100"), ("content-encoding", "zstd")]),
                max,
                256
            ),
            48
        );
        assert_eq!(
            permits_for(&headers(&[("content-length", "999999999999")]), max, 256),
            48
        );
    }

    #[test]
    fn a_reservation_never_asks_for_more_than_the_whole_pool() {
        assert_eq!(permits_for(&HeaderMap::new(), 48 * PERMIT_BYTES, 8), 8);
    }

    #[tokio::test]
    async fn a_full_pool_answers_busy_after_the_wait() {
        let pool = body_permit_pool(2 * PERMIT_BYTES);
        let _held = reserve(pool.clone(), 2, Duration::from_millis(10))
            .await
            .unwrap();
        let err = reserve(pool.clone(), 1, Duration::from_millis(20))
            .await
            .unwrap_err();
        assert!(matches!(err, GatewayError::Busy));
        drop(_held);
        assert!(reserve(pool, 2, Duration::from_millis(20)).await.is_ok());
    }
}
