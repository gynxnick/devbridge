//! Errors returned to the UI. Every command error has a stable `code` the UI can
//! branch on and a `message` written for the user. Messages never include file
//! contents or secrets.

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct CmdError {
    pub code: &'static str,
    pub message: String,
}

pub type CmdResult<T> = Result<T, CmdError>;

impl CmdError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    pub fn internal(e: impl std::fmt::Display) -> Self {
        tracing::error!(error = %e, "internal error");
        Self::new("internal", format!("Something went wrong inside DevBridge: {e}"))
    }
}

impl From<devbridge_store::StoreError> for CmdError {
    fn from(e: devbridge_store::StoreError) -> Self {
        use devbridge_store::StoreError as E;
        match e {
            E::NotFound(what) => Self::new("not_found", format!("The {what} no longer exists. Refresh and try again.")),
            E::DuplicateRoot => Self::new("duplicate_root", e.to_string()),
            E::NewerSchema { .. } => Self::new("newer_schema", e.to_string()),
            other => Self::internal(other),
        }
    }
}

impl From<devbridge_workspace::ManagerError> for CmdError {
    fn from(e: devbridge_workspace::ManagerError) -> Self {
        use devbridge_workspace::ManagerError as E;
        match e {
            E::Store(s) => s.into(),
            E::Root(r) => Self::new("invalid_root", r.to_string()),
            E::Name(n) => Self::new("invalid_name", n.to_string()),
            E::Git(g) => Self::new("git", g.to_string()),
            E::GitUnavailable => Self::new("git_unavailable", e.to_string()),
        }
    }
}

impl From<devbridge_workspace::launch::LaunchError> for CmdError {
    fn from(e: devbridge_workspace::launch::LaunchError) -> Self {
        Self::new("launch", e.to_string())
    }
}

impl From<devbridge_core::NameError> for CmdError {
    fn from(e: devbridge_core::NameError) -> Self {
        Self::new("invalid_name", e.to_string())
    }
}

impl From<devbridge_core::SettingsError> for CmdError {
    fn from(e: devbridge_core::SettingsError) -> Self {
        Self::new("invalid_settings", e.to_string())
    }
}

impl From<devbridge_git::GitError> for CmdError {
    fn from(e: devbridge_git::GitError) -> Self {
        match e {
            devbridge_git::GitError::NeedsConfirmation { .. } => Self::new("needs_confirmation", e.to_string()),
            devbridge_git::GitError::NotFound => Self::new("git_unavailable", e.to_string()),
            other => Self::new("git", other.to_string()),
        }
    }
}

impl From<devbridge_workspace::RootError> for CmdError {
    fn from(e: devbridge_workspace::RootError) -> Self {
        Self::new("invalid_root", e.to_string())
    }
}

impl From<devbridge_sync::SyncError> for CmdError {
    fn from(e: devbridge_sync::SyncError) -> Self {
        match e {
            devbridge_sync::SyncError::Store(s) => s.into(),
            devbridge_sync::SyncError::Other(m) if m == "lan_disabled" => Self::new("lan_disabled", m),
            other => Self::new("sync", other.to_string()),
        }
    }
}
