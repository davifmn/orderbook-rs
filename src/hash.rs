//! The two small hash functions the book needs, written out so the crate has no
//! dependencies.

use std::hash::{BuildHasherDefault, Hasher};

/// FNV-1a, 64 bits: the hash of `Book::digest` and of the conformance test, chosen because
/// the Go implementation uses it (`hash/fnv`), so digests are comparable across languages.
#[derive(Clone, Copy, Debug)]
pub struct Fnv1a(u64);

impl Default for Fnv1a {
    fn default() -> Self {
        Fnv1a(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv1a {
    /// Feeds a `u64` as 8 little-endian bytes.
    pub fn put(&mut self, v: u64) {
        for b in v.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    /// The hash so far.
    pub const fn sum(&self) -> u64 {
        self.0
    }
}

/// Hasher for the order-id index. Order ids are integers chosen by the caller, usually
/// sequential: one multiply by a large odd constant (Fibonacci hashing) spreads them well,
/// and is far cheaper than the default `SipHash`, which defends against hash flooding by
/// untrusted keys, a threat that does not apply to ids the venue assigns itself.
#[derive(Default, Clone, Copy)]
pub struct IdHasher(u64);

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(8) ^ u64::from(b)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        }
    }

    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0 ^ v).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
}

/// `BuildHasher` for `HashMap<OrderId, _>`.
pub type BuildIdHasher = BuildHasherDefault<IdHasher>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_matches_the_reference_values() {
        // FNV-1a 64 of the empty input is the offset basis; of "a", 0xaf63dc4c8601ec8c.
        assert_eq!(Fnv1a::default().sum(), 0xcbf2_9ce4_8422_2325);
        let mut h = Fnv1a::default();
        h.0 ^= u64::from(b'a');
        h.0 = h.0.wrapping_mul(0x0100_0000_01b3);
        assert_eq!(h.sum(), 0xaf63_dc4c_8601_ec8c);
    }
}
