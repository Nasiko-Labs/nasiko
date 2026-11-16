//! A request as a Sentry transaction, so a slow page in the UI can be followed into the server.

use std::sync::Arc;

use axum::extract::{MatchedPath, Request};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use sentry::protocol::SpanStatus;
use sentry::{Hub, SentryFutureExt, TransactionContext};

use super::scrub;

/// Routes never recorded: probes and the tunnel itself, which would only trace the reporting.
const UNTRACED_ROUTES: &[&str] = &["/health", "/api/telemetry/sentry", "/api/telemetry/config"];

/// Middleware: give the request its own Sentry scope, and record it as a transaction named
/// `<METHOD> <route template>`. Layered on the whole app with `Router::layer`, so it runs after
/// routing and the template is known. The scope is what keeps per-request context (the user
/// [`super::identify`] names) on this request's events only, so every routed request gets one,
/// probes included; only the transaction is skipped for [`UNTRACED_ROUTES`]. A request no route
/// matched (UI pages and assets, which the browser traces itself) is passed through, as is
/// everything when nothing is reported.
pub async fn trace_requests(req: Request, next: Next) -> Response {
    let Some(route) = req
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_owned())
    else {
        return next.run(req).await;
    };
    if Hub::current().client().is_none() {
        return next.run(req).await;
    }

    let hub = Arc::new(Hub::new_from_top(Hub::current()));
    if UNTRACED_ROUTES.contains(&route.as_str()) {
        return next.run(req).bind_hub(hub).await;
    }
    let request = scrub::from_http(&req);
    let headers = req
        .headers()
        .iter()
        .filter_map(|(name, value)| Some((name.as_str(), value.to_str().ok()?)));
    let context = TransactionContext::continue_from_headers(
        &format!("{} {route}", req.method()),
        "http.server",
        headers,
    );
    let transaction = hub.start_transaction(context);
    transaction.set_request(request.clone());
    hub.configure_scope(|scope| {
        scope.set_span(Some(transaction.clone().into()));
        scope.add_event_processor(move |mut event| {
            event.request.get_or_insert_with(|| request.clone());
            Some(event)
        });
    });

    let response = next.run(req).bind_hub(hub).await;
    transaction.set_status(span_status(response.status()));
    transaction.finish();
    response
}

fn span_status(status: StatusCode) -> SpanStatus {
    match status.as_u16() {
        ..400 => SpanStatus::Ok,
        401 => SpanStatus::Unauthenticated,
        403 => SpanStatus::PermissionDenied,
        404 => SpanStatus::NotFound,
        409 => SpanStatus::AlreadyExists,
        429 => SpanStatus::ResourceExhausted,
        400..500 => SpanStatus::InvalidArgument,
        503 => SpanStatus::Unavailable,
        504 => SpanStatus::DeadlineExceeded,
        _ => SpanStatus::InternalError,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_errors_and_server_errors_map_to_distinct_statuses() {
        assert_eq!(span_status(StatusCode::OK), SpanStatus::Ok);
        assert_eq!(span_status(StatusCode::NOT_MODIFIED), SpanStatus::Ok);
        assert_eq!(
            span_status(StatusCode::UNPROCESSABLE_ENTITY),
            SpanStatus::InvalidArgument
        );
        assert_eq!(
            span_status(StatusCode::TOO_MANY_REQUESTS),
            SpanStatus::ResourceExhausted
        );
        assert_eq!(
            span_status(StatusCode::BAD_GATEWAY),
            SpanStatus::InternalError
        );
        assert_eq!(
            span_status(StatusCode::GATEWAY_TIMEOUT),
            SpanStatus::DeadlineExceeded
        );
    }
}
