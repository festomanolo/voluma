//! Common filesystem interface. Drivers report exactly what they support via
//! `Capabilities`; unsupported operations return `FsError::Unsupported`
//! rather than being silently emulated.

use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("not a directory: {0}")]
    NotADirectory(String),
    #[error("is a directory: {0}")]
    IsADirectory(String),
    #[error("unsupported: {0}")]
    Unsupported(&'static str),
    #[error("corrupt filesystem: {0}")]
    Corrupt(&'static str),
    #[error(transparent)]
    Block(#[from] block_device::BlockError),
}
pub type Result<T> = std::result::Result<T, FsError>;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Capabilities {
    pub read: bool,
    pub write: bool,
    pub create: bool,
    pub delete: bool,
    pub rename: bool,
    pub permissions: bool,
    pub symbolic_links: bool,
    pub extended_attributes: bool,
}

impl Capabilities {
    pub const READ_ONLY: Self = Self {
        read: true, write: false, create: false, delete: false, rename: false,
        permissions: false, symbolic_links: false, extended_attributes: false,
    };
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind { File, Directory }

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    /// Seconds since Unix epoch, `None` if the filesystem has no value.
    pub modified: Option<i64>,
    pub created: Option<i64>,
    pub hidden: bool,
    pub read_only: bool,
    /// Driver-private locator (e.g. first cluster); opaque to callers.
    pub inode: u64,
}

pub trait Filesystem: Send + Sync {
    fn name(&self) -> &'static str;
    fn label(&self) -> Option<String>;
    fn capabilities(&self) -> Capabilities;
    fn stat(&self, path: &str) -> Result<DirEntry>;
    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>>;
    /// Streaming positional read; returns bytes read (0 at EOF).
    fn read_at(&self, entry: &DirEntry, offset: u64, buf: &mut [u8]) -> Result<usize>;
}

/// Split a path into components, rejecting `..` escapes. Accepts `/a/b`, `a/b`, `""`.
pub fn normalize_path(path: &str) -> Result<Vec<&str>> {
    let mut out = Vec::new();
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => { if out.pop().is_none() { return Err(FsError::NotFound(path.into())); } }
            c => out.push(c),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes() {
        assert_eq!(normalize_path("/a//b/./c").unwrap(), ["a", "b", "c"]);
        assert_eq!(normalize_path("").unwrap(), Vec::<&str>::new());
        assert_eq!(normalize_path("a/b/..").unwrap(), ["a"]);
    }
    #[test]
    fn rejects_escape() { assert!(normalize_path("/../etc").is_err()); }
}
