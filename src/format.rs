//! Binary layout of an Artemis PFS archive.
//!
//! All integers are little endian. The header is 11 bytes:
//!
//! ```text
//! 0x00  'p' 'f'          magic
//! 0x02  '2' | '6' | '8'  version
//! 0x03  u32              index_size
//! 0x07  u32              file_count
//! 0x0B  ...              index area
//! ```
//!
//! `index_size` counts the bytes from offset 7 up to the start of the file
//! data, so the index area itself is `index_size - 4` bytes long and the first
//! file body starts at `7 + index_size`. `file_count` is the first four bytes
//! of that covered range, which is why the size is four bytes longer than the
//! index area.
//!
//! Each index entry is `u32 path_len`, `path` (UTF-8, `\` separated), `u32
//! reserved`, `u32 offset`, `u32 size`. Entries are followed by the offset
//! table: `u32 file_count + 1`, one `(u32 position, u32 reserved)` pair per
//! file, two zero words, and `u32 table_position`. Both positions are relative
//! to offset 7.
//!
//! Version 8 additionally obfuscates every file body with a key derived from
//! `SHA1` over `[7, 7 + index_size)`; versions 2 and 6 store bodies verbatim.
//!
//! Parsing never trusts a length from the file: every field is bounded by the
//! remaining input, and all arithmetic is checked.

use std::collections::HashSet;

use crate::codec::{KEY_LEN, sha1};
use crate::error::{Error, Result};

/// Two byte archive magic.
pub const MAGIC: [u8; 2] = *b"pf";
/// Length of the fixed header.
pub const HEADER_LEN: u64 = 11;
/// Offset of the first byte covered by `index_size`.
pub const INDEX_BASE: u64 = 7;
/// Upper bound accepted for `index_size`; keeps a corrupt field from exhausting
/// memory before anything is written to disk.
pub const MAX_INDEX_SIZE: u64 = 64 * 1024 * 1024;
/// Upper bound accepted for `file_count`.
pub const MAX_FILE_COUNT: u64 = 1_000_000;
/// Upper bound accepted for a single stored path.
pub const MAX_PATH_LEN: u32 = 4096;
/// Per-entry fixed cost beyond the path bytes: length, reserved, offset, size.
const ENTRY_FIXED: u64 = 16;
/// Offset table trailer: entry count, end-of-table marker, table position.
const TABLE_TRAILER: u64 = 16;

/// Archive revision stored in byte 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    /// Version `'2'`, bodies stored verbatim.
    V2,
    /// Version `'6'`, bodies stored verbatim.
    V6,
    /// Version `'8'`, bodies XOR-obfuscated with an SHA-1 derived key.
    V8,
}

impl Version {
    /// Decodes the version byte.
    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            b'2' => Some(Version::V2),
            b'6' => Some(Version::V6),
            b'8' => Some(Version::V8),
            _ => None,
        }
    }

    /// Encodes the version byte.
    pub const fn to_byte(self) -> u8 {
        match self {
            Version::V2 => b'2',
            Version::V6 => b'6',
            Version::V8 => b'8',
        }
    }

    /// Whether file bodies are XOR-obfuscated.
    pub const fn is_obfuscated(self) -> bool {
        matches!(self, Version::V8)
    }
}

/// Parsed fixed header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Archive revision.
    pub version: Version,
    /// Bytes covered by the index, counted from offset 7.
    pub index_size: u32,
    /// Number of index entries.
    pub file_count: u32,
}

impl Header {
    /// Absolute offset of the first file body.
    pub fn data_start(&self) -> u64 {
        INDEX_BASE + u64::from(self.index_size)
    }

    /// Serialises the header into its 11 wire bytes.
    pub fn to_bytes(self) -> [u8; HEADER_LEN as usize] {
        let mut out = [0u8; HEADER_LEN as usize];
        out[0..2].copy_from_slice(&MAGIC);
        out[2] = self.version.to_byte();
        out[3..7].copy_from_slice(&self.index_size.to_le_bytes());
        out[7..11].copy_from_slice(&self.file_count.to_le_bytes());
        out
    }
}

/// One file stored in the archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// VFS path as stored, using `\` separators.
    pub path: String,
    /// Absolute offset of the body.
    pub offset: u32,
    /// Body length in bytes.
    pub size: u32,
}

