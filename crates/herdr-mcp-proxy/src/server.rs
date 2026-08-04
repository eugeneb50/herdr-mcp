use crate::ca::CaManager;
use anyhow::Result;
use herdr_mcp_trim::runner::PipelineRunner;
use std::path::Path;
use std::sync::Arc;

pub use crate::interceptor::ProxyPolicy;

pub async fn run_proxy_listener(
    port: u16,
    bind: &str,
    data_dir: &Path,
    target_hosts: Vec<String>,
    _default_policy: ProxyPolicy,
) -> Result<()> {
    let ca = Arc::new(CaManager::load_or_create(&data_dir.join("proxy"), 3650)?);
    let _runner = Arc::new(PipelineRunner::new(data_dir).await);
    let _tls_config = Arc::new(crate::tls::make_server_config(
        ca.clone(),
        target_hosts.clone(),
    )?);

    let addr = format!("{bind}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(
        "proxy listener on {addr} ({} target hosts)",
        target_hosts.len()
    );

    loop {
        let (_stream, _peer) = listener.accept().await?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls::make_server_config;
    use rustls::pki_types::{CertificateDer, ServerName};
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    #[test]
    fn test_proxy_policy_default() {
        let p = ProxyPolicy::default();
        assert!(!p.trim_outbound);
        assert!(!p.trim_inbound);
    }

    /// End-to-end handshake test: spin up a minimal TLS server using the
    /// proxy's CA + SNI resolver, then connect as a client that trusts the CA.
    /// Verifies the dynamically-signed leaf cert chains back to our root.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_e2e_tls_handshake_chains_to_ca() {
        let dir = tempdir().unwrap();
        let ca = Arc::new(CaManager::load_or_create(dir.path(), 3650).unwrap());
        let allowed_host = "api.openai.com".to_string();
        let server_config = Arc::new(
            make_server_config(ca.clone(), vec![allowed_host.clone()]).unwrap(),
        );

        // Bind ephemeral port and start a minimal TLS-accepting proxy.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_cfg = server_config.clone();
        let server_task = tokio::spawn(async move {
            let (stream, _peer) = listener.accept().await.unwrap();
            let acceptor = TlsAcceptor::from(server_cfg);
            let _tls = acceptor.accept(stream).await.unwrap();
            // Handshake succeeded; client has verified our cert chain.
        });

        // Build a client TLS config that trusts our CA.
        let mut root_store = rustls::RootCertStore::empty();
        root_store.add(CertificateDer::from(ca.ca_cert_der().to_vec())).unwrap();
        let client_config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(client_config));
        let server_name = ServerName::try_from(allowed_host.clone()).unwrap();

        // Connect and complete TLS handshake.
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let tls = tokio::time::timeout(
            Duration::from_secs(5),
            connector.connect(server_name, tcp),
        )
        .await
        .expect("TLS handshake timed out")
        .expect("TLS handshake failed");

        // Verify a peer cert was presented.
        let (_io, conn) = tls.into_inner();
        let peer = conn
            .peer_certificates()
            .expect("peer certs should be present");
        assert!(!peer.is_empty(), "proxy must present a leaf cert");

        // Wait for server to finish.
        tokio::time::timeout(Duration::from_secs(2), server_task)
            .await
            .expect("server task timed out")
            .expect("server task panicked");

        // Cleanup: write a byte and close cleanly.
        drop(conn);
    }

    /// Handshake should fail when the client uses an SNI host that is NOT in
    /// the allowed `target_hosts`. The resolver returns None → rustls handles
    /// the failure.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_e2e_tls_disallowed_sni_fails() {
        let dir = tempdir().unwrap();
        let ca = Arc::new(CaManager::load_or_create(dir.path(), 3650).unwrap());
        let server_config = Arc::new(
            make_server_config(ca.clone(), vec!["api.openai.com".into()]).unwrap(),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_cfg = server_config.clone();
        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let acceptor = TlsAcceptor::from(server_cfg);
            // Server-side accept likely fails OR succeeds without a usable
            // cert; either way we want to confirm the client fails.
            let _ = acceptor.accept(stream).await;
        });

        let mut root_store = rustls::RootCertStore::empty();
        root_store.add(CertificateDer::from(ca.ca_cert_der().to_vec())).unwrap();
        let client_config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(client_config));
        let server_name = ServerName::try_from("evil.com".to_string()).unwrap();

        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            connector.connect(server_name, tcp),
        )
        .await;

        // Either a timeout or an Err is acceptable; the strict assertion is
        // that we did NOT get a clean handshake back.
        match result {
            Ok(Ok(_)) => panic!("handshake to disallowed host should fail"),
            Ok(Err(_)) | Err(_) => {}
        }
        let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
    }
}
