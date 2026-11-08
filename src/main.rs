//! Command line front end: one argument, dispatched by path type.

use std::path::PathBuf;
use std::process::ExitCode;

use pfs_upk::{Error, Result, default_archive_path, default_unpack_directory, pack, unpack};

const USAGE: &str = "\
pfs_upk ---- Artemis Engine PFS archive packer / unpacker

Usage:
  pfs_upk <ARCHIVE.pfs>   unpack into the sibling directory named after the archive
  pfs_upk <DIRECTORY>     pack the directory into <DIRECTORY>.pfs (version 8)
  pfs_upk -h | --help     show this message

Existing output is never overwritten; remove it first if you want it replaced.";

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    let mut args = std::env::args_os().skip(1);
    let Some(first) = args.next() else {
        println!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    };

    if first
        .to_str()
        .is_some_and(|arg| arg == "-h" || arg == "--help")
    {
        println!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    if args.next().is_some() {
        eprintln!("error: expected exactly one path\n");
        eprintln!("{USAGE}");
        return Ok(ExitCode::FAILURE);
    }

    let path = PathBuf::from(first);
    let metadata = std::fs::metadata(&path).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => Error::NotFound { path: path.clone() },
        _ => Error::Io(err),
    })?;

    if metadata.is_dir() {
        let archive = default_archive_path(&path)?;
        let report = pack(&path, &archive)?;
        println!(
            "Packed {} file(s), {} byte(s) into {} (version {})",
            report.files,
            report.bytes,
            report.archive.display(),
            report.version.to_byte() as char
        );
        Ok(ExitCode::SUCCESS)
    } else if metadata.is_file() {
        let destination = default_unpack_directory(&path)?;
        let report = unpack(&path, &destination)?;
        println!(
            "Extracted {} file(s), {} byte(s) to {} (version {})",
            report.files,
            report.bytes,
            report.destination.display(),
            report.version.to_byte() as char
        );
        Ok(ExitCode::SUCCESS)
    } else {
        Err(Error::WrongInputType { path })
    }
}
