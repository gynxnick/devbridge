//! Workspace Manager: registering project folders, validating them, and the
//! developer tools (VS Code, Git, Claude Code) used to work on them.

pub mod launch;
pub mod manager;
pub mod roots;
pub mod tools;

pub use manager::{ImportRequest, ManagerError, PathInspection, import_workspace, inspect_path};
pub use roots::{Protected, RootError, validate_root};
