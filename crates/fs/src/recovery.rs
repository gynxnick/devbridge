//! Content-addressed recovery store. Keeps copies of file versions that were
//! overwritten, deleted or involved in a conflict, so no sync decision can
//! lose data. Lives in the app data directory, never inside a workspace.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::hash::{ContentHash, hash_reader, is_valid_hash};

pub struct RecoveryStore {
    dir: PathBuf,
}

impl RecoveryStore {
    pub fn new(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(dir.join("blobs"))?;
        std::fs::create_dir_all(dir.join("tmp"))?;
        Ok(Self { dir })
    }

    /// Location of a blob, or `None` for a malformed hash.
    pub fn blob_path(&self, hash: &str) -> Option<PathBuf> {
        is_valid_hash(hash).then(|| self.dir.join("blobs").join(&hash[..2]).join(hash))
    }

    pub fn contains(&self, hash: &str) -> bool {
        self.blob_path(hash).is_some_and(|p| p.is_file())
    }

    /// Copies `src` into the store; returns its hash and size. The copy is
    /// hashed while written, so what's stored is exactly what's recorded.
    pub fn put_file(&self, src: &Path) -> io::Result<(ContentHash, u64)> {
        let tmp = self.dir.join("tmp").join(ulid::Ulid::generate().to_string());
        let result = (|| {
            let mut out = File::create(&tmp)?;
            let reader = TeeReader { inner: File::open(src)?, out: &mut out };
            let (hash, size) = hash_reader(reader)?;
            out.sync_all()?;
            Ok::<_, io::Error>((hash, size))
        })();
        match result {
            Ok((hash, size)) => {
                self.install(&tmp, &hash)?;
                Ok((hash, size))
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(e)
            }
        }
    }

    /// Moves an already-verified file (e.g. a completed transfer) into the
    /// store under `hash`, falling back to copy across volumes.
    pub fn adopt(&self, file: &Path, hash: &str) -> io::Result<()> {
        let dest = self.blob_path(hash).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad hash"))?;
        if dest.is_file() {
            let _ = std::fs::remove_file(file);
            return Ok(());
        }
        std::fs::create_dir_all(dest.parent().expect("blob has parent"))?;
        if std::fs::rename(file, &dest).is_err() {
            std::fs::copy(file, &dest)?;
            std::fs::remove_file(file)?;
        }
        Ok(())
    }

    fn install(&self, tmp: &Path, hash: &str) -> io::Result<()> {
        let dest = self.blob_path(hash).expect("hash from hasher is valid");
        std::fs::create_dir_all(dest.parent().expect("blob has parent"))?;
        if dest.is_file() {
            std::fs::remove_file(tmp)?;
        } else {
            std::fs::rename(tmp, dest)?;
        }
        Ok(())
    }

    pub fn remove(&self, hash: &str) -> io::Result<()> {
        match self.blob_path(hash) {
            Some(p) => match std::fs::remove_file(p) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            },
            None => Ok(()),
        }
    }

    pub fn read(&self, hash: &str) -> io::Result<Vec<u8>> {
        std::fs::read(self.blob_path(hash).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad hash"))?)
    }

    /// Total bytes used by stored blobs.
    pub fn usage_bytes(&self) -> u64 {
        fn walk(p: &Path) -> u64 {
            std::fs::read_dir(p)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| match e.file_type() {
                            Ok(t) if t.is_dir() => walk(&e.path()),
                            Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
                            Err(_) => 0,
                        })
                        .sum()
                })
                .unwrap_or(0)
        }
        walk(&self.dir.join("blobs"))
    }
}

struct TeeReader<'a, R> {
    inner: R,
    out: &'a mut File,
}

impl<R: io::Read> io::Read for TeeReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.out.write_all(&buf[..n])?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::hash_bytes;

    #[test]
    fn put_read_remove() {
        let d = tempfile::tempdir().unwrap();
        let s = RecoveryStore::new(d.path().join("rec")).unwrap();
        let f = d.path().join("f.txt");
        std::fs::write(&f, b"hello").unwrap();
        let (h, n) = s.put_file(&f).unwrap();
        assert_eq!((h.as_str(), n), (hash_bytes(b"hello").as_str(), 5));
        assert!(s.contains(&h));
        assert_eq!(s.read(&h).unwrap(), b"hello");
        // idempotent
        s.put_file(&f).unwrap();
        assert_eq!(s.usage_bytes(), 5);
        s.remove(&h).unwrap();
        assert!(!s.contains(&h));
    }

    #[test]
    fn rejects_path_like_hashes() {
        let d = tempfile::tempdir().unwrap();
        let s = RecoveryStore::new(d.path()).unwrap();
        assert!(s.blob_path("../../etc/passwd").is_none());
        assert!(s.read("..").is_err());
    }
}
