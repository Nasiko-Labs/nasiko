//! The server's HTTP accept loop: `axum::serve`, plus a limit on idle connections.
//!
//! `axum::serve` keeps a kept-alive connection open for as long as its client does. Agents'
//! HTTP clients (`httpx` in the Python SDKs) keep theirs after every call and close them only
//! when they are next used, so with tens of thousands of agents each making a call every few
//! minutes, the server held one open connection per agent per client, idle. At 50,000 agents
//! that was 168,000 connections: about 6 GiB of server memory, and, through the internal load
//! balancer, three entries each in the node's connection-tracking table, which filled after 16
//! minutes at 50 turns a second. From then on every new connection on the node failed, the
//! server's own health probes included.
//!
//! hyper's HTTP/1 header-read timeout closes a connection that sends no request within
//! [`IDLE_CONNECTION_TIMEOUT`] of the last one finishing. It runs only while the connection
//! waits for a request: a response in flight (an SSE stream, a slow LLM call) is never cut, and
//! a client whose idle connection was closed opens a new one on its next call.

use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tower::Service;

/// How long a kept-alive connection may wait for its next request before the server closes it.
pub const IDLE_CONNECTION_TIMEOUT: Duration = Duration::from_secs(60);

/// A failed `accept` (out of file descriptors, say) is retried after this, not in a hot loop.
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(50);

/// Serve `app` on `listener` until the process ends, as `axum::serve` does with peer addresses
/// (`ConnectInfo<SocketAddr>`) and upgrades (WebSockets), closing connections idle for
/// [`IDLE_CONNECTION_TIMEOUT`].
pub async fn serve(listener: TcpListener, app: Router) {
    serve_with_idle_timeout(listener, app, IDLE_CONNECTION_TIMEOUT).await;
}

pub(crate) async fn serve_with_idle_timeout(
    listener: TcpListener,
    app: Router,
    idle_timeout: Duration,
) {
    let mut make_service = app.into_make_service_with_connect_info::<SocketAddr>();
    let mut builder = auto::Builder::new(TokioExecutor::new());
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(idle_timeout);
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(e) => {
                tracing::warn!(error = %e, "accepting a connection failed; retrying");
                tokio::time::sleep(ACCEPT_RETRY_DELAY).await;
                continue;
            }
        };
        let service = match make_service.call(peer).await {
            Ok(service) => service,
            Err(never) => match never {},
        };
        let builder = builder.clone();
        tokio::spawn(async move {
            let served = builder
                .serve_connection_with_upgrades(
                    TokioIo::new(stream),
                    TowerToHyperService::new(service),
                )
                .await;
            if let Err(e) = served {
                tracing::debug!(error = %e, %peer, "connection ended with an error");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::routing::get;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    const IDLE: Duration = Duration::from_millis(300);

    async fn start() -> SocketAddr {
        start_with(IDLE).await
    }

    async fn start_with(idle_timeout: Duration) -> SocketAddr {
        let app =
            Router::new()
                .route("/ping", get(|| async { "pong" }))
                .route(
                    "/peer",
                    get(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move {
                        peer.ip().to_string()
                    }),
                )
                .route(
                    "/slow",
                    get(|| async {
                        tokio::time::sleep(IDLE * 3).await;
                        "done"
                    }),
                )
                .route(
                    "/stream",
                    get(|| async {
                        let chunks = futures::stream::unfold(0, |n| async move {
                            if n == 3 {
                                return None;
                            }
                            tokio::time::sleep(IDLE * 2).await;
                            Some((Ok::<_, std::io::Error>(format!("data: {n}\n\n")), n + 1))
                        });
                        Body::from_stream(chunks)
                    }),
                );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_with_idle_timeout(listener, app, idle_timeout));
        addr
    }

    /// One request on a kept-alive connection, read up to the end of its body.
    async fn request(conn: &mut TcpStream, path: &str) -> String {
        conn.write_all(format!("GET {path} HTTP/1.1\r\nHost: test\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut response = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = conn.read(&mut buf).await.unwrap();
            assert!(
                n > 0,
                "connection closed mid-response: {}",
                String::from_utf8_lossy(&response)
            );
            response.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&response);
            if text.ends_with("0\r\n\r\n")
                || text.contains("content-length")
                    && text.split("\r\n\r\n").nth(1).is_some_and(|b| !b.is_empty())
            {
                return text.into_owned();
            }
        }
    }

    #[tokio::test]
    async fn an_idle_kept_alive_connection_is_closed() {
        let addr = start().await;
        let mut conn = TcpStream::connect(addr).await.unwrap();
        assert!(request(&mut conn, "/ping").await.ends_with("pong"));
        tokio::time::sleep(IDLE * 3).await;
        let mut buf = [0u8; 16];
        let read = tokio::time::timeout(IDLE * 3, conn.read(&mut buf)).await;
        assert!(
            matches!(read, Ok(Ok(0)) | Ok(Err(_))),
            "the server should have closed the idle connection: {read:?}"
        );
    }

    /// The control for the test above: with a long limit the same wait leaves the connection
    /// usable, so it was the limit that closed it.
    #[tokio::test]
    async fn an_idle_connection_under_the_limit_stays_open() {
        let addr = start_with(Duration::from_secs(60)).await;
        let mut conn = TcpStream::connect(addr).await.unwrap();
        assert!(request(&mut conn, "/ping").await.ends_with("pong"));
        tokio::time::sleep(IDLE * 3).await;
        assert!(request(&mut conn, "/ping").await.ends_with("pong"));
    }

    #[tokio::test]
    async fn a_connection_in_use_stays_open_across_requests() {
        let addr = start().await;
        let mut conn = TcpStream::connect(addr).await.unwrap();
        for _ in 0..3 {
            assert!(request(&mut conn, "/ping").await.ends_with("pong"));
            tokio::time::sleep(IDLE / 3).await;
        }
    }

    #[tokio::test]
    async fn a_slow_response_is_not_cut() {
        let addr = start().await;
        let mut conn = TcpStream::connect(addr).await.unwrap();
        assert!(request(&mut conn, "/slow").await.ends_with("done"));
    }

    #[tokio::test]
    async fn a_stream_with_long_gaps_is_not_cut() {
        let addr = start().await;
        let mut conn = TcpStream::connect(addr).await.unwrap();
        let body = request(&mut conn, "/stream").await;
        for n in 0..3 {
            assert!(body.contains(&format!("data: {n}")), "{body}");
        }
    }

    #[tokio::test]
    async fn handlers_see_the_peer_address() {
        let addr = start().await;
        let mut conn = TcpStream::connect(addr).await.unwrap();
        assert!(request(&mut conn, "/peer").await.ends_with("127.0.0.1"));
    }
}
