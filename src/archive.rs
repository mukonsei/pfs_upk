//! Filesystem side of packing and unpacking.
//!
//! Both directions stream entry bodies through a fixed buffer, so peak memory
//! is bounded by the index size (`index_size`, at most 64 MiB by default) and
//! not by the size of the largest file.
//!
//! Nothing is written until the whole index has been parsed and validated, and
//! an interrupted unpack removes the directory it created. A pack writes to a
//! sibling temporary file and renames it into place only after a successful
//! sync, so a crash cannot leave a half-written archive under the final name.

use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::codec::xor_chunk_in_place;
use crate::error::{Error, Result};
use crate::format::{self, NewEntry, Version};
use crate::path::{relative_to_vfs, vfs_to_relative};

/// Streaming buffer size for entry bodies.
const BUF_SIZE: usize = 64 * 1024;

/// Summary of an unpack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnpackReport {
    /// Archive revision that was read.
    pub version: Version,
    /// Number of files written.
    pub files: usize,
    /// Total bytes written.
    pub bytes: u64,
    /// Directory that received the files.
    pub destination: PathBuf,
}

/// Summary of a pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackReport {
    /// Archive revision that was written.
    pub version: Version,
    /// Number of files stored.
    pub files: usize,
    /// Total body bytes stored.
    pub bytes: u64,
    /// Archive that was written.
    pub archive: PathBuf,
}

/// Removes the directory it guards unless it is disarmed.
struct CleanupOnDrop<'a> {
    path: &'a Path,
    armed: bool,
}

impl Drop for CleanupOnDrop<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(self.path);
        }
    }
}

/// Removes a temporary file unless it is disarmed.
struct TempFileGuard<'a> {
    path: &'a Path,
    armed: bool,
}

impl Drop for TempFileGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(self.path);
        }
    }
}

/// Derives the default output directory for an archive: the archive's stem next
/// to the archive itself, matching the reference tool.
pub fn default_unpack_directory(archive: &Path) -> Result<PathBuf> {
    let stem = archive.file_stem().ok_or_else(|| Error::WrongInputType {
        path: archive.to_path_buf(),
    })?;
    let parent = archive.parent().unwrap_or_else(|| Path::new(""));
    Ok(parent.join(stem))
}

/// Derives the default archive path for a directory: `<name>.pfs` in its parent.
pub fn default_archive_path(directory: &Path) -> Result<PathBuf> {
    let name = directory.file_name().ok_or_else(|| Error::WrongInputType {
        path: directory.to_path_buf(),
    })?;
    let mut archive_name = name.to_os_string();
    archive_name.push(".pfs");
    let parent = directory.parent().unwrap_or_else(|| Path::new(""));
    Ok(parent.join(archive_name))
}

/// Reads `archive` and writes every entry below `destination`.
///
/// `destination` must not exist yet; this function never merges into or
/// overwrites an existing directory.
pub fn unpack(archive: &Path, destination: &Path) -> Result<UnpackReport> {
    let mut file = File::open(archive)?;
    let archive_len = file.metadata()?.len();

    if archive_len < format::HEADER_LEN {
        return Err(Error::Truncated {
            context: "header",
            needed: format::HEADER_LEN,
            remaining: archive_len,
        });
    }

    let mut header_bytes = [0u8; format::HEADER_LEN as usize];
    file.read_exact(&mut header_bytes)?;
    let header = format::parse_header(&header_bytes)?;

    let index_end = format::INDEX_BASE + u64::from(header.index_size);
    if index_end > archive_len {
        return Err(Error::Truncated {
            context: "index",
            needed: index_end,
            remaining: archive_len,
        });
    }

    file.seek(SeekFrom::Start(format::INDEX_BASE))?;
    let mut index_bytes = vec![0u8; header.index_size as usize];
    file.read_exact(&mut index_bytes)?;

    // Everything below this point has been validated; only now do we touch the
    // filesystem.
    let index = format::parse_index(header, &index_bytes, archive_len)?;

    if destination.exists() {
        return Err(Error::OutputExists {
            path: destination.to_path_buf(),
        });
    }
    fs::create_dir_all(destination)?;
    let mut cleanup = CleanupOnDrop {
        path: destination,
        armed: true,
    };

    let key = index.key;
    let mut buffer = vec![0u8; BUF_SIZE];
    let mut files = 0usize;
    let mut total = 0u64;

    for entry in &index.entries {
        // The reference reader skips entries with a zero offset.
        if entry.offset == 0 {
            continue;
        }
        let relative = vfs_to_relative(&entry.path)?;
        let target = destination.join(&relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }

        file.seek(SeekFrom::Start(u64::from(entry.offset)))?;
        let mut out = BufWriter::new(File::create(&target)?);
        let mut remaining = u64::from(entry.size);
        let mut written = 0u64;
        while remaining > 0 {
            let take = remaining.min(BUF_SIZE as u64) as usize;
            file.read_exact(&mut buffer[..take])?;
            if let Some(key) = key.as_ref() {
                xor_chunk_in_place(&mut buffer[..take], key, written);
            }
            out.write_all(&buffer[..take])?;
            written += take as u64;
            remaining -= take as u64;
        }
        out.flush()?;
        files += 1;
        total += written;
    }

    cleanup.armed = false;
    Ok(UnpackReport {
        version: index.header.version,
        files,
        bytes: total,
        destination: destination.to_path_buf(),
    })
}

