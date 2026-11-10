//! WebSocket transport: reach a harness shim at `ws://<host>:7000/acp`.
//!
//! The shim exposes the harness's stdio as one newline-terminated JSON-RPC frame
//! per WebSocket **Text** message in each direction. The SDK wants a byte stream,
//! so two in-memory pipes and two small pump tasks translate between the two
//! framings; the SDK-facing halves go through `tokio_util::compat` into
//! `ByteStreams`.
//!
//! A fresh [`connect_ws`] after a sandbox resume sends `initialize` to a harness
//! that is already initialised. The harness-shim absorbs repeat `initialize`
//! requests by replaying its cached first response, so this client stays stateless.

use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio_tungstenite::Connector;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue, Request};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::connection::{AcpConnectConfig, AcpConnection, TlsConfig};
use crate::error::{AcpError, ConnectFailure};

/// Capacity of each in-memory pipe between the SDK and a pump task. Only a
/// window: the SDK drains into an unbounded channel, so a frame larger than this
/// streams through without back-pressure deadlock.
const PIPE_BYTES: usize = 64 * 1024;

/// Must equal the harness-shim's `MAX_FRAME_BYTES` (oss/harness-shim/src/child.rs):
/// a frame the shim forwards must be one the client accepts, or the session dies
/// on a large tool result. Kept as two cross-referenced constants because this
/// crate must not depend on the shim binary.
const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

/// Connect to a harness shim over WebSocket and complete the ACP handshake.
///
/// `headers` are added to the upgrade request (e.g. `Authorization`). There is no
/// reconnect: the shim buffers frames while no client is attached, so the caller
/// opens a fresh connection after a sandbox resumes.
pub async fn connect_ws(
    url: &str,
    headers: &[(String, String)],
    config: AcpConnectConfig,
) -> Result<AcpConnection, AcpError> {
    let request = build_request(url, headers)?;
    let ws_config = WebSocketConfig::default().max_message_size(Some(MAX_MESSAGE_BYTES));
    let connector = tls_connector(url, config.tls.as_ref())?;
    let (socket, _response) = tokio_tungstenite::connect_async_tls_with_config(
        request,
        Some(ws_config),
        false,
        connector,
    )
    .await
    .map_err(connect_failure)?;
    let (sink, stream) = socket.split();

    // ws -> SDK: the pump writes `inbound_tx`, the SDK reads `inbound_rx`.
    let (inbound_tx, inbound_rx) = tokio::io::duplex(PIPE_BYTES);
    // SDK -> ws: the SDK writes `outbound_tx`, the pump reads `outbound_rx`.
    let (outbound_tx, outbound_rx) = tokio::io::duplex(PIPE_BYTES);

    let reader = tokio::spawn(pump_ws_to_pipe(stream, inbound_tx));
    // Not aborted by us: it ends on pipe EOF and sends the WebSocket Close.
    tokio::spawn(pump_pipe_to_ws(outbound_rx, sink));

    let transport =
        agent_client_protocol::ByteStreams::new(outbound_tx.compat_write(), inbound_rx.compat());
    match AcpConnection::connect(transport, config).await {
        Ok(conn) => Ok(conn.with_reader_guard(reader)),
        Err(e) => {
            reader.abort();
            Err(e)
        }
    }
}

/// A TLS connector trusting the public roots plus the CA given, presenting the client identity
/// when there is one; `None` (the library's default: public roots, no identity) for a plain
/// `ws://` URL or when no TLS material is given.
fn tls_connector(url: &str, tls: Option<&TlsConfig>) -> Result<Option<Connector>, AcpError> {
    let Some(tls) = tls.filter(|_| url.starts_with("wss://")) else {
        return Ok(None);
    };
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    // An empty CA is "the public roots only" (an endpoint that only wants a client identity).
    let cas = if tls.ca_pem.trim().is_empty() {
        Vec::new()
    } else {
        parse_certs(&tls.ca_pem, "CA")?
    };
    for cert in cas {
        roots
            .add(cert)
            .map_err(|e| invalid(format!("unusable CA certificate: {e}")))?;
    }
    let builder = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| invalid(format!("TLS setup failed: {e}")))?
    .with_root_certificates(roots);
    let config = match (&tls.client_cert_pem, &tls.client_key_pem) {
        (Some(cert), Some(key)) => {
            let key = PrivateKeyDer::from_pem_slice(key.as_bytes())
                .map_err(|e| invalid(format!("invalid client key PEM: {e}")))?;
            builder
                .with_client_auth_cert(parse_certs(cert, "client")?, key)
                .map_err(|e| invalid(format!("unusable client identity: {e}")))?
        }
        (None, None) => builder.with_no_client_auth(),
        _ => {
            return Err(invalid(
                "a client certificate needs its key, and the reverse".into(),
            ));
        }
    };
    Ok(Some(Connector::Rustls(Arc::new(config))))
}

