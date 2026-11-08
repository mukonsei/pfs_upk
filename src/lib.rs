//! Artemis Engine PFS archive packer and unpacker.
//!
//! The crate is split so the wire format can be tested without touching the
//! filesystem:
//!
//! - [`format`] parses and builds the index with bounded, checked reads.
//! - [`codec`] provides SHA-1 and the version 8 repeating-key XOR.
//! - [`path`] validates stored paths before they are joined onto a directory.
//! - [`archive`] streams file bodies in both directions.
//!
//! ```
//! use pfs_upk::format::{Header, Version};
//!
//! let header = Header { version: Version::V8, index_size: 20, file_count: 0 };
//! let bytes = header.to_bytes();
//! assert_eq!(pfs_upk::format::parse_header(&bytes).unwrap(), header);
//! ```

#![forbid(unsafe_code)]

pub mod archive;
pub mod codec;
pub mod error;
pub mod format;
pub mod path;

pub use archive::{
    PackReport, UnpackReport, default_archive_path, default_unpack_directory, pack, unpack,
};
pub use error::{Error, Result};
pub use format::{Entry, Header, Index, Version};
