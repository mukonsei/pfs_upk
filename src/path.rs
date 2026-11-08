//! Conversion between stored VFS paths and host paths.
//!
//! A PFS entry path is attacker controlled, so it is never joined onto the
//! output directory before being validated. Absolute paths, drive prefixes,
//! `..`, empty components, `.`, NUL and control characters are rejected, and
//! Windows device names are refused even on other platforms so an archive that
//! is safe on Linux cannot become dangerous when it is unpacked on Windows.

use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};
use crate::format::MAX_PATH_LEN;

/// Converts a stored VFS path into a relative, host-usable path.
pub fn vfs_to_relative(raw: &str) -> Result<PathBuf> {
    if raw.is_empty() {
        return Err(Error::InvalidPath {
            raw: raw.to_owned(),
            reason: "path is empty",
        });
    }
    if raw.len() > MAX_PATH_LEN as usize {
        return Err(Error::InvalidPath {
            raw: raw.to_owned(),
            reason: "path is longer than 4096 bytes",
        });
    }
    if raw.contains('\0') {
        return Err(Error::InvalidPath {
            raw: raw.to_owned(),
            reason: "path contains a NUL byte",
        });
    }

    let mut relative = PathBuf::new();
    for component in raw.split(['\\', '/']) {
        if component.is_empty() {
            return Err(Error::InvalidPath {
                raw: raw.to_owned(),
                reason: "path has an empty component (absolute path or doubled separator)",
            });
        }
        if component == "." {
            return Err(Error::InvalidPath {
                raw: raw.to_owned(),
                reason: "path contains a `.` component",
            });
        }
        if component == ".." {
            return Err(Error::InvalidPath {
                raw: raw.to_owned(),
                reason: "path escapes the output directory",
            });
        }
        if component.contains(':') {
            return Err(Error::InvalidPath {
                raw: raw.to_owned(),
                reason: "path component contains a drive or stream separator",
            });
        }
        if component.chars().any(|ch| ch.is_control()) {
            return Err(Error::InvalidPath {
                raw: raw.to_owned(),
                reason: "path component contains a control character",
            });
        }
        if is_windows_device_name(component) {
            return Err(Error::InvalidPath {
                raw: raw.to_owned(),
                reason: "path component is a reserved Windows device name",
            });
        }
        relative.push(component);
    }

    Ok(relative)
}

/// Converts a path relative to the packed directory root into a VFS path.
pub fn relative_to_vfs(path: &Path) -> Result<String> {
    let mut parts: Vec<&str> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => {
                let name = name.to_str().ok_or_else(|| Error::NonUtf8Name {
                    path: path.to_path_buf(),
                })?;
                parts.push(name);
            }
            _ => {
                return Err(Error::InvalidPath {
                    raw: path.display().to_string(),
                    reason: "path is not a plain relative path",
                });
            }
        }
    }
    if parts.is_empty() {
        return Err(Error::InvalidPath {
            raw: path.display().to_string(),
            reason: "path is empty",
        });
    }
    let vfs = parts.join("\\");
    if vfs.len() > MAX_PATH_LEN as usize {
        return Err(Error::InvalidPath {
            raw: vfs,
            reason: "path is longer than 4096 bytes",
        });
    }
    Ok(vfs)
}

/// `CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9`, `LPT1`-`LPT9`, including names
/// with an extension such as `NUL.txt`.
fn is_windows_device_name(component: &str) -> bool {
    let stem = match component.split('.').next() {
        Some(stem) => stem.to_ascii_uppercase(),
        None => return false,
    };
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    if stem.len() == 4 {
        let bytes = stem.as_bytes();
        let numbered = bytes[3].is_ascii_digit() && bytes[3] != b'0';
        if numbered && (stem.starts_with("COM") || stem.starts_with("LPT")) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_paths() {
        assert_eq!(
            vfs_to_relative("data\\script\\main.tjs").unwrap(),
            PathBuf::from("data").join("script").join("main.tjs")
        );
        assert_eq!(
            vfs_to_relative("data/script/main.tjs").unwrap(),
            PathBuf::from("data").join("script").join("main.tjs")
        );
        assert_eq!(
            vfs_to_relative("单文件.bin").unwrap(),
            PathBuf::from("单文件.bin")
        );
    }

    #[test]
    fn rejects_traversal() {
        for raw in [
            "../evil.bin",
            "a/../../evil.bin",
            "..\\evil.bin",
            "..",
            "a\\..\\b",
        ] {
            assert!(
                matches!(vfs_to_relative(raw), Err(Error::InvalidPath { .. })),
                "{raw} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_absolute_and_prefixed_paths() {
        for raw in [
            "/etc/passwd",
            "\\\\server\\share\\x",
            "C:\\Windows\\system32\\evil.dll",
            "c:/windows/evil.dll",
            "data\\\\file.bin",
            "data/",
        ] {
            assert!(
                matches!(vfs_to_relative(raw), Err(Error::InvalidPath { .. })),
                "{raw} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_nul_and_controls() {
        assert!(matches!(
            vfs_to_relative("a\0b"),
            Err(Error::InvalidPath { .. })
        ));
        assert!(matches!(
            vfs_to_relative("a\u{1}b"),
            Err(Error::InvalidPath { .. })
        ));
    }

    #[test]
    fn rejects_device_names() {
        for raw in ["CON", "nul.txt", "COM1", "LPT9\\x.bin", "aux"] {
            assert!(
                matches!(vfs_to_relative(raw), Err(Error::InvalidPath { .. })),
                "{raw} should be rejected"
            );
        }
        // Not device names.
        assert!(vfs_to_relative("COM0").is_ok());
        assert!(vfs_to_relative("console.bin").is_ok());
    }

    #[test]
    fn roundtrips_relative_paths() {
        let vfs = "data\\script\\main.tjs";
        let relative = vfs_to_relative(vfs).unwrap();
        assert_eq!(relative_to_vfs(&relative).unwrap(), vfs);
    }

    #[test]
    fn rejects_absolute_host_paths() {
        assert!(matches!(
            relative_to_vfs(Path::new("/etc/passwd")),
            Err(Error::InvalidPath { .. })
        ));
    }
}
