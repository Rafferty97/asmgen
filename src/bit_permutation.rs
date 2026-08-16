use std::{cmp::Ordering, collections::HashMap};

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BitPermutation {
    pub len: u8,
    pub fixed: u64,
    pub extracts: Vec<BitExtract>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BitPermutationPart {
    Fixed { len: u8, bits: u64 },
    Slice { len: u8, src_pos: u8, repeats: u8 },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BitExtract {
    LeftShift { mask: u64, lshift: u32 },
    RightShiftMul { mask: u64, rshift: u32, mul: u64 },
    SignExtend { lshift: u32, rshift: u32, mask: u64 },
}

impl BitPermutation {
    pub fn new(parts: impl IntoIterator<Item = BitPermutationPart>) -> Self {
        let mut fixed = 0;
        let mut shifts = [0; 64];

        let mut dst_pos = 0;
        for part in parts.into_iter() {
            match part {
                BitPermutationPart::Fixed { len, bits } => {
                    fixed |= bits << dst_pos;
                    dst_pos += len;
                }
                BitPermutationPart::Slice { len, src_pos, repeats } => {
                    if len == 0 {
                        continue;
                    }
                    for _ in 0..repeats {
                        let shift = dst_pos.wrapping_sub(src_pos) & 63;
                        shifts[shift as usize] |= make_mask(src_pos, len);
                        dst_pos += len;
                    }
                }
            }
        }
        let len = dst_pos;

        let mut groups = HashMap::new();
        for (shift, mask) in shifts.into_iter().enumerate() {
            *groups.entry(mask).or_insert(0) |= 1u64 << shift;
        }

        let mut extracts = vec![];
        for (src_mask, shifts) in groups {
            if src_mask == 0 {
                continue;
            }
            println!("{src_mask:064b}\t{shifts:064b}");
            match (src_mask.count_ones(), shifts.count_ones()) {
                (0, _) | (_, 0) => {}
                (1, 2..) => {
                    let lshift = src_mask.leading_zeros();
                    let mut mask = shifts.rotate_left(src_mask.trailing_zeros());
                    let rshift = 63 - mask.trailing_zeros();
                    extracts.push(BitExtract::SignExtend { lshift, rshift, mask });
                }
                (_, 3..) => {
                    let mask = src_mask;
                    let mut rshift = src_mask.trailing_zeros();
                    let mut mul = shifts.rotate_left(rshift);
                    if rshift <= mul.trailing_zeros() {
                        mul >>= rshift;
                        rshift = 0;
                    }
                    extracts.push(BitExtract::RightShiftMul { mask, rshift, mul });
                }
                _ => {
                    let mask = src_mask;
                    let max_lshift = src_mask.leading_zeros();
                    for lshift in SetBits(shifts) {
                        extracts.push(match lshift > max_lshift {
                            false => BitExtract::LeftShift { mask, lshift },
                            true => BitExtract::RightShiftMul { mask, rshift: 64 - lshift, mul: 1 },
                        });
                    }
                }
            }
        }

        for extract in &mut extracts {
            match extract {
                BitExtract::SignExtend { lshift, rshift, mask } => {
                    *mask |= (1 << lshift.saturating_sub(*rshift)) - 1;
                }
                _ => {}
            }
        }

        Self { len, fixed, extracts }
    }

    pub fn exec(&self, src: u64) -> u64 {
        let mut dst = self.fixed;
        for extract in &self.extracts {
            dst |= extract.exec(src);
        }
        dst
    }
}

impl BitExtract {
    pub fn exec(self, src: u64) -> u64 {
        match self {
            Self::LeftShift { mask, lshift } => (src & mask) << lshift,
            Self::RightShiftMul { mask, rshift, mul } => ((src & mask) >> rshift) * mul,
            Self::SignExtend { lshift, rshift, mask } => {
                ((((src as i64) << lshift) >> rshift) as u64) & mask
            }
        }
    }
}

fn make_mask(pos: u8, len: u8) -> u64 {
    if len == 0 {
        return 0;
    }
    ((1 << len) - 1) << pos
}

fn shift_bits<P>(bits: u64, src_pos: P, dst_pos: P) -> u64
where
    P: std::cmp::Ord + std::ops::Sub<P, Output = P>,
    u64: std::ops::Shl<P, Output = u64> + std::ops::Shr<P, Output = u64>,
{
    match src_pos.cmp(&dst_pos) {
        Ordering::Equal => bits,
        Ordering::Less => bits << (dst_pos - src_pos),
        Ordering::Greater => bits >> (src_pos - dst_pos),
    }
}

struct SetBits(u64);

impl Iterator for SetBits {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        match self.0 {
            0 => None,
            bits => {
                let pos = bits.trailing_zeros();
                self.0 &= self.0 - 1;
                Some(pos)
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_basic_permutation() {
        let permutation = BitPermutation::new([
            BitPermutationPart::Fixed { len: 2, bits: 0b11 },
            BitPermutationPart::Slice { len: 3, src_pos: 5, repeats: 2 },
            BitPermutationPart::Slice { len: 2, src_pos: 5, repeats: 1 },
        ]);

        println!("{permutation:?}");

        assert_eq!(permutation.exec(0b00_000_00000), 0b00_000_000_11);
        assert_eq!(permutation.exec(0b11_111_11111), 0b11_111_111_11);
        assert_eq!(permutation.exec(0b00_010_01010), 0b10_010_010_11);
        assert_eq!(permutation.exec(0b11_101_10101), 0b01_101_101_11);
        assert_eq!(permutation.exec(0b01_001_11000), 0b01_001_001_11);
        assert_eq!(permutation.exec(0b10_110_00111), 0b10_110_110_11);
    }

    #[test]
    fn test_permutation_repeat() {
        let p1 = [BitPermutationPart::Slice { len: 3, src_pos: 5, repeats: 2 }];
        let p2 = [
            BitPermutationPart::Slice { len: 3, src_pos: 5, repeats: 1 },
            BitPermutationPart::Slice { len: 3, src_pos: 5, repeats: 1 },
        ];
        assert_eq!(BitPermutation::new(p1), BitPermutation::new(p2));
    }

    #[test]
    fn test_permutation_mask_merge() {
        let p1 = [BitPermutationPart::Slice { len: 10, src_pos: 4, repeats: 1 }];
        let p2 = [
            BitPermutationPart::Slice { len: 5, src_pos: 4, repeats: 1 },
            BitPermutationPart::Slice { len: 5, src_pos: 9, repeats: 1 },
        ];
        assert_eq!(BitPermutation::new(p1), BitPermutation::new(p2));
    }

    // #[test]
    // fn test_riscv_immediates() {
    //     // Decode permutations
    //     let i_imm = BitPermutation::new([
    //         BitPermutationPart::Slice { len: 1, src_pos: 20, repeats: 1 },
    //         BitPermutationPart::Slice { len: 4, src_pos: 21, repeats: 1 },
    //         BitPermutationPart::Slice { len: 6, src_pos: 25, repeats: 1 },
    //         BitPermutationPart::Slice { len: 1, src_pos: 31, repeats: 21 },
    //     ]);
    //     let i_imm = i_imm.optimised();
    // }
}
