//! Wire framing over the TLS stream:
//!
//! ```text
//! u32 BE length (of kind + payload) | u8 kind | payload
//! kind 0: JSON `Envelope`
//! kind 1: content chunk = 26-byte ASCII transfer id (ULID) | u64 BE offset | data
//! ```
//! Frames above `MAX_FRAME` are rejected before allocation.

use std::io;

use devbridge_core::protocol::{CHUNK_SIZE, Envelope, Message, PROTOCOL_VERSION};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_FRAME: usize = CHUNK_SIZE + 1024;
const ID_LEN: usize = 26;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Msg(Message),
    Chunk { transfer_id: String, offset: u64, data: Vec<u8> },
}

fn invalid(m: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, m.into())
}

pub fn encode(f: &Frame) -> io::Result<Vec<u8>> {
    let (kind, payload) = match f {
        Frame::Msg(m) => (0u8, serde_json::to_vec(&Envelope::new(m.clone())).map_err(|e| invalid(e.to_string()))?),
        Frame::Chunk { transfer_id, offset, data } => {
            if transfer_id.len() != ID_LEN || !transfer_id.is_ascii() {
                return Err(invalid("bad transfer id"));
            }
            let mut p = Vec::with_capacity(ID_LEN + 8 + data.len());
            p.extend_from_slice(transfer_id.as_bytes());
            p.extend_from_slice(&offset.to_be_bytes());
            p.extend_from_slice(data);
            (1u8, p)
        }
    };
    if payload.len() + 1 > MAX_FRAME {
        return Err(invalid("frame too large"));
    }
    let mut out = Vec::with_capacity(5 + payload.len());
    out.extend_from_slice(&((payload.len() + 1) as u32).to_be_bytes());
    out.push(kind);
    out.extend_from_slice(&payload);
    Ok(out)
}

pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, f: &Frame) -> io::Result<()> {
    w.write_all(&encode(f)?).await
}

/// Reads one frame; `Ok(None)` on clean end of stream.
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Option<Frame>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(invalid(format!("invalid frame length {len}")));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    decode(buf).map(Some)
}

fn decode(buf: Vec<u8>) -> io::Result<Frame> {
    match buf[0] {
        0 => {
            let env: Envelope = serde_json::from_slice(&buf[1..]).map_err(|e| invalid(format!("bad message: {e}")))?;
            if env.v != PROTOCOL_VERSION {
                return Err(invalid(format!("unsupported protocol version {}", env.v)));
            }
            Ok(Frame::Msg(env.msg))
        }
        1 => {
            if buf.len() < 1 + ID_LEN + 8 {
                return Err(invalid("short chunk"));
            }
            let id = std::str::from_utf8(&buf[1..1 + ID_LEN]).map_err(|_| invalid("bad transfer id"))?.to_owned();
            let offset = u64::from_be_bytes(buf[1 + ID_LEN..1 + ID_LEN + 8].try_into().expect("8 bytes"));
            Ok(Frame::Chunk { transfer_id: id, offset, data: buf[1 + ID_LEN + 8..].to_vec() })
        }
        k => Err(invalid(format!("unknown frame kind {k}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn round_trip_and_limits() {
        let frames = vec![
            Frame::Msg(Message::Ping { nonce: 1 }),
            Frame::Chunk { transfer_id: "01JAAAAAAAAAAAAAAAAAAAAAAA".into(), offset: 42, data: vec![7; 1000] },
        ];
        let mut buf = Vec::new();
        for f in &frames {
            write_frame(&mut buf, f).await.unwrap();
        }
        let mut r = &buf[..];
        for f in &frames {
            assert_eq!(read_frame(&mut r).await.unwrap().as_ref(), Some(f));
        }
        assert_eq!(read_frame(&mut r).await.unwrap(), None);

        let huge = (MAX_FRAME as u32 + 1).to_be_bytes();
        assert!(read_frame(&mut &huge[..]).await.is_err());
        let wrong_version = br#"{"v":99,"msg":{"type":"ping","body":{"nonce":1}}}"#;
        let mut f = ((wrong_version.len() + 1) as u32).to_be_bytes().to_vec();
        f.push(0);
        f.extend_from_slice(wrong_version);
        assert!(read_frame(&mut &f[..]).await.is_err());
        let too_big =
            Frame::Chunk { transfer_id: "01JAAAAAAAAAAAAAAAAAAAAAAA".into(), offset: 0, data: vec![0; MAX_FRAME] };
        assert!(encode(&too_big).is_err());
    }
}
