//! Pairing orchestration (protocol in `devbridge_net::pairing`).
//!
//! Receiving side ("host"): opens a 5-minute, single-use window with a code,
//! answers one PAKE exchange at a time, and asks its user to approve before
//! anything is trusted. Three wrong codes close the window.
//!
//! Joining side: enters the code (and address), verifies the host's MAC, and
//! stores the host as paired only after the host user approved.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

use data_encoding::BASE64;
use devbridge_core::protocol::Message;
use devbridge_core::time::now_ms;
use devbridge_core::{DEVICE_NAME_MAX_CHARS, Device, validate_display_name};
use devbridge_net::pairing::{self, ROLE_CLIENT, ROLE_SERVER};
use devbridge_net::session::{self, Session};
use devbridge_net::{Frame, TlsConn};
use serde::Serialize;
use tokio::sync::oneshot;

use super::{Engine, EngineEvent, Inner, guard};
use crate::{SyncError, lock};

const WINDOW: Duration = Duration::from_secs(5 * 60);
const STEP_TIMEOUT: Duration = Duration::from_secs(30);
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(150);
const MAX_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingWindow {
    pub code: String,
    pub expires_at: i64,
    pub addresses: Vec<String>,
    pub port: u16,
    pub qr_svg: Option<String>,
    #[serde(skip)]
    attempts: u32,
    #[serde(skip)]
    busy: bool,
}

/// This machine's primary LAN address (no packets are sent: connecting a UDP
/// socket only selects a route).
fn primary_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    s.local_addr().ok().map(|a| a.ip()).filter(|ip| !ip.is_unspecified() && !ip.is_loopback())
}

pub(super) fn open(engine: &Engine) -> Result<PairingWindow, SyncError> {
    let inner = engine.inner();
    let lan = engine.listen_config().lan;
    let port = inner.listen_port().ok_or_else(|| {
        SyncError::Other("DevBridge isn't listening for connections. Check Settings → Network.".into())
    })?;
    // Without LAN access only this computer can connect (useful for testing
    // two instances); the UI asks the user to allow LAN connections first.
    let ip = if lan { primary_ip() } else { Some(IpAddr::from([127, 0, 0, 1])) };
    let addresses: Vec<String> = ip.map(|ip| SocketAddr::new(ip, port).to_string()).into_iter().collect();
    let code = pairing::new_code();
    let qr_svg = ip.and_then(|ip| devbridge_net::qr::pairing_svg(&ip.to_string(), port, &code));
    let w = PairingWindow {
        code: code.clone(),
        expires_at: now_ms() + WINDOW.as_millis() as i64,
        addresses,
        port,
        qr_svg,
        attempts: 0,
        busy: false,
    };
    *guard(&inner.pairing) = Some(w.clone());
    let name = guard(&inner.device_name).clone();
    engine.advertise_pairing(Some(&name));
    let inner2 = inner.clone();
    let engine2 = engine.clone();
    tokio::spawn(async move {
        tokio::time::sleep(WINDOW).await;
        let expired = guard(&inner2.pairing).as_ref().is_some_and(|w| w.code == code);
        if expired {
            close(&inner2, "The pairing code expired.");
            engine2.advertise_pairing(None);
        }
    });
    Ok(w)
}

pub(super) fn close(inner: &Inner, reason: &str) {
    if guard(&inner.pairing).take().is_some() {
        inner.emit(EngineEvent::PairingClosed { reason: reason.to_owned() });
    }
}

async fn recv_msg(s: &mut Session, t: Duration) -> Option<Message> {
    match tokio::time::timeout(t, s.rx.recv()).await {
        Ok(Some(Frame::Msg(m))) => Some(m),
        _ => None,
    }
}

async fn reject(s: &Session, reason: &str) {
    let _ = s.tx.send(Message::PairRejected { reason: reason.to_owned() }).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    s.tx.close();
}

