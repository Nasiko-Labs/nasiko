//! The server's own load: how many HTTP requests each replica is serving right now, exported as
//! `nasiko.server.http.inflight` (an UpDownCounter; `nasiko_server_http_inflight` in Prometheus,
//! one series per replica through `service.instance.id`). The server's autoscaler reads it beside
//! CPU: most of a turn is spent waiting on a model or an agent, so a replica can be full of open
//! streams long before its CPU shows it.
//!
//! A request counts from when it arrives until its response body ends, not when the handler
//! returns: a chat turn's SSE stream is served for the whole turn, and counting it only until the
//! headers went out would read every streaming replica as idle.

use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll};

use axum::body::{Body, Bytes, HttpBody};
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use opentelemetry::global;
use opentelemetry::metrics::UpDownCounter;

fn inflight() -> &'static UpDownCounter<i64> {
    static INFLIGHT: OnceLock<UpDownCounter<i64>> = OnceLock::new();
    INFLIGHT.get_or_init(|| {
        global::meter("nasiko.cp")
            .i64_up_down_counter("nasiko.server.http.inflight")
            .with_description(
                "HTTP requests this replica is serving, until each response body ends",
            )
            .build()
    })
}

/// Counts one request in, and out again when dropped.
struct InFlight;

impl InFlight {
    fn start() -> Self {
        inflight().add(1, &[]);
        Self
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        inflight().add(-1, &[]);
    }
}

/// Middleware: count the request while it is served.
pub async fn track(req: Request, next: Next) -> Response {
    let guard = InFlight::start();
    let response = next.run(req).await;
    let (parts, body) = response.into_parts();
    Response::from_parts(
        parts,
        Body::new(CountedBody {
            inner: body,
            _guard: guard,
        }),
    )
}

/// A response body that holds its request's count until it is finished or dropped.
struct CountedBody {
    inner: Body,
    _guard: InFlight,
}

impl HttpBody for CountedBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::routing::get;
    use tower::ServiceExt;

    #[tokio::test]
    async fn the_body_is_passed_through_with_its_length() {
        let app = Router::new()
            .route("/", get(|| async { "hello" }))
            .layer(axum::middleware::from_fn(track));
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.body().size_hint().exact(), Some(5));
        let bytes = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"hello");
    }
}
