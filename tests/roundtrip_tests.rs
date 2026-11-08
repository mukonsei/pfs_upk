//! Round-trip and determinism tests against the real filesystem.

mod common;

use std::fs;

use common::{TempDir, snapshot};
use pfs_upk::{Error, default_archive_path, default_unpack_directory, pack, unpack};

fn populate(root: &TempDir) {
    root.write("empty.bin", b"");
    root.write("one.bin", b"x");
    // Straddles the 20 byte XOR key boundary.
    root.write("twenty.bin", &[0xAB; 20]);
    root.write("neighbours.bin", &[0x5A; 39]);
    root.write("sub\\deep\\nested.bin", &[0x11; 512]);
    root.write("中文名称.bin", "内容".as_bytes());
    root.write("zero\\pad.bin", &[0u8; 64 * 1024]);
}

#[test]
fn roundtrip_preserves_content() {
    let tmp = TempDir::new("roundtrip");
    let source = tmp.join("source");
    fs::create_dir_all(&source).expect("create source");
    let tree = TempDir::new("roundtrip-files");
    populate(&tree);
    copy_tree(tree.path(), &source);

    let archive = tmp.join("out.pfs");
    let packed = pack(&source, &archive).expect("pack");
    assert_eq!(packed.files, 7);
    assert_eq!(packed.version, pfs_upk::Version::V8);

    let destination = tmp.join("extracted");
    let unpacked = unpack(&archive, &destination).expect("unpack");
    assert_eq!(unpacked.files, packed.files);
    assert_eq!(unpacked.bytes, packed.bytes);

    let before = snapshot(&source);
    let after = snapshot(&destination);
    assert_eq!(before.len(), after.len(), "file count");
    for (name, contents) in &before {
        let restored = after.get(name).unwrap_or_else(|| panic!("{name} missing"));
        assert_eq!(contents, restored, "contents of {name}");
    }
}

#[test]
fn empty_directory_roundtrips() {
    let tmp = TempDir::new("empty");
    let source = tmp.join("source");
    fs::create_dir_all(&source).expect("create source");

    let archive = tmp.join("empty.pfs");
    let packed = pack(&source, &archive).expect("pack");
    assert_eq!(packed.files, 0);
    assert_eq!(packed.bytes, 0);

    let destination = tmp.join("out");
    let unpacked = unpack(&archive, &destination).expect("unpack");
    assert_eq!(unpacked.files, 0);
    assert!(snapshot(&destination).is_empty());
}

#[test]
fn packing_is_deterministic() {
    let tmp = TempDir::new("deterministic");
    let source = tmp.join("source");
    fs::create_dir_all(&source).expect("create source");
    let tree = TempDir::new("deterministic-files");
    populate(&tree);
    copy_tree(tree.path(), &source);

    let first = tmp.join("first.pfs");
    let second = tmp.join("second.pfs");
    pack(&source, &first).expect("pack first");
    pack(&source, &second).expect("pack second");

    let a = fs::read(&first).expect("read first");
    let b = fs::read(&second).expect("read second");
    assert_eq!(a, b, "same tree must produce identical archives");
}

#[test]
fn bodies_are_obfuscated_in_version_8() {
    let tmp = TempDir::new("obfuscated");
    let source = tmp.join("source");
    fs::create_dir_all(&source).expect("create source");
    fs::write(source.join("secret.bin"), b"SECRET-PAYLOAD-MARKER").expect("write");

    let archive = tmp.join("secret.pfs");
    pack(&source, &archive).expect("pack");

    let bytes = fs::read(&archive).expect("read archive");
    assert!(
        !contains(&bytes, b"SECRET-PAYLOAD-MARKER"),
        "plaintext leaked into the archive body"
    );

    // The path itself is stored verbatim, so the archive is still recognizable.
    assert!(contains(&bytes, b"secret.bin"));
}

#[test]
fn refuses_to_overwrite_archive() {
    let tmp = TempDir::new("no-overwrite-archive");
    let source = tmp.join("source");
    fs::create_dir_all(&source).expect("create source");
    fs::write(source.join("a.bin"), b"a").expect("write");
    let archive = tmp.join("out.pfs");
    fs::write(&archive, b"already here").expect("write sentinel");

    let result = pack(&source, &archive);
    assert!(
        matches!(result, Err(Error::OutputExists { .. })),
        "expected OutputExists, got {result:?}"
    );
    assert_eq!(fs::read(&archive).expect("read"), b"already here");
}

#[test]
fn refuses_to_overwrite_directory() {
    let tmp = TempDir::new("no-overwrite-dir");
    let source = tmp.join("source");
    fs::create_dir_all(&source).expect("create source");
    fs::write(source.join("a.bin"), b"a").expect("write");
    let archive = tmp.join("out.pfs");
    pack(&source, &archive).expect("pack");

    let destination = tmp.join("existing");
    fs::create_dir_all(&destination).expect("create destination");
    fs::write(destination.join("keep.txt"), b"keep").expect("write sentinel");

    let result = unpack(&archive, &destination);
    assert!(
        matches!(result, Err(Error::OutputExists { .. })),
        "expected OutputExists, got {result:?}"
    );
    assert!(destination.join("keep.txt").exists());
}

#[test]
fn default_paths_match_reference_layout() {
    let archive = default_archive_path(std::path::Path::new("/data/game")).expect("default");
    assert_eq!(archive, std::path::Path::new("/data/game.pfs"));

    let destination =
        default_unpack_directory(std::path::Path::new("/data/game.pfs")).expect("default");
    assert_eq!(destination, std::path::Path::new("/data/game"));
}

#[test]
fn missing_input_is_reported() {
    let tmp = TempDir::new("missing");
    let missing = tmp.join("nope.pfs");
    assert!(unpack(&missing, &tmp.join("out")).is_err());

    let result = pack(&missing, &tmp.join("out.pfs"));
    assert!(
        matches!(result, Err(Error::WrongInputType { .. })),
        "expected WrongInputType, got {result:?}"
    );
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    for (name, contents) in snapshot(from) {
        let mut target = to.to_path_buf();
        for part in name.split('/') {
            target.push(part);
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(&target, contents).expect("write copy");
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
