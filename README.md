# pfs_upk ---- An Artemis Engine archive (un)packer in Rust

Rust port of [https://github.com/nextgal/pfs_upk](https://github.com/nextgal/pfs_upk).
## HOW to use
`pfs_upk <FILENAME.pfs>` for unpack.

`pfs_upk <PATH>` for pack.

Existing output is never overwritten; remove it first to replace it.
## HOW to build
```
cargo build --release
```
## HOW to test
```
cargo test --all-targets
```

The cross-implementation tests skip unless `PFS_UPK_CPP` points at the C++ binary.
## Compatibility
`2`, `6` and `8` unpack. Pack always writes version `8`.

Only Windows x86_64-msvc is verified. Version 2 and 6 archives from a real game
have not been exercised here, so those paths rest on synthetic tests.

The wire format and the deliberate differences from the C++ original are in
[docs/FORMAT.md](docs/FORMAT.md).

## License

GNU GPL V3

```
pfs_upk is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

pfs_upk is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.

You should have received a copy of the GNU General Public License along with pfs_upk. If not, see <https://www.gnu.org/licenses/>.
```

## Acknowledgements

[https://github.com/nextgal/pfs_upk](https://github.com/nextgal/pfs_upk)

SHA-1 is implemented in `src/codec.rs`, so the crate has no dependencies.
