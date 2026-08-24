use std::collections::HashSet;
use std::sync::LazyLock;

use crate::util::PartialBits;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AArch64Imm {
    Zero,
    Logical { value: u64 },
    MovZ { value: u64, insts: u8 },
    MovN { value: u64, insts: u8 },
}

pub fn make_aarch64_immediate(bits: PartialBits, logical: bool) -> AArch64Imm {
    if bits.min_bits() == 0 {
        return AArch64Imm::Zero;
    }

    if logical && let Some(value) = try_make_aarch64_logical_immediate(bits) {
        return AArch64Imm::Logical { value };
    }

    let lanes = |v: u64| (0..4).map(move |i| (v >> 16 * i) & 0xffff);
    let movz_lanes = lanes(bits.min_bits()).filter(|&b| b != 0).count();
    let movn_lanes = lanes(bits.max_bits()).filter(|&b| b != 0xffff).count().max(1);

    match movn_lanes < movz_lanes {
        false => AArch64Imm::MovZ { value: bits.min_bits(), insts: movz_lanes as u8 },
        true => AArch64Imm::MovN { value: bits.max_bits(), insts: movn_lanes as u8 },
    }
}

pub fn cost_aarch64_immediate(bits: PartialBits, logical: bool) -> u16 {
    match make_aarch64_immediate(bits, logical) {
        AArch64Imm::Zero => 0,
        AArch64Imm::Logical { .. } => 0,
        AArch64Imm::MovZ { insts, .. } => insts as u16,
        AArch64Imm::MovN { insts, .. } => insts as u16,
    }
}

pub fn aarch64_logical_immediates() -> &'static HashSet<u64> {
    use crate::util::{BitSet, right_mask};

    static AARCH64_LOGICAL_IMMS: LazyLock<HashSet<u64>> = LazyLock::new(|| {
        let mut imms = HashSet::new();

        for size in [2, 4, 8, 16, 32, 64] {
            for num_ones in 1..size {
                let pattern = BitSet::new(right_mask(num_ones), size);
                let tiled = pattern.tile_u64();
                for ror in 0..size {
                    let value = tiled.rotate_right(ror as u32);
                    imms.insert(value);
                }
            }
        }

        imms.remove(&0);
        imms.remove(&u64::MAX);

        imms
    });

    &*AARCH64_LOGICAL_IMMS
}

pub fn is_aarch64_logical_immediate(value: u64) -> bool {
    try_make_aarch64_logical_immediate(value.into()).is_some()
}

pub fn try_make_aarch64_logical_immediate(mask: PartialBits) -> Option<u64> {
    let (mut zeros, mut ones) = (mask.zeros(), mask.ones());

    // Degenerate cases
    if zeros == 0 || ones == 0 {
        return None;
    }

    // Check each possible tile length
    let mut tile_mask = 1u64;
    for len in [64, 32, 16, 8, 4, 2] {
        zeros |= zeros.rotate_right(len);
        ones |= ones.rotate_right(len);
        tile_mask |= tile_mask.rotate_right(len);

        if zeros & ones != 0 {
            return None;
        }

        let mask = !0 >> (64 - len);
        let (masked_zeros, masked_ones) = (zeros & mask, ones & mask);
        let (masked_zeros, masked_ones, invert) = match masked_zeros < masked_ones {
            false => (masked_zeros, masked_ones, 0),
            true => (masked_ones, masked_zeros, !0),
        };
        let left_zeros = !0 >> masked_ones.leading_zeros();
        let right_zeros = !0 << masked_ones.trailing_zeros();
        let run = left_zeros & right_zeros;

        if masked_zeros & run == 0 {
            return Some((run * tile_mask) ^ invert);
        }
    }

    // No match found
    None
}

#[cfg(test)]
mod test {
    use crate::util::BitSet;

    use super::*;

    #[test]
    fn test_make_aarch64_immediate() {
        let bits = PartialBits::parse("");
        let expected = AArch64Imm::Zero;
        assert_eq!(make_aarch64_immediate(bits, true), expected);

        let bits = PartialBits::full(!0);
        let expected = AArch64Imm::MovN { value: !0, insts: 1 };
        assert_eq!(make_aarch64_immediate(bits, true), expected);

        let bits = PartialBits::parse("00110011 11110000 0000****");
        let expected = AArch64Imm::MovZ { value: 0b00110011_11110000_00000000, insts: 2 };
        assert_eq!(make_aarch64_immediate(bits, true), expected);

        let bits = PartialBits::parse("**000011 11111111 11111111 11****11 11111111");
        let expected = AArch64Imm::Logical { value: 0x0003_ffff_ffff };
        assert_eq!(make_aarch64_immediate(bits, true), expected);

        let bits = PartialBits::parse("00110011 111111** 11111111 11****** ****1111");
        let expected = AArch64Imm::MovN { value: 0xffff_ff33_ffff_ffff, insts: 1 };
        assert_eq!(make_aarch64_immediate(bits, true), expected);
    }

    #[test]
    fn test_aarch64_logical_immediate_count() {
        assert_eq!(aarch64_logical_immediates().len(), 5334);
    }

    #[test]
    fn test_try_make_aarch64_logical_immediate() {
        let bits = PartialBits::parse("0*1**10* *0*1**** 0*1**10*");
        let expected = Some(tile_value(0b00111100, 8));
        assert_eq!(try_make_aarch64_logical_immediate(bits), expected);

        let bits = PartialBits::parse("1*0**01* *1*0**** 1*0**01*");
        let expected = Some(tile_value(0b1100_0011, 8));
        assert_eq!(try_make_aarch64_logical_immediate(bits), expected);
    }

