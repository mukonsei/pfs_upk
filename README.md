# pfs_upk (Rust)

Artemis Engine PFS archive packer / unpacker — a Rust reimplementation of
[`nextgal/pfs_upk`](https://github.com/nextgal/pfs_upk) (commit
`abdffcbeb3c733ce234aa99ed42b206d13aaed2f`).

The command line contract is unchanged: one argument, dispatch by path type.

```text
pfs_upk <ARCHIVE.pfs>   unpack into the sibling directory named after the archive
pfs_upk <DIRECTORY>     pack the directory into <DIRECTORY>.pfs (version 8)
pfs_upk -h | --help     usage
```

Existing output is never overwritten. Remove the old archive or directory
first; the tool will not merge into it or silently replace it.

```powershell
cargo build --release
.\target\release\pfs_upk.exe .\game_data        # writes .\game_data.pfs
.\target\release\pfs_upk.exe .\game_data.pfs    # writes .\game_data\
```

## Compatibility

Unpacking supports versions `2`, `6` and `8`. Packing always writes version 8,
which is the only version the reference implementation emits.

Version 8 obfuscates every file body with a key derived from
`SHA1(bytes[7 .. 7 + index_size])` and XORs with `data[i] ^= key[i % 20]`.
Versions 2 and 6 store bodies verbatim.

Verified on Windows (x86_64-pc-windows-msvc, Rust 1.99.1). The code is portable
but has not been run on Linux or macOS.

Not verified, and stated as such on purpose:

- Version 2 and 6 archives from a real game have not been exercised. The index
  layout is shared with version 8 and the offset table is optional when parsing,
  but the absence of samples means these paths are only covered by synthetic
  tests.
- The reference C++ binary was not available on the build machine, so the
  cross-implementation tests skip. Run them where it is present, see below.

## Format

All integers are little endian. The header is 11 bytes; `index_size` counts the
bytes from offset 7, which means it covers the `file_count` field plus the index
area, and the first body starts at `7 + index_size`.

```text
0x00  'p' 'f'          magic
0x02  '2' | '6' | '8'  version
0x03  u32              index_size
0x07  u32              file_count
0x0B  ...              index area
```

Each index entry is `u32 path_len`, `path` (UTF-8, `\` separated), `u32
reserved`, `u32 offset`, `u32 size`. The offset table follows the entries:
`u32 file_count + 1`, one `(u32 position, u32 reserved)` pair per file, two zero
words, then `u32 table_position`. Both positions are relative to offset 7. The
declared count is one larger than the number of records because the final
"record" is the two zero words that close the table.

Entries whose `offset` is `0` are skipped on unpack, matching the reference
reader.

## Behaviour differences from the C++ original

These are deliberate; each fixes a real defect, not a style preference.

- Reads are bounded. The original allocated `MAX_PATH` bytes and then read an
  archive-controlled length into it (`unpack.cpp:81`), a heap overflow. Path
  lengths and index sizes are now validated before any allocation or read.
- `file_count` and `index_size` are capped (1,000,000 entries, 64 MiB index)
  before allocating, so a corrupt field cannot exhaust memory.
- Stored paths are validated before use: absolute paths, drive prefixes, `..`,
  empty components, `.`, NUL, control characters and Windows device names are
  rejected. The original joined archive paths directly onto the output
  directory, which permitted path traversal (`unpack.cpp:135`).
- An unpack validates the whole index before creating anything on disk, and
  removes the directory it created if a later step fails.
- A pack writes to a sibling temporary file, syncs, then renames, so a failure
  never leaves a partial archive under the final name.
- Packing sorts entries by VFS path, so the same tree always produces identical
  bytes. The original followed directory iteration order and was not
  reproducible.
- Duplicate stored paths are a hard error instead of a silent overwrite.
- Symbolic links inside the packed tree are rejected instead of being followed.
- Errors propagate to a non-zero exit code with a typed message.

## Testing

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Test layout:

- `src/codec.rs` — SHA-1 vectors (FIPS 180-2 and padding boundaries computed
  with OpenSSL) and XOR round-trip / chunked-keystream equivalence.
- `src/format.rs` — hand-verified golden index bytes, header validation, table
  shape, key coverage, rejection of absurd counts.
- `src/path.rs` — traversal, absolute paths, device names, round-trip.
- `tests/roundtrip_tests.rs` — real filesystem round-trip, determinism,
  obfuscation, overwrite refusal.
- `tests/malformed_tests.rs` — truncated, hostile and inconsistent archives,
  including a check that a rejected archive leaves nothing behind.
- `tests/compatibility_tests.rs` — cross-implementation checks against the C++
  binary; skipped unless `PFS_UPK_CPP` points at it.

To run the compatibility tests, build the reference project and set:

```powershell
$env:PFS_UPK_CPP = "F:\path\to\pfs_upk.exe"
cargo test --test compatibility_tests -- --nocapture
```

## Dependencies

None. SHA-1 is implemented in `src/codec.rs` so the crate builds offline with no
third-party supply chain. `#![forbid(unsafe_code)]` is enforced crate-wide.

## License

GPL-3.0-or-later, inherited from the original project. See `LICENSE`.
