//! The HTTP client the server calls agents with: the agent proxy, A2A dispatch, the
//! orchestrator's agent tool calls, MAF steps and HITL resume deliveries.

use std::time::Duration;

/// Builds the client for server-to-agent calls.
///
/// It keeps no idle connections: each request opens its own connection, so a load
/// balancer in front of several agent replicas can spread requests across them. A
/// pooled connection would carry every later request to the replica it first
/// reached, and a replica added under load would get almost none of it.
///
/// `timeout` applies to a call that sets none of its own; most agent calls set their
/// own per request.
///
/// It follows no redirects: an agent's answer, 3xx included, goes back as it is. A
/// member can register an agent by URL, and only that URL is checked
/// (`catalog::routes::agent_url_to_store`); following a `302` from it to
/// `http://169.254.169.254/...` would hand the cloud metadata server's answer to
/// the member, as the MCP gateway's guarded client once did (its redirect policy,
/// `nasiko_mcp_gateway::net`).
pub fn build(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("failed to build the agent http client")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    const RESPONSE: &[u8] =
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\nok";

    /// A local HTTP/1.1 listener that answers every request on a connection with a
    /// keep-alive response, and counts the connections it accepted.
    async fn counting_listener() -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = accepted.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                counter.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    // One read per request: the test's requests are tiny GETs.
                    while matches!(socket.read(&mut buf).await, Ok(n) if n > 0) {
                        if socket.write_all(RESPONSE).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        (url, accepted)
    }

    async fn get_twice(client: &reqwest::Client, url: &str) {
        for _ in 0..2 {
            let body = client.get(url).send().await.unwrap().text().await.unwrap();
            assert_eq!(body, "ok");
        }
    }

    #[tokio::test]
    async fn every_agent_request_opens_its_own_connection() {
        let (url, accepted) = counting_listener().await;
        get_twice(&build(Duration::from_secs(5)), &url).await;
        assert_eq!(accepted.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_redirect_from_an_agent_is_returned_not_followed() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = socket.read(&mut buf).await;
                    let reply = b"HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/computeMetadata/v1/\r\nContent-Length: 0\r\n\r\n";
                    let _ = socket.write_all(reply).await;
                });
            }
        });
        let response = build(Duration::from_secs(5))
            .post(&url)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
    }

    /// The control: the same listener sees a pooled client reuse one connection, so
    /// the count above is the client's doing, not the listener's.
    #[tokio::test]
    async fn a_pooled_client_reuses_its_connection() {
        let (url, accepted) = counting_listener().await;
        get_twice(&reqwest::Client::new(), &url).await;
        assert_eq!(accepted.load(Ordering::SeqCst), 1);
    }
}
