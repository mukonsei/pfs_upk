//! SHA-1 and the repeating-key XOR used by PFS version 8.
//!
//! Version 8 stores the per-archive key material as `SHA1` over the byte range
//! `[7, 7 + index_size)`, i.e. the `file_count` field plus the whole index
//! area. File bodies are obfuscated with `data[i] ^= key[i % 20]`, which is its
//! own inverse.
//!
//! The hash is implemented here rather than pulled from a crate so the archive
//! format stays buildable offline and the dependency list stays empty.

/// Length of an SHA-1 digest, and therefore of the XOR key.
pub const KEY_LEN: usize = 20;

/// Input block size of SHA-1.
const BLOCK_LEN: usize = 64;

/// Computes the SHA-1 digest of `data`.
pub fn sha1(data: &[u8]) -> [u8; KEY_LEN] {
    let mut state: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];

    let mut chunks = data.chunks_exact(BLOCK_LEN);
    let mut block = [0u8; BLOCK_LEN];
    for chunk in &mut chunks {
        block.copy_from_slice(chunk);
        compress(&mut state, &block);
    }

    // Final block(s): data remainder, 0x80, zero padding, 64-bit big-endian
    // bit length. A remainder longer than 55 bytes needs a second block.
    let rem = chunks.remainder();
    let mut tail = [0u8; 2 * BLOCK_LEN];
    tail[..rem.len()].copy_from_slice(rem);
    tail[rem.len()] = 0x80;

    let tail_len = if rem.len() < BLOCK_LEN - 8 {
        BLOCK_LEN
    } else {
        2 * BLOCK_LEN
    };
    let bit_len = (data.len() as u64).wrapping_mul(8);
    tail[tail_len - 8..tail_len].copy_from_slice(&bit_len.to_be_bytes());

    for chunk in tail[..tail_len].chunks_exact(BLOCK_LEN) {
        block.copy_from_slice(chunk);
        compress(&mut state, &block);
    }

    let mut digest = [0u8; KEY_LEN];
    for (chunk, word) in digest.chunks_exact_mut(4).zip(state.iter()) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    digest
}

/// Processes one 64-byte block into `state`.
fn compress(state: &mut [u32; 5], block: &[u8; BLOCK_LEN]) {
    let mut w = [0u32; 80];
    for (index, chunk) in block.chunks_exact(4).enumerate() {
        w[index] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    for index in 16..80 {
        w[index] = (w[index - 3] ^ w[index - 8] ^ w[index - 14] ^ w[index - 16]).rotate_left(1);
    }

    let [mut a, mut b, mut c, mut d, mut e] = *state;
    for (index, word) in w.iter().enumerate() {
        let (f, k) = match index {
            0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
            20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
            _ => (b ^ c ^ d, 0xCA62_C1D6),
        };
        let tmp = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(*word);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = tmp;
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
}

/// Applies the repeating-key XOR in place, starting at key offset 0.
pub fn xor_in_place(data: &mut [u8], key: &[u8; KEY_LEN]) {
    xor_chunk_in_place(data, key, 0);
}

/// Applies the repeating-key XOR in place, where `start` is the offset of
/// `data[0]` within the entry. This keeps the keystream continuous when an
/// entry is streamed through several buffer-sized chunks.
pub fn xor_chunk_in_place(data: &mut [u8], key: &[u8; KEY_LEN], start: u64) {
    for (offset, byte) in data.iter_mut().enumerate() {
        let position = start.wrapping_add(offset as u64) % KEY_LEN as u64;
        *byte ^= key[position as usize];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(digest: [u8; KEY_LEN]) -> String {
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn sha1_known_vectors() {
        // FIPS 180-2 and RFC 3174 sample vectors.
        assert_eq!(hex(sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn sha1_padding_boundaries() {
        // Lengths around the point where the 64-bit length needs its own block.
        // Expected digests were produced with Python's hashlib (OpenSSL).
        let cases: [(usize, &str); 5] = [
            (55, "c1c8bbdc22796e28c0e15163d20899b65621d65a"),
            (56, "c2db330f6083854c99d4b5bfb6e8f29f201be699"),
            (64, "0098ba824b5c16427bd7a1122a5a442a25ec644d"),
            (119, "ee971065aaa017e0632a8ca6c77bb3bf8b1dfc56"),
            (120, "f34c1488385346a55709ba056ddd08280dd4c6d6"),
        ];
        for (len, expected) in cases {
            assert_eq!(hex(sha1(&[b'a'; 120][..len])), expected, "length {len}");
        }
    }

    #[test]
    fn sha1_multi_block() {
        let data = vec![b'a'; 1_000];
        assert_eq!(hex(sha1(&data)), "291e9a6c66994949b57ba5e650361e98fc36b1ba");
    }

    #[test]
    fn xor_is_its_own_inverse() {
        let key = sha1(b"key material");
        let original: Vec<u8> = (0..100u8).collect();
        let mut buffer = original.clone();
        xor_in_place(&mut buffer, &key);
        assert_ne!(buffer, original);
        xor_in_place(&mut buffer, &key);
        assert_eq!(buffer, original);
    }

    #[test]
    fn chunked_xor_matches_single_pass() {
        let key = sha1(b"key material");
        let original: Vec<u8> = (0..97u8).cycle().take(97 + 20).collect();

        let mut whole = original.clone();
        xor_in_place(&mut whole, &key);

        let mut chunked = original.clone();
        let mut start = 0u64;
        for chunk in chunked.chunks_mut(7) {
            xor_chunk_in_place(chunk, &key, start);
            start += chunk.len() as u64;
        }

        assert_eq!(whole, chunked);
    }
}
