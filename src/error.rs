//! Error type shared by the parser, the codec and the archive layer.

use std::fmt;
use std::io;
use std::path::PathBuf;

/// Everything that can go wrong while reading or writing a PFS archive.
///
/// The variants are deliberately specific: a corrupted archive must produce a
/// diagnosable error instead of a panic or a silent short read.
#[derive(Debug)]
pub enum Error {
    /// An I/O operation failed.
    Io(io::Error),
    /// The first two bytes are not the `pf` magic.
    NotArtemis,
    /// The version byte is not one of `2`, `6`, `8`.
    UnsupportedVersion(u8),
    /// `index_size` claims more bytes than the configured limit or the file.
    IndexTooLarge { declared: u64, limit: u64 },
    /// `file_count` claims more entries than the configured limit allows.
    EntryCountTooLarge { declared: u64, limit: u64 },
    /// A read ran past the end of its buffer or the archive.
    Truncated {
        context: &'static str,
        needed: u64,
        remaining: u64,
    },
    /// A stored path length does not fit into the index area.
    PathLengthOutOfRange { declared: u32, remaining: u64 },
    /// The stored path escapes the output directory or contains illegal parts.
    InvalidPath { raw: String, reason: &'static str },
    /// The same archive path appears twice.
    DuplicateEntry { path: String },
    /// An entry points outside the archive body.
    DataOutOfRange {
        path: String,
        offset: u64,
        size: u64,
        archive_len: u64,
    },
    /// The reconstructed index cannot be represented as `u32` offsets.
    SizeOverflow,
    /// The output path already exists; this tool never overwrites.
    OutputExists { path: PathBuf },
    /// The input is a directory where a file was expected, or vice versa.
    WrongInputType { path: PathBuf },
    /// The input path does not exist at all.
    NotFound { path: PathBuf },
    /// A file name cannot be stored as UTF-8, which the format requires.
    NonUtf8Name { path: PathBuf },
    /// A symbolic link was found inside the input tree.
    SymlinkUnsupported { path: PathBuf },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(err) => write!(f, "I/O error: {err}"),
            Error::NotArtemis => write!(f, "not a PFS archive: missing `pf` magic"),
            Error::UnsupportedVersion(byte) => write!(
                f,
                "unsupported PFS version {byte:#04x} (expected `2`, `6` or `8`)"
            ),
            Error::IndexTooLarge { declared, limit } => write!(
                f,
                "index size {declared} exceeds the accepted maximum of {limit} bytes"
            ),
            Error::EntryCountTooLarge { declared, limit } => write!(
                f,
                "file count {declared} exceeds the accepted maximum of {limit} entries"
            ),
            Error::Truncated {
                context,
                needed,
                remaining,
            } => write!(
                f,
                "truncated archive while reading {context}: need {needed} bytes, {remaining} available"
            ),
            Error::PathLengthOutOfRange {
                declared,
                remaining,
            } => write!(
                f,
                "entry path length {declared} is larger than the {remaining} bytes left in the index"
            ),
            Error::InvalidPath { raw, reason } => {
                write!(f, "unsafe archive path {raw:?}: {reason}")
            }
            Error::DuplicateEntry { path } => {
                write!(f, "duplicate archive path {path:?}")
            }
            Error::DataOutOfRange {
                path,
                offset,
                size,
                archive_len,
            } => write!(
                f,
                "entry {path:?} spans {offset}..{} but the archive is {archive_len} bytes",
                offset.saturating_add(*size)
            ),
            Error::SizeOverflow => write!(
                f,
                "archive size does not fit into the 32-bit offsets used by the format"
            ),
            Error::OutputExists { path } => {
                write!(f, "refusing to overwrite existing path {}", path.display())
            }
            Error::WrongInputType { path } => write!(
                f,
                "unexpected input type for {}: pass a `.pfs` file to unpack or a directory to pack",
                path.display()
            ),
            Error::NotFound { path } => {
                write!(f, "no such file or directory: {}", path.display())
            }
            Error::NonUtf8Name { path } => write!(
                f,
                "cannot store non-UTF-8 file name {} in a PFS archive",
                path.display()
            ),
            Error::SymlinkUnsupported { path } => write!(
                f,
                "symbolic link {} is not supported while packing",
                path.display()
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::Io(err)
    }
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;
