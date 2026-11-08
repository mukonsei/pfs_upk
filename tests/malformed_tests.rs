//! Corrupt, hostile and truncated archives.
//!
//! Every case must produce a typed error and must not leave anything behind on
//! disk, which is the whole point of validating the index before writing.

mod common;

use std::fs;
use std::path::Path;

use common::TempDir;
use pfs_upk::codec;
use pfs_upk::format::{self, NewEntry, Version};
use pfs_upk::{Error, unpack};

/// Builds a well-formed version 8 archive for `entries`.
fn build(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let files: Vec<NewEntry> = entries
        .iter()
        .map(|(path, data)| NewEntry {
            path: (*path).to_owned(),
            size: data.len() as u32,
        })
        .collect();
    let built = format::build_index(Version::V8, &files).expect("build index");
    let key = built.key.expect("version 8 key");

    let mut archive = Vec::from(built.header.to_bytes());
    archive.extend_from_slice(&built.bytes[4..]);
    for (_, data) in entries {
        let mut body = data.to_vec();
        codec::xor_in_place(&mut body, &key);
        archive.extend_from_slice(&body);
    }
    archive
}

fn write(tmp: &TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = tmp.join(name);
    fs::write(&path, bytes).expect("write archive");
    path
}

fn expect_unpack_error(archive: &Path, destination: &Path, predicate: fn(&Error) -> bool) {
    let result = unpack(archive, destination);
    match result {
        Err(err) if predicate(&err) => {}
        other => panic!("expected the documented error, got {other:?}"),
    }
    assert!(
        !destination.exists(),
        "a rejected archive must not create {}",
        destination.display()
    );
}

#[test]
fn empty_file_is_rejected() {
    let tmp = TempDir::new("empty-archive");
    let archive = write(&tmp, "empty.pfs", b"");
    expect_unpack_error(&archive, &tmp.join("out"), |err| {
        matches!(err, Error::Truncated { .. })
    });
}

#[test]
fn short_header_is_rejected() {
    let tmp = TempDir::new("short-header");
    let archive = write(&tmp, "short.pfs", b"pf8");
    expect_unpack_error(&archive, &tmp.join("out"), |err| {
        matches!(err, Error::Truncated { .. })
    });
}

#[test]
fn bad_magic_is_rejected() {
    let tmp = TempDir::new("bad-magic");
    let mut bytes = build(&[("a.bin", b"a")]);
    bytes[0] = b'X';
    let archive = write(&tmp, "magic.pfs", &bytes);
    expect_unpack_error(&archive, &tmp.join("out"), |err| {
        matches!(err, Error::NotArtemis)
    });
}

#[test]
fn unknown_version_is_rejected() {
    let tmp = TempDir::new("bad-version");
    let mut bytes = build(&[("a.bin", b"a")]);
    bytes[2] = b'9';
    let archive = write(&tmp, "version.pfs", &bytes);
    expect_unpack_error(&archive, &tmp.join("out"), |err| {
        matches!(err, Error::UnsupportedVersion(b'9'))
    });
}

#[test]
fn absurd_index_size_is_rejected_before_allocating() {
    let tmp = TempDir::new("huge-index");
    let mut bytes = build(&[("a.bin", b"a")]);
    bytes[3..7].copy_from_slice(&u32::MAX.to_le_bytes());
    let archive = write(&tmp, "huge-index.pfs", &bytes);
    expect_unpack_error(&archive, &tmp.join("out"), |err| {
        matches!(err, Error::IndexTooLarge { .. })
    });
}

#[test]
fn absurd_file_count_is_rejected() {
    let tmp = TempDir::new("huge-count");
    let mut bytes = build(&[("a.bin", b"a")]);
    bytes[7..11].copy_from_slice(&u32::MAX.to_le_bytes());
    let archive = write(&tmp, "huge-count.pfs", &bytes);
    expect_unpack_error(&archive, &tmp.join("out"), |err| {
        matches!(err, Error::EntryCountTooLarge { .. })
    });
}

