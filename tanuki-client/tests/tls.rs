#![cfg(not(target_arch = "wasm32"))]
use tanuki_client::{Connection, ConnectionError};
use tokio_tungstenite::tungstenite::{Error, error::UrlError};

/// A `wss://` URL must reach the TLS handshake. The peer here accepts TCP and
/// hangs up, so the handshake fails. That failure must be an ordinary transport
/// error: neither "TLS support not compiled in" nor a missing-CryptoProvider
/// panic (the SDK installs ring when the application has not chosen one).
#[tokio::test]
async fn wss_urls_reach_the_tls_handshake() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        drop(socket);
    });
    let Err(error) = Connection::open(&format!("wss://{address}/v1/ws")).await else {
        panic!("the peer never completes a handshake");
    };
    assert!(
        !matches!(
            error,
            ConnectionError::Transport(Error::Url(UrlError::TlsFeatureNotEnabled))
        ),
        "TLS support is missing: {error}"
    );
    assert!(
        matches!(
            error,
            ConnectionError::Transport(Error::Io(_) | Error::Tls(_))
        ),
        "expected a handshake failure, got {error:?}"
    );
    server.await.unwrap();
}
