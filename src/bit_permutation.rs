use std::cmp::Ordering;
use std::collections::HashMap;
use std::hint::unreachable_unchecked;
use std::ops::{BitAnd, Shl, Shr};
use std::u64;

use crate::playground::min_cost_cover;

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

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct BitExtract {
    /// A left rotation
    pub rol: u8,
    /// An arithmetic right shift
    pub sar: u8,
    /// A right rotation
    pub ror: u8,
    /// A bitwise and
    pub and: u64,
    /// A multiplication, guaranteed to have the low bit set
    pub mul: u64,
    /// Whether to substitute the left rotation with a logical shift
    pub shl: bool,
    /// Whether to substitute the right rotation with a logical shift
    pub shr: bool,
    /// The operation cost
    pub cost: u8,
    /// The covering mask
    pub cover: u64,
}

impl BitPermutation {
    pub fn new(parts: impl IntoIterator<Item = BitPermutationPart>) -> Self {
        // Parse the input
        let mut dst_pos = 0;
        let mut fixed = 0;
        let mut rol_masks = [0; 64];

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
                        let rol = dst_pos.wrapping_sub(src_pos) & 63;
                        rol_masks[rol as usize] |= make_mask(src_pos, len);
                        dst_pos += len;
                    }
                }
            }
        }

        // Save the resulting output length
        let len = dst_pos;

        // Join rotation groups with identical source masks
        let mut groups = HashMap::new();
        for (rol, src_mask) in rol_masks.into_iter().enumerate().filter(|(_, m)| *m != 0) {
            *groups.entry(src_mask).or_insert(0) |= 1u64 << rol;
        }

        // Generate candidates
        let mut candidates = vec![];
        for (src_mask, rols) in groups {
            // println!("{src_mask:064b}\t{rols:064b}");

            let one_bit_src = src_mask.count_ones() == 1;
            let multi_bit_dst = rols.count_ones() > 1;

            if one_bit_src && multi_bit_dst {
                let src_bit = src_mask.trailing_zeros() as u8;
                let dst_mask = rols.rotate_left(src_bit as u32);
                candidates.push(BitExtract::new_broadcast(src_bit, dst_mask));
            } else {
                for rol in SetBits(rols) {
                    candidates.push(BitExtract::new_rol(src_mask, rol));
                }
                if multi_bit_dst {
                    candidates.push(BitExtract::new_repeat(src_mask, rols));
                }
            }
        }

        // todo: fuse shifts and broadcasts where possible

        for candidate in &mut candidates {
            candidate.simplify();
            candidate.calc_cost();
            // println!("{candidate:?}\n{:064b}\n", candidate.cover);
        }

        let extracts = min_cost_cover(&candidates)
            .into_iter()
            .map(|idx| candidates[idx])
            .collect();

        // fixme: exploit known-zero bits (will need API to express)

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

impl Default for BitExtract {
    fn default() -> Self {
        Self {
            rol: 0,
            sar: 0,
            ror: 0,
            mul: 1,
            and: u64::MAX,
            shl: false,
            shr: false,
            cost: 0,
            cover: 0,
        }
    }
}

impl BitExtract {
    pub fn new_rol(src_mask: u64, rol: u8) -> Self {
        debug_assert_ne!(src_mask, 0);
        debug_assert!(rol < 64);

        let mask = src_mask.rotate_left(rol as u32);

        Self { rol, and: mask, cover: mask, ..Default::default() }
    }

    pub fn new_ror(src_mask: u64, ror: u8) -> Self {
        debug_assert!(ror < 64);
        Self::new_rol(src_mask, (64 - ror) % 64)
    }

    pub fn new_broadcast(src_bit: u8, dst_mask: u64) -> Self {
        debug_assert!(src_bit < 64);
        debug_assert_ne!(dst_mask, 0);

        let cover = find_smallest_cover(dst_mask);
        let rol = 63 - src_bit;
        let sar = cover.len - 1;
        let ror = 63 - sar - cover.pos;

        Self { rol, sar, ror, and: dst_mask, cover: dst_mask, ..Default::default() }
    }

    pub fn new_repeat(src_mask: u64, rols: u64) -> Self {
        let cover = find_smallest_cover(rols);
        let ror = cover.pos;
        let and = src_mask.rotate_left(ror as u32);
        let mul = rols.rotate_right(ror as u32);
        let cover = and.wrapping_mul(mul);

        Self { ror, and, mul, cover, ..Default::default() }
    }

