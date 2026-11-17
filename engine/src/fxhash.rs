//! A small, fast, non-cryptographic hasher for the engine's memo caches (the "Fx" hash of rustc
//! and Firefox: per word `h = (h.rotl(5) ^ x) * K`). The std `HashMap`'s SipHash took 13-20 % of
//! all samples in the capacity model's margin and site caches (profile 74ff26a, par-refine design
//! §8). Only for maps that are looked up and filled, never iterated where the order could reach a
//! result: the hasher changes the iteration order, not what a lookup returns. No HashDoS concern
//! (keys come from the board, not from a network). O(words of the key) per hash.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

const K: u64 = 0x51_7c_c1_b7_27_22_0a_95;

/// The Fx hasher (see the module doc).
#[derive(Default, Clone, Copy)]
pub struct FxHasher {
    hash: u64,
}

impl FxHasher {
    #[inline]
    fn add(&mut self, x: u64) {
        self.hash = (self.hash.rotate_left(5) ^ x).wrapping_mul(K);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut c = bytes.chunks_exact(8);
        for w in &mut c {
            self.add(u64::from_le_bytes(w.try_into().expect("8 bytes")));
        }
        let r = c.remainder();
        if !r.is_empty() {
            let mut b = [0u8; 8];
            b[..r.len()].copy_from_slice(r);
            self.add(u64::from_le_bytes(b));
        }
    }
    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(i as u64);
    }
    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add(i as u64);
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(i as u64);
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }
    #[inline]
    fn write_i32(&mut self, i: i32) {
        self.add(i as u32 as u64);
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

pub type FxBuild = BuildHasherDefault<FxHasher>;
pub type FxHashMap<K, V> = HashMap<K, V, FxBuild>;
pub type FxHashSet<K> = HashSet<K, FxBuild>;

/// An empty [`FxHashMap`].
pub fn map<K, V>() -> FxHashMap<K, V> {
    HashMap::with_hasher(FxBuild::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::{BuildHasher, Hash};

    fn h<T: Hash>(x: T) -> u64 {
        FxBuild::default().hash_one(x)
    }

    #[test]
    fn equal_keys_hash_equal_and_near_keys_differ() {
        assert_eq!(h((3usize, 1u8, 7i32, 1.5f64.to_bits())), h((3usize, 1u8, 7i32, 1.5f64.to_bits())));
        assert_ne!(h((3usize, 0u8)), h((3usize, 1u8)));
        assert_ne!(h((3usize, 1u8)), h((1usize, 3u8)));
        assert_ne!(h(vec![1u64, 2, 3]), h(vec![1u64, 2, 4]));
        // byte slices with a tail shorter than a word
        let a: &[u8] = &[1, 2, 3, 4, 5, 6, 7, 8, 9];
        let b: &[u8] = &[1, 2, 3, 4, 5, 6, 7, 8, 10];
        assert_ne!(h(a), h(b));
    }

    #[test]
    fn a_map_finds_what_it_holds() {
        let mut m: FxHashMap<(usize, i32), f64> = map();
        for i in 0..1000 {
            m.insert((i, -(i as i32)), i as f64);
        }
        for i in 0..1000 {
            assert_eq!(m.get(&(i, -(i as i32))), Some(&(i as f64)));
        }
        assert_eq!(m.get(&(5, 5)), None);
    }
}
