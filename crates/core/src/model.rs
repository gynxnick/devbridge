//! Domain model shared by persistence, commands and (via JSON) the UI.
//!
//! Enum string forms are stored in SQLite and sent to the UI, so they are part of
//! the on-disk format: never rename a variant's string without a migration.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ids::{DeviceId, WorkspaceId};

#[derive(Debug, Error, PartialEq, Eq)]
#[error("unknown {kind} value: {value:?}")]
pub struct UnknownVariant {
    pub kind: &'static str,
    pub value: String,
}

/// Declares an enum whose string form is stable (DB + JSON).
macro_rules! str_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident => $s:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $($(#[$vmeta])* #[serde(rename = $s)] $variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $s),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = UnknownVariant;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($s => Ok($name::$variant),)+
                    _ => Err(UnknownVariant { kind: stringify!($name), value: s.to_owned() }),
                }
            }
        }
    };
}

str_enum!(
    /// How a workspace is shared between devices.
    WorkspaceMode { Synced => "synced", Remote => "remote" }
);

str_enum!(
    /// User-visible synchronization state of a workspace.
    SyncState {
        /// No paired device shares this workspace yet.
        LocalOnly => "local_only",
        Paused => "paused",
        Idle => "idle",
        Syncing => "syncing",
        Offline => "offline",
        Error => "error",
    }
);

str_enum!(
    DeviceStatus { Local => "local", Pending => "pending", Paired => "paired", Revoked => "revoked" }
);

str_enum!(
    DeviceRole { Peer => "peer", Host => "host", Client => "client" }
);

str_enum!(
    OsKind { MacOs => "macos", Windows => "windows", Linux => "linux", Other => "other" }
);

impl OsKind {
    pub fn current() -> Self {
        match std::env::consts::OS {
            "macos" => OsKind::MacOs,
            "windows" => OsKind::Windows,
            "linux" => OsKind::Linux,
            _ => OsKind::Other,
        }
    }
}

str_enum!(
    /// Accent colors offered for workspaces. Rendered by the UI's palette.
    WorkspaceColor {
        Slate => "slate", Red => "red", Orange => "orange", Amber => "amber",
        Green => "green", Teal => "teal", Blue => "blue", Indigo => "indigo",
        Violet => "violet", Pink => "pink",
    }
);

str_enum!(
    /// Icons offered for workspaces. Names map to icons in the UI.
    WorkspaceIcon {
        Folder => "folder", Code => "code", Globe => "globe", Server => "server",
        Smartphone => "smartphone", Gamepad => "gamepad", Database => "database",
        Package => "package", Terminal => "terminal", Book => "book",
    }
);

pub const WORKSPACE_NAME_MAX_CHARS: usize = 80;
pub const DEVICE_NAME_MAX_CHARS: usize = 64;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum NameError {
    #[error("Name can't be empty.")]
    Empty,
    #[error("Name is too long (maximum {max} characters).")]
    TooLong { max: usize },
    #[error("Name can't contain control characters.")]
    ControlChars,
}