    fn simplify(&mut self) {
        // Combine rol and ror
        if self.sar == 0 {
            let offset = u8::min(self.rol, self.ror);
            self.rol -= offset;
            self.ror -= offset;
        }

        // Reduce rotations to shifts where possible
        // if !self.shl && self.rol != 0 {
        //     let mask = self.exec(u64::MAX);
        //     // println!("shl mask = {}", PrintBinary(mask));
        //     // println!("        vs {}", PrintBinary(self.and));
        //     if self.and & mask == 0 {
        //         self.shl = true;
        //         self.and |= mask;
        //     }
        // }

        // if !self.shr && self.ror != 0 {
        //     let mask = self.exec(u64::MAX);
        //     // println!("shr mask = {}", PrintBinary(mask));
        //     // println!("        vs {}", PrintBinary(self.and));
        //     if self.and & mask == 0 {
        //         self.shr = true;
        //         self.and |= mask;
        //     }
        // }
    }

    fn calc_cost(&mut self) {
        // fixme: determine whether mask can be elided by changing rotations to shifts
        println!("{self:?}\n");

        let c_rol = if self.rol != 0 { 1 } else { 0 };
        let c_sar = if self.sar != 0 { 1 } else { 0 };
        let c_ror = if self.ror != 0 { 1 } else { 0 };
        let c_and = if self.and != !0 { 1 } else { 0 };
        let c_mul = match self.mul.count_ones() {
            0 => unsafe { unreachable_unchecked() },
            1 => 0, // no-op
            2 => 2, // left shift, bitwise or
            _ => 3, // real multiply
        };

        self.cost = [c_rol, c_sar, c_ror, c_and, c_mul].iter().sum();
    }

    pub fn exec(self, value: u64) -> u64 {
        let value = match self.shl {
            false => value.rotate_left(self.rol as u32),
            true => value.shl(self.rol as u32),
        };
        let value = (value as i64).shr(self.sar as u32) as u64;
        let value = match self.shr {
            false => value.rotate_right(self.ror as u32),
            true => value.shr(self.ror as u32),
        };
        let value = value.wrapping_mul(self.mul);
        let value = value.bitand(self.and);
        value
    }
}

impl std::fmt::Debug for BitExtract {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut sep = false;
        let mut write_sep =
            |f: &mut std::fmt::Formatter<'_>| match std::mem::replace(&mut sep, true) {
                true => write!(f, ", "),
                false => Ok(()),
            };

        if self.rol != 0 {
            write_sep(f)?;
            match self.shl {
                false => write!(f, "shl {}", self.rol)?,
                true => write!(f, "rol {}", self.rol)?,
            }
        }
        if self.sar != 0 {
            write_sep(f)?;
            write!(f, "sar {}", self.sar)?;
        }
        if self.ror != 0 {
            write_sep(f)?;
            match self.shl {
                false => write!(f, "shr {}", self.ror)?,
                true => write!(f, "ror {}", self.ror)?,
            }
        }
        if self.and != u64::MAX {
            write_sep(f)?;
            write!(f, "and {}", PrintBinary(self.and))?;
        }
        if self.mul != 1 {
            write_sep(f)?;
            write!(f, "mul {}", PrintBinary(self.mul))?;
        }
        write!(f, " (cost = {})", self.cost)
    }
}

struct PrintBinary(u64);

impl std::fmt::Display for PrintBinary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0 == 0 {
            return write!(f, "[]");
        }

        let mut parts = (0..64)
            .step_by(4)
            .rev()
            .map(|i| (self.0 >> i) & 0b1111)
            .skip_while(|b| *b == 0);

        write!(f, "[{:04b}", parts.next().unwrap())?;
        for part in parts {
            write!(f, " {:04b}", part)?;
        }
        write!(f, "]")
    }
}

fn make_mask(pos: u8, len: u8) -> u64 {
    if len == 0 {
        return 0;
    }
    ((1 << len) - 1) << pos
}

fn right_mask(len: u8) -> u64 {
    (1 << len) - 1
}

fn left_mask(len: u8) -> u64 {
    !(u64::MAX >> len)
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
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        match self.0 {
            0 => None,
            bits => {
                let pos = bits.trailing_zeros();
                self.0 &= self.0 - 1;
                Some(pos as u8)
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Cover {
    pos: u8,
    len: u8,
}

fn find_smallest_cover(value: u64) -> Cover {
    debug_assert_ne!(value, 0);

    // todo: optimise

    let lz = value.leading_zeros() as u8;
    let tz = value.trailing_zeros() as u8;
    Cover { pos: tz, len: 64 - tz - lz }
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
