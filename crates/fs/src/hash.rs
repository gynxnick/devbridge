//! Streaming BLAKE3 content hashing. Memory use is constant regardless of
//! file size.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

pub type ContentHash = String;

/// Returns `(hex hash, size)` of the file's content.
pub fn hash_file(path: &Path) -> io::Result<(ContentHash, u64)> {
    hash_reader(File::open(path)?)
}

pub fn hash_reader(mut r: impl Read) -> io::Result<(ContentHash, u64)> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut size = 0u64;
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    Ok((hasher.finalize().to_hex().to_string(), size))
}

pub fn hash_bytes(b: &[u8]) -> ContentHash {
    blake3::hash(b).to_hex().to_string()
}

/// True if `s` looks like a hex BLAKE3 digest (prevents hashes being used as
/// path fragments in the recovery store with unexpected characters).
pub fn is_valid_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_hash_matches_bytes_hash() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        let data: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&p, &data).unwrap();
        let (h, n) = hash_file(&p).unwrap();
        assert_eq!(n, data.len() as u64);
        assert_eq!(h, hash_bytes(&data));
        assert!(is_valid_hash(&h));
        assert_ne!(h, hash_bytes(b"other"));
    }

    #[test]
    fn hash_validation() {
        assert!(!is_valid_hash("../etc"));
        assert!(!is_valid_hash(&"A".repeat(64)));
        assert!(is_valid_hash(&"a".repeat(64)));
    }
}
