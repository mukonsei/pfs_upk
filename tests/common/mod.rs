//! Shared helpers for the integration tests.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A self-deleting directory under the system temp dir.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Creates a unique directory named after `tag`.
    pub fn new(tag: &str) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pfs_upk_test_{}_{}_{}",
            std::process::id(),
            unique,
            tag
        ));
        if path.exists() {
            fs::remove_dir_all(&path).expect("clean stale temp dir");
        }
        fs::create_dir_all(&path).expect("create temp dir");
        TempDir { path }
    }

    /// Root of the directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Joins a relative path onto the root.
    pub fn join(&self, relative: &str) -> PathBuf {
        let mut path = self.path.clone();
        for part in relative.split(['/', '\\']) {
            path.push(part);
        }
        path
    }

    /// Writes `contents` at `relative`, creating parent directories.
    pub fn write(&self, relative: &str, contents: &[u8]) -> PathBuf {
        let path = self.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(&path, contents).expect("write file");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Reads every regular file below `root` into a map keyed by a `/` separated
/// relative path.
pub fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    collect(root, root, &mut out);
    out
}

fn collect(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(dir)
        .expect("read dir")
        .collect::<std::result::Result<Vec<_>, _>>()
        .expect("read dir entries");
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().expect("file type");
        if file_type.is_dir() {
            collect(root, &path, out);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .expect("path below root")
            .to_string_lossy()
            .replace('\\', "/");
        out.insert(relative, fs::read(&path).expect("read file"));
    }
}
