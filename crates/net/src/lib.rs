//! Networking for DevBridge.
//!
//! Trust model: every device has a long-lived self-signed TLS certificate whose
//! private key lives in the OS credential store. Two devices trust each other
//! only after a PAKE-protected pairing that the receiving user approves; from
//! then on connections use TLS 1.3 with mutual authentication, and the
//! application layer checks the peer certificate's fingerprint against the
//! pinned value on every connection. No CA, no relay, no Web PKI.

pub mod conn;
pub mod discovery;
pub mod frame;
pub mod identity;
pub mod pairing;
pub mod qr;
pub mod session;
pub mod tls;

pub use conn::{Listener, TlsConn, connect};
pub use frame::Frame;
pub use identity::{Identity, KeyringStore, MemoryStore, SecretStore};
pub use session::{Session, SessionTx};

/// Default TCP port for DevBridge connections.
pub const DEFAULT_PORT: u16 = 47821;
