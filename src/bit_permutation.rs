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

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct BitExtract {
    pub mask: u64,
    pub shift: u8,
    pub mul: u64,
}

impl BitPermutation {
    pub fn new(parts: impl IntoIterator<Item = BitPermutationPart>) -> Self {
        let mut fixed = 0;
        let mut shift_to_mask = HashMap::new();

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
                        let shift = (dst_pos as i8) - (src_pos as i8);
                        let entry = shift_to_mask.entry(shift).or_insert(0);
                        *entry |= make_mask(src_pos, len);
                        dst_pos += len;
                    }
                }
            }
        }
        let len = dst_pos;

        let mut mask_to_muls = HashMap::new();
        for (shift, mask) in shift_to_mask {
            let entry = mask_to_muls.entry(mask).or_insert(0);
            *entry |= 1 << (mask.trailing_zeros() as i8 + shift);
        }

        let mut extracts: Vec<_> = mask_to_muls
            .into_iter()
            .map(|(mask, mul)| {
                let shift = mask.trailing_zeros() as u8;
                BitExtract { mask, shift, mul }
            })
            .collect();
        extracts.sort_by_key(|e| e.mask);

        Self { len, fixed, extracts }
    }

    pub fn optimised(mut self) -> Self {
        self.optimise();
        self
    }

    pub fn optimise(&mut self) {
        for ex in &mut self.extracts {
            let shift = ex.shift.min(ex.mul.trailing_zeros() as u8);
            ex.shift -= shift;
            ex.mul >>= shift;
        }

        // todo: sign-extension optimisation
        // todo: sub trick for run of 1s
        // todo: multiply vs shift-or optimisation (popcount == 2)
        // todo: schedule OR operations as a binary tree to minimise latency
        // todo: investigate fused shift-or on aarch64
    }

    // pub fn optimise(&mut self) {
    //     // Convert repeated bit to signed shift
    //     if let Some(shift) = self
    //         .extracts
    //         .last()
    //         .filter(|e| e.src_len == 1 && e.dst_pos + e.dst_len >= self.len)
    //         .map(|e| (u64::BITS as u8 - e.dst_pos) - 1)
    //         .filter(|shift| *shift > 0)
    //     {
    //         self.fixed <<= shift;
    //         self.signed_shift += shift;
    //         self.extracts.iter_mut().for_each(|e| e.shift_left(shift));
    //     }

    //     // Merge and prune extracts
    //     for i in 0..(self.extracts.len() - 1) {
    //         let [left, right] = &mut self.extracts[i..][..2] else {
    //             unreachable!()
    //         };
    //         if let Some(merged) = left.try_merge(*right) {
    //             *left = BitExtract::default();
    //             *right = merged;
    //         }
    //     }
    //     self.extracts.retain(|e| !e.is_empty());
    // }

    pub fn exec(&self, src: u64) -> u64 {
        let mut dst = self.fixed;
        for extract in &self.extracts {
            dst |= extract.exec(src);
        }
        dst
    }
}

impl BitExtract {
    pub fn is_empty(self) -> bool {
        self.mask == 0
    }

    pub fn exec(self, src: u64) -> u64 {
        ((src & self.mask) >> self.shift) * self.mul
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