    #[test]
    fn test_is_aarch64_logical_immediate() {
        // Check all immediates
        for &imm in aarch64_logical_immediates().iter() {
            assert!(is_aarch64_logical_immediate(imm));
        }

        // Check all zeros and all ones
        assert!(!is_aarch64_logical_immediate(0));
        assert!(!is_aarch64_logical_immediate(!0));

        // Check all 8-bit tilings
        for bits in 0..256 {
            let value = BitSet::new(bits, 8).tile_u64();
            assert_eq!(
                is_aarch64_logical_immediate(value),
                aarch64_logical_immediates().contains(&value)
            );
        }
    }

    #[test]
    fn test_cost_aarch64_mat_imm() {
        // Zero: no nonzero lanes, movz path needs nothing (xzr).
        let bits = PartialBits::full(0);
        assert_eq!(cost_aarch64_immediate(bits, true), 0);

        // All-ones: movn path covers every lane, but the movn itself still costs 1.
        let bits = PartialBits::full(!0);
        assert_eq!(cost_aarch64_immediate(bits, true), 1);

        // Single nonzero lane, each position: one movz.
        let bits = PartialBits::full(0x0000_0000_0000_1234);
        assert_eq!(cost_aarch64_immediate(bits, true), 1);

        let bits = PartialBits::full(0x0000_0000_1234_0000);
        assert_eq!(cost_aarch64_immediate(bits, true), 1);

        let bits = PartialBits::full(0x0000_1234_0000_0000);
        assert_eq!(cost_aarch64_immediate(bits, true), 1);

        let bits = PartialBits::full(0x1234_0000_0000_0000);
        assert_eq!(cost_aarch64_immediate(bits, true), 1);

        // Single non-0xffff lane: movn + 0 movk beats movz + 3 movk.
        let bits = PartialBits::full(0xffff_ffff_ffff_1234);
        assert_eq!(cost_aarch64_immediate(bits, true), 1);

        let bits = PartialBits::full(0x1234_ffff_ffff_ffff);
        assert_eq!(cost_aarch64_immediate(bits, true), 1);

        // Two nonzero lanes (the 0x00010001 mask from the multiply-broadcast case).
        let bits = PartialBits::full(0x0000_0000_0001_0001);
        assert_eq!(cost_aarch64_immediate(bits, true), 2);

        // Two non-0xffff lanes.
        let bits = PartialBits::full(0xffff_1234_5678_ffff);
        assert_eq!(cost_aarch64_immediate(bits, true), 2);

        // Three lanes each way; neither path wins, both cost 3.
        let bits = PartialBits::full(0x1234_5678_0000_ffff);
        assert_eq!(cost_aarch64_immediate(bits, true), 3);

        // Four nonzero, four non-0xffff: 4 either way.
        let bits = PartialBits::full(0x1234_5678_9abc_def0);
        assert_eq!(cost_aarch64_immediate(bits, true), 4);

        // A logical-immediate-shaped value costs no materialisation instructions.
        let bits = PartialBits::full(0x0000_0000_00ff_ff00);
        assert_eq!(cost_aarch64_immediate(bits, true), 0);

        // A logical-immediate-shaped value that needs to be matieralised anyway.
        let bits = PartialBits::full(0x0000_0000_00ff_ff00);
        assert_eq!(cost_aarch64_immediate(bits, false), 2);

        // Everything free: min_bits == 0 (movz path costs 0), so the min is 0.
        let bits = PartialBits::full(0xdead_beef_dead_beef).with_used(0);
        assert_eq!(cost_aarch64_immediate(bits, true), 0);

        // One fully-free lane, rest constrained to zero: the free lane can go to 0.
        let bits = PartialBits::full(0).with_used(0xffff_ffff_ffff_0000);
        assert_eq!(cost_aarch64_immediate(bits, true), 0);

        // One fully-free lane, rest constrained to 0xffff: free lane goes to 0xffff,
        // so the movn path sees zero non-full lanes and costs 1.
        let bits = PartialBits::full(0xffff_ffff_ffff_ffff).with_used(0xffff_ffff_ffff_0000);
        assert_eq!(cost_aarch64_immediate(bits, true), 1);

        // Free bits *within* a lane that also has demanded ones: the lane is nonzero
        // under min_bits regardless, so it still needs a movz.
        let bits = PartialBits::full(0x0000_0000_0000_1234).with_used(0x0000_0000_0000_ff00);
        assert_eq!(cost_aarch64_immediate(bits, true), 1);

        // Free bits within a lane whose demanded bits are all zero: min_bits clears
        // the lane entirely, so it costs nothing on the movz path.
        let bits = PartialBits::full(0).with_used(0x0000_0000_0000_00ff);
        assert_eq!(cost_aarch64_immediate(bits, true), 0);

        // Freedom lets the movn path win: three lanes are demanded-0xffff, the
        // fourth is free and resolves to 0xffff.
        let bits = PartialBits::full(0xffff_ffff_ffff_0000).with_used(0xffff_ffff_ffff_0000);
        assert_eq!(cost_aarch64_immediate(bits, true), 1);

        // Partial freedom that can't rescue either path: two lanes demand a mix of
        // ones and zeros, so both min_bits and max_bits leave them dirty.
        let bits = PartialBits::full(0x0000_1234_5678_0000).with_used(0x0000_ffff_ffff_0000);
        assert_eq!(cost_aarch64_immediate(bits, true), 2);
    }

    fn tile_value(mut bits: u64, mut len: u32) -> u64 {
        while len < 64 {
            bits |= bits << len;
            len <<= 1;
        }
        bits
    }
}