/// Packs every regular file below `directory` into a version 8 archive at
/// `output`.
///
/// `output` must not exist. Entries are sorted by their VFS path so the same
/// tree always produces the same bytes, which the reference tool does not
/// guarantee because it follows directory iteration order.
pub fn pack(directory: &Path, output: &Path) -> Result<PackReport> {
    if !directory.is_dir() {
        return Err(Error::WrongInputType {
            path: directory.to_path_buf(),
        });
    }
    if output.exists() {
        return Err(Error::OutputExists {
            path: output.to_path_buf(),
        });
    }

    let mut files = Vec::new();
    walk(directory, directory, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));

    let built = format::build_index(Version::V8, &files)?;
    let total: u64 = files.iter().map(|file| u64::from(file.size)).sum();

    let temp_path = {
        let mut name = output
            .file_name()
            .ok_or_else(|| Error::WrongInputType {
                path: output.to_path_buf(),
            })?
            .to_os_string();
        name.push(format!(".tmp{}", std::process::id()));
        output.with_file_name(name)
    };

    let mut temp = TempFileGuard {
        path: &temp_path,
        armed: true,
    };

    {
        let handle = File::create(&temp_path)?;
        let mut out = BufWriter::new(handle);
        out.write_all(&built.header.to_bytes())?;
        // `bytes` starts with the file count, which already lives in the header
        // at offset 7, so the on-disk index area starts at byte 4.
        out.write_all(&built.bytes[4..])?;

        let mut buffer = vec![0u8; BUF_SIZE];
        for file in &files {
            let relative = vfs_to_relative(&file.path)?;
            let source_path = directory.join(relative);
            let mut source = File::open(&source_path)?;
            let key = built.key;

            let mut remaining = u64::from(file.size);
            let mut written = 0u64;
            while remaining > 0 {
                let take = remaining.min(BUF_SIZE as u64) as usize;
                source.read_exact(&mut buffer[..take])?;
                if let Some(key) = key.as_ref() {
                    xor_chunk_in_place(&mut buffer[..take], key, written);
                }
                out.write_all(&buffer[..take])?;
                written += take as u64;
                remaining -= take as u64;
            }
            // Guard against a file that changed size after the index was built.
            let mut probe = [0u8; 1];
            if source.read(&mut probe)? != 0 {
                return Err(Error::Truncated {
                    context: "source file grew while packing",
                    needed: written + 1,
                    remaining: written,
                });
            }
        }

        out.flush()?;
        let handle = out
            .into_inner()
            .map_err(|err| Error::Io(err.into_error()))?;
        handle.sync_all()?;
    }

    fs::rename(&temp_path, output)?;
    temp.armed = false;

    Ok(PackReport {
        version: built.header.version,
        files: files.len(),
        bytes: total,
        archive: output.to_path_buf(),
    })
}

/// Recursively collects regular files below `root`.
fn walk(root: &Path, dir: &Path, out: &mut Vec<NewEntry>) -> Result<()> {
    let mut entries: Vec<fs::DirEntry> =
        fs::read_dir(dir)?.collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(Error::SymlinkUnsupported { path });
        }
        if file_type.is_dir() {
            walk(root, &path, out)?;
            continue;
        }
        if !file_type.is_file() {
            // Sockets, FIFOs and devices have no place in an archive.
            continue;
        }

        let relative = path.strip_prefix(root).map_err(|_| Error::InvalidPath {
            raw: path.display().to_string(),
            reason: "path is outside the packed directory",
        })?;
        let vfs = relative_to_vfs(relative)?;
        let size = u32::try_from(entry.metadata()?.len()).map_err(|_| Error::SizeOverflow)?;
        out.push(NewEntry { path: vfs, size });
    }
    Ok(())
}
