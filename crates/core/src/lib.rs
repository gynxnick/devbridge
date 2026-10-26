//! Pure domain types shared by every DevBridge crate.
//!
//! Nothing in this crate talks to the network, the database or the UI. Anything
//! that needs to be unit-tested in isolation (identifiers, enums with stable wire
//! names, appearance validation, …) lives here.

pub mod fsutil;
pub mod ids;
pub mod model;
pub mod protocol;
pub mod redact;
pub mod relpath;
pub mod rules;
pub mod time;
pub mod version;

pub use ids::{DeviceId, WorkspaceId};
pub use model::*;
pub use relpath::{PathError, RelPath};
