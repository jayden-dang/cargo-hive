//! Fixed-length bitset used for liveness marks.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitSet {
    len: usize,
    words: Vec<u64>,
}

impl BitSet {
    pub fn with_len(n: usize) -> BitSet {
        BitSet {
            len: n,
            words: vec![0; n.div_ceil(64)],
        }
    }

    pub fn insert(&mut self, i: usize) -> bool {
        assert!(i < self.len, "bitset index {i} past length {}", self.len);
        let word = i / 64;
        let mask = 1u64 << (i % 64);
        let was_clear = self.words[word] & mask == 0;
        self.words[word] |= mask;
        was_clear
    }

    pub fn contains(&self, i: usize) -> bool {
        assert!(i < self.len, "bitset index {i} past length {}", self.len);
        let word = i / 64;
        let mask = 1u64 << (i % 64);
        self.words[word] & mask != 0
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::BitSet;

    #[test]
    fn insert_sets_word_boundaries_and_rejects_a_second_set() {
        let len = 128usize;
        let bits = [0usize, 63, 64, len - 1];
        let mut set = BitSet::with_len(len);
        for &bit in &bits {
            assert!(set.insert(bit), "first insert of {bit}");
            assert!(!set.insert(bit), "second insert of {bit}");
        }
        for i in 0..len {
            assert_eq!(set.contains(i), bits.contains(&i), "bit {i}");
        }
    }
}
