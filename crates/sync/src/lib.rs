//! Sync engine for Synced Project Mode.

pub mod apply;
pub mod engine;
pub mod indexer;

pub use engine::{Engine, EngineConfig, EngineEvent, EngineStatus, ListenConfig, PairingWindow, Tuning};

use std::sync::{Mutex, MutexGuard};

use devbridge_store::{Store, StoreError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Apply(#[from] devbridge_fs::apply::ApplyError),
    #[error("{0}")]
    Other(String),
}

/// Locks the store. A poisoned lock (panic mid-operation) is recovered: SQLite
/// transactions keep the data consistent.
pub fn lock(store: &Mutex<Store>) -> MutexGuard<'_, Store> {
    store.lock().unwrap_or_else(|p| p.into_inner())
}
