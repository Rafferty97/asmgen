use std::ops::{BitAnd, BitOr, Shl, Shr};

use crate::util::mask::right_mask;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BitSet {
    /// The bits in the bit set, stored in the least significant bits of the word.
    /// All unused bits are cleared.
    bits: u64,
    /// The length of the bit set, between `0` and `64`.
    len: u8,
}

impl BitSet {
    pub fn new(bits: u64, len: usize) -> Self {
        let len = len.min(64) as u8;
        let bits = bits & right_mask::<u64>(len);
        Self { bits, len }
    }

    /// Repeats this pattern to fill a 64-bit word.
    pub fn tile_u64(self) -> u64 {
        let Self { mut bits, mut len } = self;
        while len < 64 {
            bits |= bits << len;
            len <<= 1;
        }
        bits
    }
}

/// A bitset, where some of the bits may be unused and thus free to assume any value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PartialBits {
    /// The bitset. Any unused bits must be set to 1.
    bits: u64,
    /// The used bits.
    used: u64,
}

impl Default for PartialBits {
    fn default() -> Self {
        Self { bits: u64::MAX, used: u64::MAX }
    }
}

impl PartialBits {
    pub const ZERO: Self = Self::full(0);

    pub const MAX: Self = Self::full(u64::MAX);

    pub fn new(bits: u64, used: u64) -> Self {
        debug_assert_eq!(!bits & !used, 0);
        Self { bits, used }
    }

    pub const fn full(bits: u64) -> Self {
        Self { bits, used: u64::MAX }
    }

    pub fn empty() -> Self {
        Self { bits: u64::MAX, used: 0 }
    }

    pub fn with_used(self, used: u64) -> Self {
        Self::new(self.bits | !used, self.used & used)
    }

    pub fn bits(self) -> u64 {
        self.bits
    }

    pub fn used(self) -> u64 {
        self.used
    }

    pub fn ones(self) -> u64 {
        self.bits & self.used
    }

    pub fn zeros(self) -> u64 {
        !self.bits
    }

    pub fn unused(self) -> u64 {
        !self.used
    }

    pub fn min_bits(self) -> u64 {
        self.bits & self.used
    }

    pub fn max_bits(self) -> u64 {
        self.bits
    }

    pub fn contains(self, value: u64) -> bool {
        (value | !self.used) == self.bits
    }
}

impl From<u64> for PartialBits {
    fn from(value: u64) -> Self {
        Self::full(value)
    }
}

impl BitOr for PartialBits {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        let bits = self.bits | rhs.bits;
        let used = self.ones() | rhs.ones() | (self.used & rhs.used);
        Self::new(bits, used)
    }
}

impl BitAnd for PartialBits {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        let bits = self.bits & rhs.bits;
        let used = !bits | (self.used & rhs.used);
        Self::new(bits, used)
    }
}

impl Shl<u8> for PartialBits {
    type Output = Self;

    fn shl(self, rhs: u8) -> Self {
        Self::new(self.bits << rhs, !(!self.used << rhs))
    }
}

impl Shr<u8> for PartialBits {
    type Output = Self;

    fn shr(self, rhs: u8) -> Self {
        Self::new(self.bits >> rhs, !(!self.used >> rhs))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn partial_bits_bitor() {
        let a = PartialBits::full(0b000_111_111).with_used(0b111_111_000);
        let b = PartialBits::full(0b011_011_011).with_used(0b110_110_110);
        let c = PartialBits::full(0b011_111_111).with_used(0b110_111_010);
        assert_eq!(a | b, c);

        assert!(c.contains(a.min_bits() | b.min_bits()));
        assert!(c.contains(a.min_bits() | b.max_bits()));
        assert!(c.contains(a.max_bits() | b.min_bits()));
        assert!(c.contains(a.max_bits() | b.max_bits()));
    }

    #[test]
    fn partial_bits_bitand() {
        let a = PartialBits::full(0b000_111_111).with_used(0b111_111_000);
        let b = PartialBits::full(0b011_011_011).with_used(0b110_110_110);
        let c = PartialBits::full(0b000_011_011).with_used(0b111_110_100);
        assert_eq!(a & b, c);

        assert!(c.contains(a.min_bits() & b.min_bits()));
        assert!(c.contains(a.min_bits() & b.max_bits()));
        assert!(c.contains(a.max_bits() & b.min_bits()));
        assert!(c.contains(a.max_bits() & b.max_bits()));
    }
}
