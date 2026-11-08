# PFS archive format

All integers are little endian. The header is 11 bytes.

```text
0x00  'p' 'f'          magic
0x02  '2' | '6' | '8'  version
0x03  u32              index_size
0x07  u32              file_count
0x0B  ...              index area
```

`index_size` counts the bytes from offset 7, so it covers the `file_count`
field plus the index area, and the first file body starts at `7 + index_size`.

## Index entry

```text
u32   path_len
bytes path          UTF-8, `\` separated
u32   reserved
u32   offset        absolute offset of the body
u32   size          body length
```

## Offset table

Follows the entries:

```text
u32   file_count + 1
repeat file_count times:
      u32 position  relative to offset 7
      u32 reserved
u32   0
u32   0
u32   table_position   relative to offset 7
```

The declared count is one larger than the number of records: the final
"record" is the two zero words that close the table.

## Obfuscation

Version 8 XORs every body with a key derived from
`SHA1(bytes[7 .. 7 + index_size])`:

```text
data[i] ^= key[i % 20]
```

Versions 2 and 6 store bodies verbatim.

## Reader behaviour

Entries whose `offset` is `0` are skipped, matching the reference reader.

## Differences from the C++ original

These are deliberate; each fixes a defect, not a style preference.

- Reads are bounded. The original allocated `MAX_PATH` bytes and then read an
  archive-controlled length into it (`unpack.cpp:81`), a heap overflow. Path
  lengths and index sizes are validated before any allocation or read.
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
