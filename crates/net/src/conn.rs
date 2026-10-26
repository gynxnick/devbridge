//! TCP + TLS connection setup.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::ServerName;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};

use crate::identity::{Identity, fingerprint};
use crate::tls;

pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Concurrent in-progress handshakes (limits resource use by unpaired peers).
const MAX_PENDING_HANDSHAKES: usize = 16;

pub struct TlsConn {
    pub stream: TlsStream<TcpStream>,
    /// SHA-256 fingerprint of the certificate the peer presented.
    pub peer_fingerprint: String,
    pub peer_addr: SocketAddr,
    /// True if we dialed (TLS client).
    pub outbound: bool,
}

fn other(e: impl ToString) -> io::Error {
    io::Error::other(e.to_string())
}

pub struct Listener {
    tcp: TcpListener,
    acceptor: TlsAcceptor,
}

impl Listener {
    pub async fn bind(addr: SocketAddr, id: &Identity) -> io::Result<Self> {
        let tcp = TcpListener::bind(addr).await?;
        let acceptor = TlsAcceptor::from(tls::server_config(id).map_err(other)?);
        Ok(Self { tcp, acceptor })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.tcp.local_addr()
    }

    /// Accepts connections forever, handshaking each on its own task and
    /// sending completed connections to `tx`. Ends when `tx` is closed.
    pub async fn run(self, tx: mpsc::Sender<TlsConn>) {
        let sem = Arc::new(Semaphore::new(MAX_PENDING_HANDSHAKES));
        loop {
            let (tcp, addr) = match self.tcp.accept().await {
                Ok(x) => x,
                Err(e) => {
                    tracing::warn!(error = %e, "accept failed");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
            };
            if tx.is_closed() {
                return;
            }
            let Ok(permit) = sem.clone().try_acquire_owned() else {
                tracing::warn!(%addr, "too many pending handshakes; dropping connection");
                continue;
            };
            let acceptor = self.acceptor.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let _ = tcp.set_nodelay(true);
                match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await {
                    Ok(Ok(stream)) => {
                        let fp = stream.get_ref().1.peer_certificates().and_then(|c| c.first()).map(|c| fingerprint(c));
                        if let Some(fp) = fp {
                            let _ = tx
                                .send(TlsConn {
                                    stream: TlsStream::Server(stream),
                                    peer_fingerprint: fp,
                                    peer_addr: addr,
                                    outbound: false,
                                })
                                .await;
                        }
                    }
                    Ok(Err(e)) => tracing::debug!(%addr, error = %e, "TLS handshake failed"),
                    Err(_) => tracing::debug!(%addr, "TLS handshake timed out"),
                }
            });
        }
    }
}

/// Dials `addr`. With `expected` set, the handshake fails unless the server
/// presents exactly that certificate.
pub async fn connect(addr: SocketAddr, id: &Identity, expected: Option<&str>) -> io::Result<TlsConn> {
    let connector = TlsConnector::from(tls::client_config(id, expected).map_err(other)?);
    let fut = async {
        let tcp = TcpStream::connect(addr).await?;
        let _ = tcp.set_nodelay(true);
        let name = ServerName::try_from("devbridge.local").map_err(other)?;
        connector.connect(name, tcp).await
    };
    let stream = tokio::time::timeout(HANDSHAKE_TIMEOUT, fut)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "connection timed out"))??;
    let fp = stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|c| c.first())
        .map(|c| fingerprint(c))
        .ok_or_else(|| other("peer presented no certificate"))?;
    Ok(TlsConn { stream: TlsStream::Client(stream), peer_fingerprint: fp, peer_addr: addr, outbound: true })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{Frame, read_frame, write_frame};
    use devbridge_core::protocol::Message;
    use tokio::io::AsyncWriteExt;

    async fn listen(id: &Identity) -> (SocketAddr, mpsc::Receiver<TlsConn>) {
        let l = Listener::bind("127.0.0.1:0".parse().unwrap(), id).await.unwrap();
        let addr = l.local_addr().unwrap();
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(l.run(tx));
        (addr, rx)
    }

    #[tokio::test]
    async fn mutual_tls_reports_both_fingerprints() {
        let server = Identity::generate().unwrap();
        let client = Identity::generate().unwrap();
        let (addr, mut rx) = listen(&server).await;
        let mut c = connect(addr, &client, Some(&server.fingerprint)).await.unwrap();
        assert_eq!(c.peer_fingerprint, server.fingerprint);
        write_frame(&mut c.stream, &Frame::Msg(Message::Ping { nonce: 9 })).await.unwrap();
        c.stream.flush().await.unwrap();
        let mut s = rx.recv().await.unwrap();
        assert_eq!(s.peer_fingerprint, client.fingerprint);
        assert_eq!(read_frame(&mut s.stream).await.unwrap(), Some(Frame::Msg(Message::Ping { nonce: 9 })));
    }

    #[tokio::test]
    async fn wrong_pinned_fingerprint_is_refused() {
        let server = Identity::generate().unwrap();
        let impostor_expected = Identity::generate().unwrap().fingerprint;
        let client = Identity::generate().unwrap();
        let (addr, _rx) = listen(&server).await;
        assert!(connect(addr, &client, Some(&impostor_expected)).await.is_err());
    }

    #[tokio::test]
    async fn plain_tcp_without_tls_gets_nothing() {
        let server = Identity::generate().unwrap();
        let (addr, mut rx) = listen(&server).await;
        let mut tcp = TcpStream::connect(addr).await.unwrap();
        tcp.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();
        let got = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
        assert!(got.is_err(), "no connection is surfaced without a TLS client certificate");
    }
}