#[test]
fn truncated_index_is_rejected() {
    let tmp = TempDir::new("truncated-index");
    let mut bytes = build(&[("a.bin", b"a")]);
    bytes.truncate(13);
    let archive = write(&tmp, "truncated.pfs", &bytes);
    expect_unpack_error(&archive, &tmp.join("out"), |err| {
        matches!(err, Error::Truncated { .. })
    });
}

#[test]
fn entry_past_end_of_archive_is_rejected() {
    let tmp = TempDir::new("past-end");
    let mut bytes = build(&[("a.bin", b"0123456789")]);
    // Drop part of the body: the declared size no longer fits.
    bytes.truncate(bytes.len() - 4);
    let archive = write(&tmp, "past-end.pfs", &bytes);
    expect_unpack_error(&archive, &tmp.join("out"), |err| {
        matches!(err, Error::DataOutOfRange { .. })
    });
}

#[test]
fn traversal_paths_are_rejected_and_cleaned_up() {
    let tmp = TempDir::new("traversal");
    let bytes = build(&[("../escaped.bin", b"payload")]);
    let archive = write(&tmp, "traversal.pfs", &bytes);

    expect_unpack_error(&archive, &tmp.join("out"), |err| {
        matches!(err, Error::InvalidPath { .. })
    });
    assert!(
        !tmp.join("escaped.bin").exists(),
        "path traversal must not write outside the destination"
    );
}

#[test]
fn absolute_paths_are_rejected() {
    let tmp = TempDir::new("absolute");
    let bytes = build(&[("/etc/escaped.bin", b"payload")]);
    let archive = write(&tmp, "absolute.pfs", &bytes);
    expect_unpack_error(&archive, &tmp.join("out"), |err| {
        matches!(err, Error::InvalidPath { .. })
    });
}

#[test]
fn duplicate_entries_are_rejected() {
    let tmp = TempDir::new("duplicate");
    let bytes = build(&[("same.bin", b"a"), ("same.bin", b"b")]);
    let archive = write(&tmp, "duplicate.pfs", &bytes);
    expect_unpack_error(&archive, &tmp.join("out"), |err| {
        matches!(err, Error::DuplicateEntry { .. })
    });
}

#[test]
fn zero_offset_entries_are_skipped() {
    let tmp = TempDir::new("zero-offset");
    let mut bytes = build(&[("a.bin", b"aaa")]);
    // Blank the offset of the single entry: the reference reader skips such
    // entries, so unpack must succeed without writing the file.
    let index_start = format::HEADER_LEN as usize;
    let offset_field = index_start + 4 + 5 + 4;
    bytes[offset_field..offset_field + 4].copy_from_slice(&0u32.to_le_bytes());

    let archive = write(&tmp, "zero.pfs", &bytes);
    let destination = tmp.join("out");
    let report = unpack(&archive, &destination).expect("unpack");
    assert_eq!(report.files, 0);
    assert!(!destination.join("a.bin").exists());
}

#[test]
fn valid_archive_from_bytes_helper_still_unpacks() {
    // Control case: the helper used by the negative tests produces a working
    // archive, so a failure above is caused by the mutation and not the harness.
    let tmp = TempDir::new("control");
    let bytes = build(&[("ok.bin", b"hello"), ("sub/ok2.bin", b"world")]);
    let archive = write(&tmp, "ok.pfs", &bytes);
    let destination = tmp.join("out");
    let report = unpack(&archive, &destination).expect("unpack");
    assert_eq!(report.files, 2);
    assert_eq!(
        fs::read(destination.join("ok.bin")).expect("read"),
        b"hello".to_vec()
    );
    assert_eq!(
        fs::read(destination.join("sub").join("ok2.bin")).expect("read"),
        b"world".to_vec()
    );
}
