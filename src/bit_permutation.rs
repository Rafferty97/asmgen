use std::cmp::Ordering;
use std::collections::HashMap;
use std::hint::unreachable_unchecked;
use std::ops::{BitAnd, Shr};
use std::u64;

// fixme: investigate cranelift lowering:
// - fuse shift and mask into `ubfx` or `pext`

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
    /// Pre-shift
    pub sh1: RotateOrShift,
    /// Right arithmetic shift
    pub sar: u8,
    /// Post-shift
    pub sh2: RotateOrShift,
    /// Mask
    pub and: u64,
    /// Multiply
    pub mul: u64,
    /// The operation cost
    pub cost: u8,
    /// The covering mask
    pub cover: u64,
}

impl Default for BitExtract {
    fn default() -> Self {
        Self {
            sh1: RotateOrShift::None,
            sar: 0,
            sh2: RotateOrShift::None,
            and: u64::MAX,
            mul: 1,
            cost: 0,
            cover: 0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RotateOrShift {
    None,
    ShiftLeft(u8),
    ShiftRight(u8),
    RotateLeft(u8),
}

impl RotateOrShift {
    fn new_rol(rol: u8, dst_mask: u64) -> Self {
        let rol = rol & 63;

        if rol == 0 {
            return Self::None;
        }

        let shl = Self::ShiftLeft(rol);
        if dst_mask & shl.zero_bits() == 0 {
            return shl;
        }

        let shr = Self::ShiftRight(64 - rol);
        if dst_mask & shr.zero_bits() == 0 {
            return shr;
        }

        Self::RotateLeft(rol)
    }

    fn new_ror(ror: u8, dst_mask: u64) -> Self {
        Self::new_rol(64 - ror, dst_mask)
    }

    fn new_shl(shl: u8) -> Self {
        match shl {
            0 => Self::None,
            _ => Self::ShiftLeft(shl),
        }
    }

    fn new_shr(shr: u8) -> Self {
        match shr {
            0 => Self::None,
            _ => Self::ShiftRight(shr),
        }
    }

    fn zero_bits(self) -> u64 {
        match self {
            Self::None => 0,
            Self::ShiftLeft(amt) => right_mask(amt),
            Self::ShiftRight(amt) => left_mask(amt),
            Self::RotateLeft(_) => 0,
        }
    }

    fn net_rol(self) -> i32 {
        match self {
            Self::None => 0,
            Self::ShiftLeft(shl) => shl as i32,
            Self::ShiftRight(shr) => -(shr as i32),
            Self::RotateLeft(rol) => rol as i32,
        }
    }

    fn cost(self) -> u8 {
        match self {
            Self::None => 0,
            _ => 1,
        }
    }

    fn exec(self, value: u64) -> u64 {
        match self {
            Self::None => value,
            Self::ShiftLeft(amt) => value << amt,
            Self::ShiftRight(amt) => value >> amt,
            Self::RotateLeft(amt) => value.rotate_left(amt as u32),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct ShiftExtract {
    /// Left rotation
    pub rol: u8,
    /// Destination mask
    pub dst_mask: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct RepeatExtract {
    /// Source mask
    pub src_mask: u64,
    /// Left rotation mask
    pub rol_mask: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct BroadcastExtract {
    /// Bit position being broadcast
    pub src_pos: u8,
    /// Destination mask
    pub dst_mask: u64,
}

impl BitPermutation {
    pub fn new(parts: impl IntoIterator<Item = BitPermutationPart>) -> Self {
        // Parse the input
        let mut dst_pos = 0;
        let mut fixed = 0;
        let mut rol_masks = [0; 64];

        // For each (shift, broadcast) pair, determine best feasible combination

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
        let mut shifts = vec![];
        let mut broadcasts = vec![];
        let mut repeats = vec![];

        for (src_mask, rol_mask) in groups {
            let one_bit_src = src_mask.count_ones() == 1;
            let multi_bit_dst = rol_mask.count_ones() > 1;

            if one_bit_src && multi_bit_dst {
                let src_pos = src_mask.trailing_zeros() as u8;
                let dst_mask = rol_mask.rotate_left(src_pos as u32);
                broadcasts.push(BroadcastExtract { src_pos, dst_mask });
            } else {
                for rol in SetBits(rol_mask) {
                    let dst_mask = src_mask.rotate_left(rol as u32);
                    shifts.push(ShiftExtract { rol, dst_mask });
                }
                if multi_bit_dst {
                    repeats.push(RepeatExtract { src_mask, rol_mask });
                }
            }
        }

        let mut candidates = vec![];

        for shift in &shifts {
            candidates.push(BitExtract::new_shift(*shift));
        }

        for broadcast in broadcasts {
            candidates.push(BitExtract::new_broadcast(broadcast));

            for shift in &shifts {
                candidates.extend(BitExtract::try_new_shift_broadcast(*shift, broadcast));
            }
        }

        for repeat in repeats {
            // fixme: try all plausible rotations
            candidates.push(BitExtract::new_repeat(repeat, 0));
            candidates.push(BitExtract::new_repeat(
                repeat,
                repeat.src_mask.trailing_zeros() as u8,
            ));
        }

        for candidate in &mut candidates {
            candidate.calc_cost();
            println!("{candidate:?}");
        }
        println!("----------");

        // Find minimum cost
        let extracts: Vec<_> = min_cost_cover(&candidates)
            .into_iter()
            .map(|idx| candidates[idx])
            .collect();

        for extract in &extracts {
            println!("{extract:?}");
        }
        println!("----------");

        // todo: exploit known-zero bits (will need API to express)

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
    fn new_shift(extract: ShiftExtract) -> Self {
        let ShiftExtract { rol, dst_mask } = extract;

        let sh1 = RotateOrShift::None;
        let sar = 0;
        let sh2 = RotateOrShift::new_rol(rol, dst_mask);
        let and = match dst_mask | sh2.zero_bits() {
            u64::MAX => u64::MAX,
            _ => dst_mask,
        };
        let mul = 1;
        let cost = 0;
        let cover = dst_mask;

        Self { sh1, sar, sh2, and, mul, cost, cover }
    }

    fn new_broadcast(extract: BroadcastExtract) -> Self {
        let BroadcastExtract { src_pos, dst_mask } = extract;

        // todo: find ways to elide mask?

        let sh1 = RotateOrShift::new_shl(63 - src_pos);
        let sar = (63 - dst_mask.trailing_zeros()) as u8;
        let sh2 = RotateOrShift::None;
        let and = dst_mask;
        let mul = 1;
        let cost = 0;
        let cover = dst_mask;

        Self { sh1, sar, sh2, and, mul, cost, cover }
    }

    fn new_repeat(extract: RepeatExtract, shr: u8) -> Self {
        let RepeatExtract { src_mask, rol_mask } = extract;

        let sh1 = RotateOrShift::None;
        let sar = 0;
        let sh2 = RotateOrShift::new_shr(shr);
        let and = src_mask.shr(shr);
        let mul = rol_mask.rotate_left(shr as u32);
        let cost = 0;
        let cover = and.wrapping_mul(mul);

        Self { sh1, sar, sh2, and, mul, cost, cover }
    }

    fn try_new_shift_broadcast(shift: ShiftExtract, broadcast: BroadcastExtract) -> Option<Self> {
        let ShiftExtract { rol, dst_mask: sh_dst_mask } = shift;
        let BroadcastExtract { src_pos, dst_mask: bc_dst_mask } = broadcast;

        let dst_mask = sh_dst_mask | bc_dst_mask;

        // fixme: optimise sh1 to shift if possible
        // fixme: optimise sh2 to shift if possible

        // Broadcast determines initial rotation
        let sh1 = RotateOrShift::RotateLeft(63 - src_pos);

        // Shift determines final rotation, and therefore arithmetic shift distance
        let sar_lz = bc_dst_mask
            .rotate_right((src_pos + rol) as u32)
            .leading_zeros();
        let sar = (63 - sar_lz) as u8;

        // Shift determines final rotation
        let rol = (rol as i32) - sh1.net_rol() + (sar as i32);
        let sh2 = RotateOrShift::RotateLeft((rol % 64) as u8);

        let and = dst_mask;
        let mul = 1;
        let cost = 0;
        let cover = dst_mask;

        Some(Self { sh1, sar, sh2, and, mul, cost, cover })
    }

    fn calc_cost(&mut self) {
        // fixme: determine whether mask can be elided by changing rotations to shifts
        // fixme: fix cost modelling for immediate instantiation

        let c_sh1 = self.sh1.cost();
        let c_sar = if self.sar != 0 { 1 } else { 0 };
        let c_sh2 = self.sh2.cost();
        let c_and = if self.and != u64::MAX { 1 } else { 0 };
        let c_sh2_and = match (self.sh2, is_right_mask(self.and), c_sh2 + c_and) {
            // (RotateOrShift::ShiftRight(_), true, 2..) => 1,
            _ => c_sh2 + c_and,
        };
        let c_mul = match (self.mul.count_ones(), self.mul & 1 != 0) {
            (0, _) => unsafe { unreachable_unchecked() },
            (1, true) => 0,  // no-op
            (1, false) => 1, // right shift
            (2, true) => 2,  // left shift, bitwise or
            (2, false) => 3, // two left shifts, bitwise or
            _ => 3,          // real multiply
        };
        let c_or = 1;

        self.cost = [c_sh1, c_sar, c_sh2_and, c_mul, c_or].iter().sum();
    }

    pub fn exec(self, value: u64) -> u64 {
        let value = self.sh1.exec(value);
        let value = (value as i64).shr(self.sar as u32) as u64;
        let value = self.sh2.exec(value);
        let value = value.wrapping_mul(self.mul);
        let value = value.bitand(self.and);
        value
    }
}

impl std::fmt::Debug for BitExtract {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.sh1 {
            RotateOrShift::None => write!(f, "none"),
            RotateOrShift::ShiftLeft(amt) => write!(f, "shl {amt}"),
            RotateOrShift::ShiftRight(amt) => write!(f, "shr {amt}"),
            RotateOrShift::RotateLeft(amt) => write!(f, "rol {amt}"),
        }?;
        match self.sar {
            0 => write!(f, ", none"),
            amt => write!(f, ", sar {amt}"),
        }?;
        match self.sh2 {
            RotateOrShift::None => write!(f, ", none"),
            RotateOrShift::ShiftLeft(amt) => write!(f, ", shl {amt}"),
            RotateOrShift::ShiftRight(amt) => write!(f, ", shr {amt}"),
            RotateOrShift::RotateLeft(amt) => write!(f, ", rol {amt}"),
        }?;
        match self.and {
            u64::MAX => write!(f, ", none"),
            amt => write!(f, ", and {}", PrintBinary(amt)),
        }?;
        match self.mul {
            1 => write!(f, ", none"),
            amt => write!(f, ", mul {}", PrintBinary(amt)),
        }?;
        write!(f, " (cost = {})", self.cost)?;
        write!(f, "\ncover = {}", PrintBinary(self.cover))
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

fn is_right_mask(bits: u64) -> bool {
    bits.wrapping_add(1).count_ones() <= 1
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
