//! Cross-checks against the reference C++ implementation.
//!
//! These tests need the upstream binary. Point `PFS_UPK_CPP` at it, for example:
//!
//! ```text
//! set PFS_UPK_CPP=F:\文档\Projects\test\pfs_upk-cpp\build\pfs_upk.exe
//! cargo test --test compatibility_tests -- --nocapture
//! ```
//!
//! When the variable is unset the tests report SKIP and pass, so a machine
//! without a C++ toolchain still gets a green unit and integration run. The
//! final report must state whether they actually ran.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{TempDir, snapshot};
use pfs_upk::{pack, unpack};

fn reference_binary() -> Option<PathBuf> {
    let value = std::env::var_os("PFS_UPK_CPP")?;
    let path = PathBuf::from(value);
    if path.is_file() { Some(path) } else { None }
}

fn run_reference(binary: &Path, argument: &Path) {
    let output = Command::new(binary)
        .arg(argument)
        .output()
        .expect("run reference pfs_upk");
    assert!(
        output.status.success(),
        "reference pfs_upk failed on {}: {}{}",
        argument.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn populate(root: &Path) {
    fs::create_dir_all(root.join("sub").join("deep")).expect("create dirs");
    fs::write(root.join("zero.bin"), b"").expect("write");
    fs::write(root.join("one.bin"), b"x").expect("write");
    fs::write(root.join("boundary.bin"), vec![0x7Fu8; 20]).expect("write");
    fs::write(
        root.join("sub").join("deep").join("blob.bin"),
        vec![0x33u8; 8192],
    )
    .expect("write");
}

#[test]
fn reference_pack_reads_with_rust() {
    let Some(binary) = reference_binary() else {
        eprintln!("SKIP: set PFS_UPK_CPP to the reference binary to run this test");
        return;
    };

    let tmp = TempDir::new("cpp-pack-rust-unpack");
    let source = tmp.join("source");
    populate(&source);

    run_reference(&binary, &source);
    let archive = tmp.join("source.pfs");
    assert!(
        archive.is_file(),
        "reference did not write {}",
        archive.display()
    );

    let before = snapshot(&source);
    let destination = tmp.join("from-cpp");
    let report = unpack(&archive, &destination).expect("rust unpack of a cpp archive");
    assert_eq!(report.files, before.len());
    assert_eq!(snapshot(&destination), before);
}

#[test]
fn rust_pack_reads_with_reference() {
    let Some(binary) = reference_binary() else {
        eprintln!("SKIP: set PFS_UPK_CPP to the reference binary to run this test");
        return;
    };

    let tmp = TempDir::new("rust-pack-cpp-unpack");
    let source = tmp.join("source");
    populate(&source);

    let archive = tmp.join("rust.pfs");
    pack(&source, &archive).expect("rust pack");
    assert!(archive.is_file());

    run_reference(&binary, &archive);
    let destination = tmp.join("rust");
    assert!(
        destination.is_dir(),
        "reference did not create {}",
        destination.display()
    );

    let before = snapshot(&source);
    assert_eq!(snapshot(&destination), before);
}
