//! A framed, full-duplex session over an authenticated TLS connection.
//!
//! Outgoing control messages have priority over content chunks, so acks and
//! pings aren't stuck behind a large transfer. Both outgoing queues are
//! bounded, which gives natural backpressure to senders.

use std::net::SocketAddr;

use devbridge_core::protocol::Message;
use tokio::io::{AsyncWriteExt, split};
use tokio::sync::{mpsc, watch};

use crate::conn::TlsConn;
use crate::frame::{Frame, read_frame, write_frame};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Closed;

#[derive(Clone)]
pub struct SessionTx {
    ctrl: mpsc::Sender<Frame>,
    data: mpsc::Sender<Frame>,
    close: watch::Sender<bool>,
}

impl SessionTx {
    pub async fn send(&self, msg: Message) -> Result<(), Closed> {
        self.ctrl.send(Frame::Msg(msg)).await.map_err(|_| Closed)
    }

    /// Sends a control message *in order with* previously queued chunks
    /// (e.g. "content end"), instead of with control priority.
    pub async fn send_after_chunks(&self, msg: Message) -> Result<(), Closed> {
        self.data.send(Frame::Msg(msg)).await.map_err(|_| Closed)
    }

    pub async fn send_chunk(&self, transfer_id: &str, offset: u64, data: Vec<u8>) -> Result<(), Closed> {
        self.data.send(Frame::Chunk { transfer_id: transfer_id.into(), offset, data }).await.map_err(|_| Closed)
    }

    pub fn close(&self) {
        let _ = self.close.send(true);
    }

    pub fn is_closed(&self) -> bool {
        *self.close.borrow() || self.ctrl.is_closed()
    }
}

pub struct Session {
    pub peer_fingerprint: String,
    pub peer_addr: SocketAddr,
    pub outbound: bool,
    pub tx: SessionTx,
    pub rx: mpsc::Receiver<Frame>,
}

pub fn spawn(conn: TlsConn) -> Session {
    let (mut rd, mut wr) = split(conn.stream);
    let (ctrl_tx, mut ctrl_rx) = mpsc::channel::<Frame>(256);
    let (data_tx, mut data_rx) = mpsc::channel::<Frame>(8);
    let (in_tx, in_rx) = mpsc::channel::<Frame>(256);
    let (close_tx, mut close_rx) = watch::channel(false);

    let close_on_read_end = close_tx.clone();
    tokio::spawn(async move {
        loop {
            match read_frame(&mut rd).await {
                Ok(Some(f)) => {
                    if in_tx.send(f).await.is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::debug!(error = %e, "session read ended");
                    break;
                }
            }
        }
        let _ = close_on_read_end.send(true);
    });

    tokio::spawn(async move {
        loop {
            let f = tokio::select! {
                biased;
                _ = close_rx.changed() => break,
                Some(f) = ctrl_rx.recv() => f,
                Some(f) = data_rx.recv() => f,
                else => break,
            };
            if write_frame(&mut wr, &f).await.is_err() || wr.flush().await.is_err() {
                break;
            }
        }
        // Drain queued control frames (e.g. a final Bye) best-effort.
        while let Ok(f) = ctrl_rx.try_recv() {
            if write_frame(&mut wr, &f).await.is_err() {
                break;
            }
        }
        let _ = wr.flush().await;
        let _ = wr.shutdown().await;
    });

    Session {
        peer_fingerprint: conn.peer_fingerprint,
        peer_addr: conn.peer_addr,
        outbound: conn.outbound,
        tx: SessionTx { ctrl: ctrl_tx, data: data_tx, close: close_tx },
        rx: in_rx,
    }
}