/// Host side: an unpaired device connected.
pub(super) async fn respond(inner: Arc<Inner>, conn: TlsConn) {
    let client_fp = conn.peer_fingerprint.clone();
    let mut s = session::spawn(conn);
    let code = {
        let mut w = guard(&inner.pairing);
        match w.as_mut() {
            Some(w) if w.expires_at > now_ms() && !w.busy => {
                w.busy = true;
                Some(w.code.clone())
            }
            _ => None,
        }
    };
    let Some(code) = code else {
        reject(&s, "This device isn't accepting pairing requests right now.").await;
        return;
    };
    let release = |inner: &Inner| {
        if let Some(w) = guard(&inner.pairing).as_mut() {
            w.busy = false;
        }
    };

    let Some(Message::PairRequest { spake, device }) = recv_msg(&mut s, STEP_TIMEOUT).await else {
        release(&inner);
        s.tx.close();
        return;
    };
    let (pake, my_msg) = pairing::pake_start(&code);
    if s.tx.send(Message::PairResponse { spake: BASE64.encode(&my_msg) }).await.is_err() {
        release(&inner);
        return;
    }
    let key = BASE64.decode(spake.as_bytes()).ok().and_then(|m| pairing::pake_finish(pake, &m));
    let confirm = recv_msg(&mut s, STEP_TIMEOUT).await;
    let server_fp = inner.identity.fingerprint.clone();
    let valid = match (&key, confirm) {
        (Some(k), Some(Message::PairConfirm { mac })) => k.verify(ROLE_CLIENT, &server_fp, &client_fp, &mac),
        _ => false,
    };
    if !valid {
        let too_many = {
            let mut w = guard(&inner.pairing);
            match w.as_mut() {
                Some(w) => {
                    w.attempts += 1;
                    w.busy = false;
                    w.attempts >= MAX_ATTEMPTS
                }
                None => false,
            }
        };
        tracing::warn!("pairing attempt with an incorrect code");
        if too_many {
            close(&inner, "Too many incorrect codes. Start pairing again to get a new code.");
        }
        reject(&s, "The code didn't match. Check it and try again.").await;
        return;
    }
    let key = key.expect("valid implies key");
    let name = validate_display_name(&device.name, DEVICE_NAME_MAX_CHARS).unwrap_or_else(|_| "Unnamed device".into());
    if device.device_id == inner.device_id {
        release(&inner);
        reject(&s, "A device can't pair with itself.").await;
        return;
    }

    // Ask the user. Nothing is trusted before they approve.
    let request_id = ulid::Ulid::generate().to_string();
    let (tx, rx) = oneshot::channel();
    guard(&inner.approvals).insert(request_id.clone(), tx);
    inner.emit(EngineEvent::PairingRequest {
        request_id: request_id.clone(),
        device_name: name.clone(),
        os: device.os,
        verification_code: key.verification_code(),
    });
    let approved = matches!(tokio::time::timeout(APPROVAL_TIMEOUT, rx).await, Ok(Ok(true)));
    guard(&inner.approvals).remove(&request_id);
    if !approved {
        reject(&s, "The other person declined the request.").await;
        close(&inner, "Pairing request declined.");
        return;
    }
    let store = inner.store.clone();
    let (id, os, fp) = (device.device_id, device.os, client_fp.clone());
    let n = name.clone();
    let saved = Inner::blocking(move || Ok(lock(&store).add_paired_device(id, &n, os, &fp, None)?)).await;
    if saved.is_err() {
        reject(&s, "Pairing failed on the other device.").await;
        close(&inner, "Couldn't save the paired device.");
        return;
    }
    let _ = s
        .tx
        .send(Message::PairAccepted { mac: key.mac(ROLE_SERVER, &server_fp, &client_fp), device: inner.device_info() })
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    s.tx.close();
    close(&inner, &format!("Paired with {name}."));
    inner.emit(EngineEvent::DevicesChanged);
}

/// Joining side.
pub(super) async fn join(inner: Arc<Inner>, addr: SocketAddr, code: &str) -> Result<Device, SyncError> {
    if pairing::normalize_code(code).is_none() {
        return Err(SyncError::Other(
            "That code doesn't look right. It has 8 letters and digits, like 7KQ2-M9XD.".into(),
        ));
    }
    let conn = devbridge_net::connect(addr, &inner.identity, None).await.map_err(|e| {
        SyncError::Other(format!(
            "Couldn't reach {addr}: {e}. Check the address and that both computers are on the same network."
        ))
    })?;
    let server_fp = conn.peer_fingerprint.clone();
    {
        let store = inner.store.clone();
        let fp = server_fp.clone();
        if let Ok(Some(d)) = Inner::blocking(move || Ok(lock(&store).paired_by_fingerprint(&fp)?)).await {
            return Ok(d);
        }
    }
    let mut s = session::spawn(conn);
    let (pake, my_msg) = pairing::pake_start(code);
    s.tx.send(Message::PairRequest { spake: BASE64.encode(&my_msg), device: inner.device_info() })
        .await
        .map_err(|_| SyncError::Other("Connection closed.".into()))?;
    let their = match recv_msg(&mut s, STEP_TIMEOUT).await {
        Some(Message::PairResponse { spake }) => spake,
        Some(Message::PairRejected { reason }) => return Err(SyncError::Other(reason)),
        _ => return Err(SyncError::Other("The other device didn't respond.".into())),
    };
    let key = BASE64
        .decode(their.as_bytes())
        .ok()
        .and_then(|m| pairing::pake_finish(pake, &m))
        .ok_or_else(|| SyncError::Other("Pairing failed (invalid response).".into()))?;
    let my_fp = inner.identity.fingerprint.clone();
    s.tx.send(Message::PairConfirm { mac: key.mac(ROLE_CLIENT, &server_fp, &my_fp) })
        .await
        .map_err(|_| SyncError::Other("Connection closed.".into()))?;
    inner.emit(EngineEvent::PairingVerification { verification_code: key.verification_code() });
    let result = match recv_msg(&mut s, APPROVAL_TIMEOUT + STEP_TIMEOUT).await {
        Some(Message::PairAccepted { mac, device }) => {
            if !key.verify(ROLE_SERVER, &server_fp, &my_fp, &mac) {
                Err(SyncError::Other(
                    "The other device couldn't prove it knows the code. Pairing was stopped; try again on a trusted network.".into(),
                ))
            } else if device.device_id == inner.device_id {
                Err(SyncError::Other("A device can't pair with itself.".into()))
            } else {
                let name = validate_display_name(&device.name, DEVICE_NAME_MAX_CHARS)
                    .unwrap_or_else(|_| "Unnamed device".into());
                let store = inner.store.clone();
                let a = addr.to_string();
                Inner::blocking(move || {
                    Ok(lock(&store).add_paired_device(device.device_id, &name, device.os, &server_fp, Some(&a))?)
                })
                .await
            }
        }
        Some(Message::PairRejected { reason }) => Err(SyncError::Other(reason)),
        _ => Err(SyncError::Other("No answer from the other device. The request may have timed out.".into())),
    };
    s.tx.close();
    if result.is_ok() {
        inner.emit(EngineEvent::DevicesChanged);
    }
    result
}
