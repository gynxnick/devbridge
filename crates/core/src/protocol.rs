//! DevBridge sync protocol v1: control messages exchanged between paired
//! devices. This file is the normative definition; docs/PROTOCOL.md describes
//! it in prose. Binary content chunks are framed separately (see `devbridge-net`).
//!
//! Every `RelPath` field validates on deserialization, so a malformed or
//! hostile path is rejected before any handler sees it.

use serde::{Deserialize, Serialize};

use crate::ids::{DeviceId, WorkspaceId};
use crate::model::{OsKind, WorkspaceColor, WorkspaceIcon};
use crate::relpath::RelPath;

pub const PROTOCOL_VERSION: u32 = 1;
/// Content chunk size for transfers.
pub const CHUNK_SIZE: usize = 1024 * 1024;
/// Upper bound for one batch of changes or acks.
pub const MAX_BATCH: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub device_id: DeviceId,
    pub name: String,
    pub os: OsKind,
    pub app_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeOp {
    Upsert,
    Delete,
}

/// One file version, as announced to a peer. Renames are a delete plus an
/// upsert; the receiver satisfies the upsert from local content with the same
/// hash when it has it, so renames don't re-transfer data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub change_id: String,
    pub origin_device: DeviceId,
    pub path: RelPath,
    pub op: ChangeOp,
    pub version_id: String,
    /// Ancestor version ids, nearest first (bounded).
    pub ancestry: Vec<String>,
    /// Short content keys of ancestors (see `version::content_key`), so a
    /// receiver can tell the change was made on top of content it already has.
    #[serde(default)]
    pub base_hashes: Vec<String>,
    /// Hex BLAKE3 of the content (`None` for deletions).
    pub hash: Option<String>,
    pub size: u64,
    pub mode: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AckResult {
    Applied,
    Duplicate,
    /// Accepted; content transfer is in progress. A final ack follows.
    Pending,
    Conflict,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ack {
    pub change_id: String,
    pub result: AckResult,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteWorkspace {
    pub ws: WorkspaceId,
    pub name: String,
    /// Absolute path on the host, in the host's native form.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "body", rename_all = "snake_case")]
pub enum Message {
    // --- session ---
    /// `listen_port`: where the sender accepts connections (for reconnecting).
    Hello {
        device: DeviceInfo,
        protocol_versions: Vec<u32>,
        #[serde(default)]
        listen_port: Option<u16>,
    },
    Ping {
        nonce: u64,
    },
    Pong {
        nonce: u64,
    },
    Error {
        code: String,
        message: String,
    },
    Bye {
        reason: String,
    },

    // --- workspace sharing ---
    WorkspaceOffer {
        ws: WorkspaceId,
        name: String,
        color: WorkspaceColor,
        icon: WorkspaceIcon,
    },
    WorkspaceJoined {
        ws: WorkspaceId,
    },
    WorkspaceLeft {
        ws: WorkspaceId,
    },

    // --- sync ---
    /// Digest of the sender's manifest; the receiver requests the full
    /// manifest only if its view differs.
    ManifestDigest {
        ws: WorkspaceId,
        digest: String,
        count: u64,
    },
    ManifestRequest {
        ws: WorkspaceId,
    },
    /// A batch of changes. `reconcile` marks manifest entries sent for
    /// reconciliation rather than new live edits.
    Changes {
        ws: WorkspaceId,
        changes: Vec<Change>,
        reconcile: bool,
    },
    Acks {
        ws: WorkspaceId,
        acks: Vec<Ack>,
    },
    /// Request content by hash, starting at `offset` (resume support).
    ContentRequest {
        transfer_id: String,
        ws: WorkspaceId,
        hash: String,
        offset: u64,
    },
    ContentEnd {
        transfer_id: String,
        size: u64,
    },
    ContentError {
        transfer_id: String,
        code: String,
        message: String,
    },
    ContentCancel {
        transfer_id: String,
    },

    // --- pairing (only before the peer is trusted) ---
    PairRequest {
        spake: String,
        device: DeviceInfo,
    },
    PairResponse {
        spake: String,
    },
    PairConfirm {
        mac: String,
    },
    PairAccepted {
        mac: String,
        device: DeviceInfo,
    },
    PairRejected {
        reason: String,
    },

    // --- remote workspace (host/client) ---
    RemoteAccessRequest {
        ssh_public_key: String,
    },
    RemoteAccessGranted {
        ssh_user: String,
        ssh_port: u16,
        host_os: OsKind,
        workspaces: Vec<RemoteWorkspace>,
        /// The host's SSH server keys, pinned by the client (no TOFU prompt).
        #[serde(default)]
        host_keys: Vec<String>,
    },
    RemoteAccessDenied {
        reason: String,
    },
    RemoteAccessRevoked {},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u32,
    pub msg: Message,
}

impl Envelope {
    pub fn new(msg: Message) -> Self {
        Self { v: PROTOCOL_VERSION, msg }
    }
}

impl Message {
    /// Messages allowed on a connection whose peer is not (yet) paired.
    pub fn allowed_before_pairing(&self) -> bool {
        matches!(
            self,
            Message::PairRequest { .. }
                | Message::PairResponse { .. }
                | Message::PairConfirm { .. }
                | Message::PairAccepted { .. }
                | Message::PairRejected { .. }
                | Message::Error { .. }
                | Message::Bye { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format_is_stable() {
        let env = Envelope::new(Message::Ping { nonce: 7 });
        assert_eq!(serde_json::to_string(&env).unwrap(), r#"{"v":1,"msg":{"type":"ping","body":{"nonce":7}}}"#);
    }

    #[test]
    fn hostile_path_is_rejected_at_parse_time() {
        let raw = r#"{"v":1,"msg":{"type":"changes","body":{"ws":"01JAAAAAAAAAAAAAAAAAAAAAAA","reconcile":false,"changes":[
            {"changeId":"c","originDevice":"01JAAAAAAAAAAAAAAAAAAAAAAA","path":"../../.ssh/authorized_keys","op":"upsert",
             "versionId":"v","ancestry":[],"hash":"00","size":1,"mode":null}]}}}"#;
        assert!(serde_json::from_str::<Envelope>(raw).is_err());
    }

    #[test]
    fn change_round_trips() {
        let c = Change {
            change_id: "c1".into(),
            origin_device: DeviceId::new(),
            path: RelPath::parse("src/a.rs").unwrap(),
            op: ChangeOp::Upsert,
            version_id: "v1".into(),
            ancestry: vec!["v0".into()],
            base_hashes: vec!["abababababababab".into()],
            hash: Some("ab".into()),
            size: 3,
            mode: Some(0o644),
        };
        let env = Envelope::new(Message::Changes { ws: WorkspaceId::new(), changes: vec![c], reconcile: false });
        let back: Envelope = serde_json::from_str(&serde_json::to_string(&env).unwrap()).unwrap();
        assert_eq!(back, env);
    }

    #[test]
    fn only_pairing_messages_allowed_unpaired() {
        assert!(Message::PairConfirm { mac: String::new() }.allowed_before_pairing());
        assert!(!Message::ManifestRequest { ws: WorkspaceId::new() }.allowed_before_pairing());
    }
}