/// The certificates in `pem`; at least one is required.
fn parse_certs(pem: &str, what: &str) -> Result<Vec<CertificateDer<'static>>, AcpError> {
    let certs = CertificateDer::pem_slice_iter(pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| invalid(format!("invalid {what} certificate PEM: {e}")))?;
    if certs.is_empty() {
        return Err(invalid(format!("the {what} PEM holds no certificate")));
    }
    Ok(certs)
}

fn invalid(message: String) -> AcpError {
    AcpError::ConnectFailed {
        kind: ConnectFailure::InvalidRequest,
        message,
    }
}

fn connect_failure(error: WsError) -> AcpError {
    let kind = match &error {
        // A certificate the client does not trust surfaces as an I/O error wrapping rustls's; it
        // is not a shim that is still coming up, so it must not be retried.
        WsError::Io(e) if e.get_ref().is_some_and(|inner| inner.is::<rustls::Error>()) => {
            ConnectFailure::Tls
        }
        WsError::Io(_) => ConnectFailure::Unreachable,
        WsError::Tls(_) => ConnectFailure::Tls,
        WsError::Http(response) => ConnectFailure::Rejected {
            status: response.status().as_u16(),
        },
        _ => ConnectFailure::InvalidRequest,
    };
    AcpError::ConnectFailed {
        kind,
        message: error.to_string(),
    }
}

fn build_request(url: &str, headers: &[(String, String)]) -> Result<Request<()>, AcpError> {
    let mut request = url
        .into_client_request()
        .map_err(|e| invalid(format!("invalid websocket url {url:?}: {e}")))?;
    for (name, value) in headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|e| invalid(format!("invalid header name {name:?}: {e}")))?;
        let value = HeaderValue::from_str(value)
            .map_err(|e| invalid(format!("invalid value for header {name}: {e}")))?;
        request.headers_mut().append(name, value);
    }
    Ok(request)
}

/// Copy each Text message into the pipe as one newline-terminated line. Ends by
/// shutting the pipe, which the SDK sees as EOF.
async fn pump_ws_to_pipe<S, W>(mut stream: S, mut pipe: W)
where
    S: futures::Stream<Item = Result<Message, WsError>> + Unpin,
    W: AsyncWrite + Unpin,
{
    while let Some(next) = stream.next().await {
        let outcome = match next {
            Ok(Message::Text(text)) => write_line(&mut pipe, text.as_str()).await,
            Ok(Message::Binary(_)) => {
                Err("harness shim sent a binary frame; only text is allowed".to_owned())
            }
            Ok(Message::Close(frame)) => {
                // TODO NAS-1102: surface 1012 (takeover) vs 1011 (child exited) to the caller.
                let (code, reason) = frame.map_or((None, String::new()), |f| {
                    (Some(u16::from(f.code)), f.reason.to_string())
                });
                tracing::info!(?code, %reason, "acp websocket closed by shim");
                break;
            }
            // Ping/Pong are answered by tungstenite itself.
            Ok(_) => Ok(()),
            Err(e) => Err(format!("websocket read failed: {e}")),
        };
        if let Err(reason) = outcome {
            tracing::warn!(%reason, "acp websocket reader stopping");
            break;
        }
    }
    let _ = pipe.shutdown().await;
}

async fn write_line<W: AsyncWrite + Unpin>(pipe: &mut W, text: &str) -> Result<(), String> {
    pipe.write_all(text.as_bytes())
        .await
        .map_err(|e| format!("pipe write failed: {e}"))?;
    if !text.ends_with('\n') {
        pipe.write_all(b"\n")
            .await
            .map_err(|e| format!("pipe write failed: {e}"))?;
    }
    Ok(())
}

/// Send each line the SDK writes as one Text message. Closes the socket when the
/// SDK side goes away or a send fails.
async fn pump_pipe_to_ws<R, K>(pipe: R, mut sink: K)
where
    R: AsyncRead + Unpin,
    K: futures::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let mut lines = BufReader::new(pipe).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if let Err(e) = sink.send(Message::text(line)).await {
                    tracing::warn!(error = %e, "acp websocket writer stopping");
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(error = %e, "acp websocket writer: pipe read failed");
                break;
            }
        }
    }
    let _ = sink.close().await;
}
