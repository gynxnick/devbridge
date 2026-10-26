//! Filesystem engine. Everything that touches workspace files on disk lives
//! here; it knows nothing about the database or the network.

pub mod apply;
pub mod hash;
pub mod recovery;
pub mod rules;
pub mod scan;
pub mod watch;

/// Free bytes on the volume holding `path`.
pub fn available_space(path: &std::path::Path) -> std::io::Result<u64> {
    fs4::available_space(path)
}