/// One offset-table record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableEntry {
    /// Position of the entry's `reserved` field, relative to offset 7.
    pub entry_pos: u32,
    /// Reserved word, written as zero by the reference implementation.
    pub reserved: u32,
}

/// Trailing offset table. Optional because versions 2 and 6 may not carry one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffsetTable {
    /// Declared entry count, normally `file_count + 1`.
    pub count: u32,
    /// One record per file.
    pub entries: Vec<TableEntry>,
    /// Position of the table header, relative to offset 7.
    pub table_pos: u32,
}

/// Fully parsed index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    /// Fixed header.
    pub header: Header,
    /// File entries in stored order.
    pub entries: Vec<Entry>,
    /// Present only when a well-formed table trails the entries.
    pub table: Option<OffsetTable>,
    /// XOR key for version 8 archives.
    pub key: Option<[u8; KEY_LEN]>,
}

impl Index {
    /// Entries that actually carry a body. The reference reader skips entries
    /// whose offset is zero, and this implementation keeps that behaviour.
    pub fn stored_entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|entry| entry.offset != 0)
    }
}

/// Bounded reader over the index bytes that reports what it needed.
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Cursor { data, pos: 0 }
    }

    fn remaining(&self) -> u64 {
        (self.data.len() - self.pos) as u64
    }

    fn take(&mut self, len: usize, context: &'static str) -> Result<&'a [u8]> {
        if len as u64 > self.remaining() {
            return Err(Error::Truncated {
                context,
                needed: len as u64,
                remaining: self.remaining(),
            });
        }
        let slice = &self.data[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    fn u32(&mut self, context: &'static str) -> Result<u32> {
        let bytes = self.take(4, context)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}

/// Parses the 11-byte header.
pub fn parse_header(bytes: &[u8]) -> Result<Header> {
    if bytes.len() < HEADER_LEN as usize {
        return Err(Error::Truncated {
            context: "header",
            needed: HEADER_LEN,
            remaining: bytes.len() as u64,
        });
    }
    if bytes[0..2] != MAGIC {
        return Err(Error::NotArtemis);
    }
    let version = Version::from_byte(bytes[2]).ok_or(Error::UnsupportedVersion(bytes[2]))?;
    let header = Header {
        version,
        index_size: u32::from_le_bytes([bytes[3], bytes[4], bytes[5], bytes[6]]),
        file_count: u32::from_le_bytes([bytes[7], bytes[8], bytes[9], bytes[10]]),
    };
    if u64::from(header.index_size) > MAX_INDEX_SIZE {
        return Err(Error::IndexTooLarge {
            declared: u64::from(header.index_size),
            limit: MAX_INDEX_SIZE,
        });
    }
    if u64::from(header.file_count) > MAX_FILE_COUNT {
        return Err(Error::EntryCountTooLarge {
            declared: u64::from(header.file_count),
            limit: MAX_FILE_COUNT,
        });
    }
    Ok(header)
}

/// Parses the whole index. `index_bytes` is the on-disk range
/// `[7, 7 + index_size)`, so its length must equal `header.index_size` and its
/// first four bytes are the `file_count` field.
pub fn parse_index(header: Header, index_bytes: &[u8], archive_len: u64) -> Result<Index> {
    if index_bytes.len() as u64 != u64::from(header.index_size) {
        return Err(Error::Truncated {
            context: "index",
            needed: u64::from(header.index_size),
            remaining: index_bytes.len() as u64,
        });
    }
    if u64::from(header.index_size) < 4 {
        return Err(Error::Truncated {
            context: "index",
            needed: 4,
            remaining: u64::from(header.index_size),
        });
    }
    if header.data_start() > archive_len {
        return Err(Error::Truncated {
            context: "index",
            needed: header.data_start(),
            remaining: archive_len,
        });
    }

    // The index area proper starts after the count field, at file offset 11.
    let mut cursor = Cursor::new(&index_bytes[4..]);
    let file_count = header.file_count as usize;
    let mut entries: Vec<Entry> = Vec::with_capacity(file_count.min(1024));
    let mut seen: HashSet<String> = HashSet::with_capacity(file_count.min(1024));

    for _ in 0..header.file_count {
        let path_len = cursor.u32("entry path length")?;
        if path_len > MAX_PATH_LEN {
            return Err(Error::PathLengthOutOfRange {
                declared: path_len,
                remaining: cursor.remaining(),
            });
        }
        let raw = cursor.take(path_len as usize, "entry path")?;
        let path = std::str::from_utf8(raw)
            .map_err(|_| Error::InvalidPath {
                raw: String::from_utf8_lossy(raw).into_owned(),
                reason: "path is not valid UTF-8",
            })?
            .to_owned();
        // The reserved word is not meaningfully constrained by the format; the
        // reference reader skips it and so do we, but it must be present.
        let _reserved = cursor.u32("entry reserved field")?;
        let offset = cursor.u32("entry offset")?;
        let size = cursor.u32("entry size")?;

        if !seen.insert(path.clone()) {
            return Err(Error::DuplicateEntry { path });
        }
        entries.push(Entry { path, offset, size });
    }

    let mut table = None;
    if cursor.remaining() >= 4 {
        // The declared count is one larger than the number of records: the last
        // "record" is the two zero words that close the table, which is why the
        // reference writer emits `file_count` pairs followed by an eight byte
        // end marker.
        let count = cursor.u32("offset table count")?;
        if count == header.file_count.saturating_add(1) {
            let records_len = u64::from(count) - 1;
            let needed = records_len * 8 + 12;
            if cursor.remaining() >= needed {
                let mut records = Vec::with_capacity((records_len as usize).min(1024));
                for _ in 0..records_len {
                    let entry_pos = cursor.u32("offset table entry")?;
                    let reserved = cursor.u32("offset table reserved field")?;
                    records.push(TableEntry {
                        entry_pos,
                        reserved,
                    });
                }
                let _end_marker = cursor.take(8, "offset table end marker")?;
                let table_pos = cursor.u32("offset table position")?;
                table = Some(OffsetTable {
                    count,
                    entries: records,
                    table_pos,
                });
            }
        }
    }

    let data_start = header.data_start();
    for entry in &entries {
        if entry.offset == 0 {
            continue;
        }
        let offset = u64::from(entry.offset);
        let size = u64::from(entry.size);
        let end = offset
            .checked_add(size)
            .ok_or_else(|| Error::DataOutOfRange {
                path: entry.path.clone(),
                offset,
                size,
                archive_len,
            })?;
        if offset < data_start || end > archive_len {
            return Err(Error::DataOutOfRange {
                path: entry.path.clone(),
                offset,
                size,
                archive_len,
            });
        }
    }

    let key = if header.version.is_obfuscated() {
        Some(sha1(index_bytes))
    } else {
        None
    };

    Ok(Index {
        header,
        entries,
        table,
        key,
    })
}

/// One file to be written into a new archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEntry {
    /// VFS path, already using `\` separators.
    pub path: String,
    /// Body length in bytes.
    pub size: u32,
}

/// Serialised index produced by [`build_index`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltIndex {
    /// Header to write first.
    pub header: Header,
    /// Bytes for `[7, 7 + index_size)`, i.e. the count field plus index area.
    pub bytes: Vec<u8>,
    /// XOR key for the archive version being written.
    pub key: Option<[u8; KEY_LEN]>,
}

/// Builds a version 8 index for `files`, in the given order.
///
/// The layout matches the reference implementation byte for byte, including
/// the fact that `index_size` covers the `file_count` field.
pub fn build_index(version: Version, files: &[NewEntry]) -> Result<BuiltIndex> {
    let file_count = u32::try_from(files.len()).map_err(|_| Error::EntryCountTooLarge {
        declared: files.len() as u64,
        limit: u32::MAX as u64,
    })?;

    let mut index_area: u64 = TABLE_TRAILER;
    for file in files {
        let path_len = u64::try_from(file.path.len()).map_err(|_| Error::SizeOverflow)?;
        if path_len > u64::from(MAX_PATH_LEN) {
            return Err(Error::PathLengthOutOfRange {
                declared: u32::try_from(file.path.len()).unwrap_or(u32::MAX),
                remaining: u64::from(MAX_PATH_LEN),
            });
        }
        index_area = index_area
            .checked_add(path_len + ENTRY_FIXED)
            .ok_or(Error::SizeOverflow)?;
    }
    // Offset-table records plus their two reserved words each.
    index_area = index_area
        .checked_add(u64::from(file_count) * 8)
        .ok_or(Error::SizeOverflow)?;

    let index_size = u32::try_from(index_area + 4).map_err(|_| Error::SizeOverflow)?;
    let header = Header {
        version,
        index_size,
        file_count,
    };

    let mut data_start = header.data_start();
    let mut bytes: Vec<u8> = Vec::with_capacity(index_area as usize + 4);
    bytes.extend_from_slice(&file_count.to_le_bytes());

    // Entry positions are relative to offset 7, which is four bytes before the
    // first entry, hence the +4.
    let mut positions: Vec<u32> = Vec::with_capacity(files.len());
    for file in files {
        let path_len = u32::try_from(file.path.len()).map_err(|_| Error::SizeOverflow)?;
        bytes.extend_from_slice(&path_len.to_le_bytes());
        bytes.extend_from_slice(file.path.as_bytes());
        positions.push(u32::try_from(bytes.len()).map_err(|_| Error::SizeOverflow)? + 4);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(
            &u32::try_from(data_start)
                .map_err(|_| Error::SizeOverflow)?
                .to_le_bytes(),
        );
        bytes.extend_from_slice(&file.size.to_le_bytes());
        data_start = data_start
            .checked_add(u64::from(file.size))
            .ok_or(Error::SizeOverflow)?;
    }

    let table_pos = u32::try_from(bytes.len()).map_err(|_| Error::SizeOverflow)? + 4;
    bytes.extend_from_slice(&file_count.saturating_add(1).to_le_bytes());
    for position in &positions {
        bytes.extend_from_slice(&position.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
    }
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&table_pos.to_le_bytes());

    debug_assert_eq!(bytes.len() as u64, index_area + 4);
    debug_assert_eq!(bytes.len() as u64, u64::from(header.index_size));

    let key = if version.is_obfuscated() {
        Some(sha1(&bytes))
    } else {
        None
    };

    Ok(BuiltIndex { header, bytes, key })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_files() -> Vec<NewEntry> {
        vec![
            NewEntry {
                path: "a.bin".to_owned(),
                size: 3,
            },
            NewEntry {
                path: "sub\\b.bin".to_owned(),
                size: 0,
            },
        ]
    }

    #[test]
    fn header_roundtrip() {
        let header = Header {
            version: Version::V8,
            index_size: 0x40,
            file_count: 2,
        };
        let bytes = header.to_bytes();
        assert_eq!(bytes.len(), 11);
        assert_eq!(parse_header(&bytes).unwrap(), header);
        assert_eq!(bytes, [b'p', b'f', b'8', 0x40, 0, 0, 0, 2, 0, 0, 0]);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = Header {
            version: Version::V8,
            index_size: 4,
            file_count: 0,
        }
        .to_bytes();
        bytes[0] = b'x';
        assert!(matches!(parse_header(&bytes), Err(Error::NotArtemis)));
    }

    #[test]
    fn rejects_unknown_version() {
        let mut bytes = Header {
            version: Version::V8,
            index_size: 4,
            file_count: 0,
        }
        .to_bytes();
        bytes[2] = b'9';
        assert!(matches!(
            parse_header(&bytes),
            Err(Error::UnsupportedVersion(b'9'))
        ));
    }

    #[test]
    fn rejects_absurd_counts_before_allocating() {
        let mut bytes = Header {
            version: Version::V8,
            index_size: 4,
            file_count: 0,
        }
        .to_bytes();
        bytes[3..7].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            parse_header(&bytes),
            Err(Error::IndexTooLarge { .. })
        ));

        let mut bytes = Header {
            version: Version::V8,
            index_size: 4,
            file_count: 0,
        }
        .to_bytes();
        bytes[7..11].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            parse_header(&bytes),
            Err(Error::EntryCountTooLarge { .. })
        ));
    }

    #[test]
    fn built_index_roundtrips() {
        let files = sample_files();
        let built = build_index(Version::V8, &files).unwrap();
        assert_eq!(built.bytes.len(), built.header.index_size as usize);

        let mut archive = built.header.to_bytes().to_vec();
        archive.extend_from_slice(&built.bytes[4..]);
        let archive_len = archive.len() as u64 + 3;
        archive.extend_from_slice(&[9, 9, 9]);

        let index = parse_index(built.header, &built.bytes, archive_len).unwrap();
        assert_eq!(index.header, built.header);
        assert_eq!(index.entries.len(), 2);
        assert_eq!(index.entries[0].path, "a.bin");
        assert_eq!(index.entries[0].offset, built.header.index_size + 7);
        assert_eq!(index.entries[0].size, 3);
        assert_eq!(index.entries[1].path, "sub\\b.bin");
        assert_eq!(index.entries[1].offset, built.header.index_size + 10);
        assert_eq!(index.key, built.key);
    }

    #[test]
    fn golden_index_bytes() {
        // Two files, 3 and 0 bytes. Bytes verified by hand against the reference
        // layout: index_size = entries (46) + count (4) + table (16) + end
        // marker (8) + table position (4) + the count field itself (4).
        let built = build_index(Version::V8, &sample_files()).unwrap();
        assert_eq!(built.header.index_size, 0x52);
        assert_eq!(built.bytes.len(), 0x52);
        assert_eq!(built.header.data_start(), 0x59);

        let expected: Vec<u8> = vec![
            // file_count = 2
            0x02, 0x00, 0x00, 0x00, //
            // entry 0: path_len = 5, "a.bin"
            0x05, 0x00, 0x00, 0x00, b'a', b'.', b'b', b'i', b'n', //
            // reserved, offset (0x59), size (3)
            0x00, 0x00, 0x00, 0x00, 0x59, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, //
            // entry 1: path_len = 9, "sub\b.bin"
            0x09, 0x00, 0x00, 0x00, b's', b'u', b'b', b'\\', b'b', b'.', b'b', b'i', b'n', //
            // reserved, offset (0x5c), size (0)
            0x00, 0x00, 0x00, 0x00, 0x5c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
            // offset table: declared count = 3
            0x03, 0x00, 0x00, 0x00, //
            // (position 0x11, reserved), (position 0x2a, reserved)
            0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
            0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
            // end marker, table position (0x36)
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x36, 0x00, 0x00, 0x00,
        ];
        assert_eq!(built.bytes, expected);
    }

    #[test]
    fn table_entries_have_expected_shape() {
        let built = build_index(Version::V8, &sample_files()).unwrap();
        let index = parse_index(built.header, &built.bytes, 1_000).unwrap();
        let table = index.table.expect("table present");
        // The declared count includes the end marker, so records = count - 1.
        assert_eq!(table.count, 3);
        assert_eq!(table.entries.len(), 2);
        assert_eq!(table.table_pos, 0x36);
        assert_eq!(table.entries[0].entry_pos, 0x11);
        assert_eq!(table.entries[1].entry_pos, 0x2a);
    }

    #[test]
    fn version_8_key_covers_count_and_index() {
        let built = build_index(Version::V8, &sample_files()).unwrap();
        assert_eq!(built.key, Some(sha1(&built.bytes)));

        let built = build_index(Version::V6, &sample_files()).unwrap();
        assert_eq!(built.key, None);
    }

    #[test]
    fn rejects_path_length_past_index_end() {
        let header = Header {
            version: Version::V8,
            index_size: 8,
            file_count: 1,
        };
        let mut bytes = header.file_count.to_le_bytes().to_vec();
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            parse_index(header, &bytes, 1_000),
            Err(Error::PathLengthOutOfRange { .. })
        ));
    }

    #[test]
    fn rejects_entry_pointing_past_archive() {
        let files = vec![NewEntry {
            path: "big.bin".to_owned(),
            size: 100,
        }];
        let built = build_index(Version::V8, &files).unwrap();
        // Archive claims one byte past the end of the body.
        let archive_len = built.header.data_start() + 99;
        assert!(matches!(
            parse_index(built.header, &built.bytes, archive_len),
            Err(Error::DataOutOfRange { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_paths() {
        let files = vec![
            NewEntry {
                path: "same.bin".to_owned(),
                size: 1,
            },
            NewEntry {
                path: "same.bin".to_owned(),
                size: 1,
            },
        ];
        let built = build_index(Version::V8, &files).unwrap();
        assert!(matches!(
            parse_index(built.header, &built.bytes, 10_000),
            Err(Error::DuplicateEntry { .. })
        ));
    }

    #[test]
    fn empty_archive_roundtrips() {
        let built = build_index(Version::V8, &[]).unwrap();
        assert_eq!(built.header.file_count, 0);
        assert_eq!(built.header.index_size, 20);
        let index = parse_index(built.header, &built.bytes, built.header.data_start()).unwrap();
        assert!(index.entries.is_empty());
        assert_eq!(index.table.unwrap().count, 1);
    }
}