/// Trims and validates a human-readable display name.
pub fn validate_display_name(raw: &str, max: usize) -> Result<String, NameError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name.chars().count() > max {
        return Err(NameError::TooLong { max });
    }
    if name.chars().any(char::is_control) {
        return Err(NameError::ControlChars);
    }
    Ok(name.to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: DeviceId,
    pub name: String,
    pub os: OsKind,
    pub os_version: Option<String>,
    pub status: DeviceStatus,
    pub role: DeviceRole,
    pub is_local: bool,
    /// SHA-256 fingerprint of the device's pinned TLS public key (paired devices).
    pub cert_fingerprint: Option<String>,
    pub last_seen_at: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    /// Absolute, canonical path of the workspace root on *this* device.
    pub root_path: String,
    pub color: WorkspaceColor,
    pub icon: WorkspaceIcon,
    pub mode: WorkspaceMode,
    pub sync_state: SyncState,
    pub active_device_id: Option<DeviceId>,
    pub last_synced_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

str_enum!(
    ThemePreference { System => "system", Light => "light", Dark => "dark" }
);

/// Application settings, persisted as one JSON document. Every field has a
/// default so older stored documents keep loading as fields are added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    pub theme: ThemePreference,
    /// Custom VS Code CLI path. `None` means auto-detect.
    pub vscode_path: Option<String>,
    /// Custom Git executable path. `None` means auto-detect.
    pub git_path: Option<String>,
    /// Extra exclusion patterns (gitignore syntax) applied to every workspace.
    pub exclude_patterns: Vec<String>,
    /// Secret-pattern opt-ins: default secret exclusions the user explicitly allowed to sync.
    pub secret_optins: Vec<String>,
    pub respect_gitignore: bool,
    pub max_concurrent_transfers: u32,
    /// 0 = unlimited.
    pub bandwidth_limit_kbps: u32,
    pub max_file_size_mb: u32,
    pub launch_at_login: bool,
    pub start_minimized: bool,
    pub notifications: bool,
    pub recovery_retention_days: u32,
    pub reconcile_interval_minutes: u32,
    /// Accept connections from paired devices on the local network. Off by
    /// default: DevBridge then listens on loopback only.
    pub lan_enabled: bool,
    pub listen_port: u16,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            vscode_path: None,
            git_path: None,
            exclude_patterns: Vec::new(),
            secret_optins: Vec::new(),
            respect_gitignore: true,
            max_concurrent_transfers: 4,
            bandwidth_limit_kbps: 0,
            max_file_size_mb: 2048,
            launch_at_login: false,
            start_minimized: false,
            notifications: true,
            recovery_retention_days: 30,
            reconcile_interval_minutes: 10,
            lan_enabled: false,
            listen_port: 47821,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SettingsError {
    #[error("{field} must be between {min} and {max}.")]
    OutOfRange { field: &'static str, min: u32, max: u32 },
    #[error("{field} must not be empty when set.")]
    EmptyPath { field: &'static str },
}

impl AppSettings {
    /// Rejects values the engine can't honour. Called before persisting.
    pub fn validate(&self) -> Result<(), SettingsError> {
        fn range(field: &'static str, v: u32, min: u32, max: u32) -> Result<(), SettingsError> {
            if (min..=max).contains(&v) { Ok(()) } else { Err(SettingsError::OutOfRange { field, min, max }) }
        }
        range("Concurrent transfers", self.max_concurrent_transfers, 1, 32)?;
        range("Bandwidth limit", self.bandwidth_limit_kbps, 0, 10_000_000)?;
        range("Maximum file size", self.max_file_size_mb, 1, 1_048_576)?;
        range("Recovery retention", self.recovery_retention_days, 1, 3650)?;
        range("Reconcile interval", self.reconcile_interval_minutes, 1, 1440)?;
        if self.listen_port < 1024 {
            return Err(SettingsError::OutOfRange { field: "Port", min: 1024, max: 65535 });
        }
        for (field, v) in [("VS Code path", &self.vscode_path), ("Git path", &self.git_path)] {
            if v.as_deref().is_some_and(|p| p.trim().is_empty()) {
                return Err(SettingsError::EmptyPath { field });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_strings_round_trip() {
        for v in SyncState::ALL {
            assert_eq!(v.as_str().parse::<SyncState>().unwrap(), *v);
        }
        for v in WorkspaceIcon::ALL {
            assert_eq!(v.as_str().parse::<WorkspaceIcon>().unwrap(), *v);
        }
        assert!("SYNCED".parse::<WorkspaceMode>().is_err());
    }

    #[test]
    fn enum_json_matches_db_string() {
        assert_eq!(serde_json::to_string(&SyncState::LocalOnly).unwrap(), "\"local_only\"");
        assert_eq!(serde_json::to_string(&OsKind::MacOs).unwrap(), "\"macos\"");
    }

    #[test]
    fn display_name_validation() {
        assert_eq!(validate_display_name("  My App ", 80).unwrap(), "My App");
        assert_eq!(validate_display_name("   ", 80), Err(NameError::Empty));
        assert_eq!(validate_display_name("a\nb", 80), Err(NameError::ControlChars));
        assert_eq!(validate_display_name(&"x".repeat(81), 80), Err(NameError::TooLong { max: 80 }));
        // Counted in characters, not bytes.
        assert!(validate_display_name(&"é".repeat(80), 80).is_ok());
    }

    #[test]
    fn settings_default_is_valid_and_partial_json_loads() {
        assert_eq!(AppSettings::default().validate(), Ok(()));
        let s: AppSettings = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(s.theme, ThemePreference::Dark);
        assert_eq!(s.max_concurrent_transfers, 4);
    }

    #[test]
    fn settings_validation_rejects_bad_values() {
        let s = AppSettings { max_concurrent_transfers: 0, ..Default::default() };
        assert!(matches!(s.validate(), Err(SettingsError::OutOfRange { .. })));
        let s = AppSettings { vscode_path: Some("  ".into()), ..Default::default() };
        assert!(matches!(s.validate(), Err(SettingsError::EmptyPath { .. })));
    }
}
