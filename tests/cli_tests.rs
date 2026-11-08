//! End-to-end tests that drive the built binary.

mod common;

use std::fs;
use std::process::Command;

use common::{TempDir, snapshot};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_pfs_upk")
}

#[test]
fn no_arguments_prints_usage_and_succeeds() {
    let output = Command::new(binary()).output().expect("run binary");
    assert!(output.status.success(), "usage must exit successfully");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"), "stdout was {stdout}");
}

#[test]
fn missing_path_reports_not_found() {
    let tmp = TempDir::new("cli-missing");
    let output = Command::new(binary())
        .arg(tmp.join("nope.pfs"))
        .output()
        .expect("run binary");
    assert!(!output.status.success(), "missing input must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no such file or directory"),
        "stderr was {stderr}"
    );
}

#[test]
fn too_many_arguments_fail() {
    let output = Command::new(binary())
        .args(["a", "b"])
        .output()
        .expect("run binary");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("exactly one path"));
}

#[test]
fn pack_then_unpack_via_cli() {
    let tmp = TempDir::new("cli-roundtrip");
    let source = tmp.join("game_data");
    fs::create_dir_all(source.join("sub")).expect("create dirs");
    fs::write(source.join("hello.txt"), b"hello artemis").expect("write");
    fs::write(source.join("sub").join("blob.bin"), vec![0x42u8; 300]).expect("write");

    let packed = Command::new(binary())
        .arg(&source)
        .output()
        .expect("run pack");
    assert!(
        packed.status.success(),
        "pack failed: {}",
        String::from_utf8_lossy(&packed.stderr)
    );

    let archive = tmp.join("game_data.pfs");
    assert!(archive.is_file());
    let before = snapshot(&source);

    // The archive's stem collides with the source directory, so move it aside
    // the same way a user would.
    let moved = tmp.join("original");
    fs::rename(&source, &moved).expect("rename source");

    let unpacked = Command::new(binary())
        .arg(&archive)
        .output()
        .expect("run unpack");
    assert!(
        unpacked.status.success(),
        "unpack failed: {}",
        String::from_utf8_lossy(&unpacked.stderr)
    );

    assert_eq!(snapshot(&source), before);
    assert_eq!(snapshot(&moved), before);
}

#[test]
fn second_pack_refuses_to_overwrite() {
    let tmp = TempDir::new("cli-overwrite");
    let source = tmp.join("data");
    fs::create_dir_all(&source).expect("create dir");
    fs::write(source.join("a.bin"), b"a").expect("write");

    let first = Command::new(binary()).arg(&source).output().expect("run");
    assert!(first.status.success());

    let second = Command::new(binary()).arg(&source).output().expect("run");
    assert!(!second.status.success(), "must refuse to overwrite");
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("refusing to overwrite"),
        "stderr was {stderr}"
    );
}
