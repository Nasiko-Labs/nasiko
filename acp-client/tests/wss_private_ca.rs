//! A `wss://` endpoint signed by a private CA is reachable only with that CA's PEM, and one that
//! requires a client certificate only with an identity the CA issued.

use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use nasiko_acp_client::sdk::schema::ProtocolVersion;
use nasiko_acp_client::sdk::schema::v1::InitializeResponse;
use nasiko_acp_client::{AcpConnectConfig, AcpError, ConnectFailure, TlsConfig, connect_ws};
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, ExtendedKeyUsagePurpose, IsCa, KeyPair,
};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::Message;

/// A throwaway CA, a server certificate and a client identity it issued.
struct Pki {
    ca_pem: String,
    server_chain: Vec<CertificateDer<'static>>,
    server_key: PrivateKeyDer<'static>,
    client_cert_pem: String,
    client_key_pem: String,
}

fn pki() -> Pki {
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = CertifiedIssuer::self_signed(ca_params, KeyPair::generate().unwrap()).unwrap();

    let server_key = KeyPair::generate().unwrap();
    let mut server = CertificateParams::new(vec!["127.0.0.1".to_owned()]).unwrap();
    server.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server_cert = server.signed_by(&server_key, &ca).unwrap();

    let client_key = KeyPair::generate().unwrap();
    let mut client = CertificateParams::new(vec!["client.test".to_owned()]).unwrap();
    client.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let client_cert = client.signed_by(&client_key, &ca).unwrap();

    Pki {
        ca_pem: ca.pem(),
        server_chain: vec![server_cert.der().clone()],
        server_key: PrivateKeyDer::try_from(server_key.serialize_der()).unwrap(),
        client_cert_pem: client_cert.pem(),
        client_key_pem: client_key.serialize_pem(),
    }
}

/// Serve TLS (requiring a client certificate from the CA when `require_client`); the server
/// completes the WebSocket upgrade and answers `initialize`. Returns the URL.
async fn spawn_tls_shim(pki: &Pki, require_client: bool) -> String {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .unwrap();
    let builder = if require_client {
        let mut roots = rustls::RootCertStore::empty();
        for cert in CertificateDer::pem_slice_iter(pki.ca_pem.as_bytes()) {
            roots.add(cert.unwrap()).unwrap();
        }
        let verifier =
            rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
                .build()
                .unwrap();
        builder.with_client_cert_verifier(verifier)
    } else {
        builder.with_no_client_auth()
    };
    let config = builder
        .with_single_cert(pki.server_chain.clone(), pki.server_key.clone_key())
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("wss://{}/acp", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let Ok(mut ws) = tokio_tungstenite::accept_async(tls).await else {
                    return;
                };
                while let Some(Ok(Message::Text(text))) = ws.next().await {
                    let req: serde_json::Value = serde_json::from_str(text.as_str()).unwrap();
                    if req["method"] == "initialize" {
                        let result =
                            serde_json::json!(InitializeResponse::new(ProtocolVersion::V1));
                        let reply = serde_json::json!({"jsonrpc": "2.0", "id": req["id"], "result": result});
                        let _ = ws.send(Message::text(reply.to_string())).await;
                    }
                }
            });
        }
    });
    url
}

/// Install the process-wide provider the default connector (no TLS config) relies on; the server
/// does the same at startup.
fn install_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn config_with(tls: TlsConfig) -> AcpConnectConfig {
    AcpConnectConfig {
        tls: Some(tls),
        handshake_timeout: std::time::Duration::from_secs(5),
        ..Default::default()
    }
}

fn is_tls(err: &AcpError) -> bool {
    matches!(
        err,
        AcpError::ConnectFailed {
            kind: ConnectFailure::Tls,
            ..
        }
    )
}

#[tokio::test]
async fn a_private_ca_is_refused_without_its_pem_and_trusted_with_it() {
    install_provider();
    let pki = pki();
    let url = spawn_tls_shim(&pki, false).await;

    let err = match connect_ws(&url, &[], AcpConnectConfig::default()).await {
        Ok(_) => panic!("connected without the CA"),
        Err(e) => e,
    };
    assert!(is_tls(&err), "{err:?}");

    let tls = TlsConfig {
        ca_pem: pki.ca_pem.clone(),
        client_cert_pem: None,
        client_key_pem: None,
    };
    if let Err(e) = connect_ws(&url, &[], config_with(tls)).await {
        panic!("connect with the CA failed: {e:?}");
    }
}

#[tokio::test]
async fn an_endpoint_requiring_a_client_certificate_needs_the_identity() {
    install_provider();
    let pki = pki();
    let url = spawn_tls_shim(&pki, true).await;

    let without = TlsConfig {
        ca_pem: pki.ca_pem.clone(),
        client_cert_pem: None,
        client_key_pem: None,
    };
    assert!(
        connect_ws(&url, &[], config_with(without)).await.is_err(),
        "connected without a client identity"
    );

    let with = TlsConfig {
        ca_pem: pki.ca_pem.clone(),
        client_cert_pem: Some(pki.client_cert_pem.clone()),
        client_key_pem: Some(pki.client_key_pem.clone()),
    };
    if let Err(e) = connect_ws(&url, &[], config_with(with)).await {
        panic!("connect with the identity failed: {e:?}");
    }
}

#[tokio::test]
async fn unusable_tls_material_is_an_invalid_request() {
    let bad = [
        TlsConfig {
            ca_pem: "not a certificate".into(),
            client_cert_pem: None,
            client_key_pem: None,
        },
        TlsConfig {
            ca_pem: pki().ca_pem,
            client_cert_pem: Some("not a certificate".into()),
            client_key_pem: Some("not a key".into()),
        },
    ];
    for tls in bad {
        let err = match connect_ws("wss://127.0.0.1:1/acp", &[], config_with(tls)).await {
            Ok(_) => panic!("connected"),
            Err(e) => e,
        };
        assert!(
            matches!(
                err,
                AcpError::ConnectFailed {
                    kind: ConnectFailure::InvalidRequest,
                    ..
                }
            ),
            "{err:?}"
        );
    }
}

#[tokio::test]
async fn an_empty_ca_with_an_identity_means_the_public_roots_alone() {
    let pki = pki();
    let tls = TlsConfig {
        ca_pem: String::new(),
        client_cert_pem: Some(pki.client_cert_pem),
        client_key_pem: Some(pki.client_key_pem),
    };
    // The material is accepted: the failure is the unreachable port, not an invalid request.
    let err = match connect_ws("wss://127.0.0.1:1/acp", &[], config_with(tls)).await {
        Ok(_) => panic!("connected"),
        Err(e) => e,
    };
    assert!(
        matches!(
            err,
            AcpError::ConnectFailed {
                kind: ConnectFailure::Unreachable,
                ..
            }
        ),
        "{err:?}"
    );
}
