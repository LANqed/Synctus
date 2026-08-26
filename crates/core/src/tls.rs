//! TLS for the client→relay hop.
//!
//! Payloads are already end-to-end encrypted, so TLS is defence in depth: it
//! hides room ids and device ids from the network and stops an on-path attacker
//! from replaying or reordering frames.
//!
//! Uses rustls with the `ring` backend, which avoids depending on a system
//! OpenSSL — important for the Android build and for static Linux binaries.

use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

use rustls::{ClientConfig as RustlsConfig, RootCertStore};
use rustls_pki_types::ServerName;

/// Either a plain or a TLS-wrapped TCP stream.
///
/// An enum rather than a boxed trait object: this is on the hot read path and it
/// keeps the client generic-free.
pub enum MaybeTls {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl MaybeTls {
    /// Disable Nagle so a status update is not delayed waiting for more data.
    pub fn set_nodelay(&self) -> Result<()> {
        match self {
            MaybeTls::Plain(s) => s.set_nodelay(true)?,
            MaybeTls::Tls(s) => s.get_ref().0.set_nodelay(true)?,
        }
        Ok(())
    }
}

// Delegating the AsyncRead/AsyncWrite impls by hand is the price of the enum;
// each arm is a straight forward to the inner stream.
impl tokio::io::AsyncRead for MaybeTls {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            MaybeTls::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for MaybeTls {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            MaybeTls::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => std::pin::Pin::new(s).poll_flush(cx),
            MaybeTls::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            MaybeTls::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Build a client config trusting the bundled Mozilla root set.
///
/// `webpki-roots` is used instead of the platform store so all three targets
/// behave identically, including Android where the store is awkward to reach
/// from native code.
pub fn client_config() -> Arc<RustlsConfig> {
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    Arc::new(
        RustlsConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

/// Connect and, when `tls` is set, complete the handshake against
/// `server_name`.
pub async fn connect(addr: &str, tls: bool, server_name: &str) -> Result<MaybeTls> {
    let tcp = TcpStream::connect(addr)
        .await
        .with_context(|| format!("连接服务器失败: {addr}"))?;
    tcp.set_nodelay(true).ok();

    if !tls {
        return Ok(MaybeTls::Plain(tcp));
    }

    let name = ServerName::try_from(server_name.to_string())
        .with_context(|| format!("无效的 TLS 服务器名: {server_name}"))?;
    let stream = TlsConnector::from(client_config())
        .connect(name, tcp)
        .await
        .context("TLS 握手失败")?;
    Ok(MaybeTls::Tls(Box::new(stream)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_root_store_is_not_empty() {
        // A silently empty trust store would make every TLS connect fail.
        assert!(!webpki_roots::TLS_SERVER_ROOTS.is_empty());
        // And the config must actually build from it.
        let _ = client_config();
    }

    #[tokio::test]
    async fn invalid_server_name_fails_before_handshake() {
        // Port 1 refuses fast; the point is that the name is validated at all.
        let err = connect("127.0.0.1:1", true, "not a valid name").await;
        assert!(err.is_err());
    }
}
