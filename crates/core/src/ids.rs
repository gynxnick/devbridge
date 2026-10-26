//! Strongly typed identifiers. All IDs are ULIDs: sortable by creation time and
//! globally unique without coordination between devices.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use ulid::Ulid;

macro_rules! ulid_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Ulid);

        impl $name {
            pub fn new() -> Self {
                Self(Ulid::generate())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = ulid::DecodeError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ulid::from_string(s).map(Self)
            }
        }
    };
}

ulid_id!(
    /// Identifies one installation of DevBridge on one computer.
    DeviceId
);
ulid_id!(
    /// Identifies a project registered with DevBridge. Stable across devices.
    WorkspaceId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_string_and_json() {
        let id = WorkspaceId::new();
        let parsed: WorkspaceId = id.to_string().parse().unwrap();
        assert_eq!(id, parsed);

        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{id}\""));
        assert_eq!(serde_json::from_str::<WorkspaceId>(&json).unwrap(), id);
    }

    #[test]
    fn rejects_garbage() {
        assert!("not-a-ulid".parse::<DeviceId>().is_err());
        assert!("".parse::<DeviceId>().is_err());
    }
}
