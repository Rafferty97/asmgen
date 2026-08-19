use std::cell::Cell;
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::u64;

use fnv::FnvHashMap;
use itertools::Itertools;
use smallvec::SmallVec;

use crate::bit_utils::is_aarch64_logical_immediate;
use crate::partial_bits::PartialBits;
use crate::playground::min_cost_cover;
use crate::print::PrintBits;

static ISA: Isa = Isa::AArch64;

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct BitPermutation {
    len: u8,
    fixed: u64,
    rot_masks: FnvHashMap<u8, u64>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BitPermutationPart {
    /// A fixed bit pattern
    Fixed { len: u8, bits: u64 },
    /// A contiguous slice of bits from the input
    Slice { len: u8, src_pos: u8 },
    /// A single input bit, repeated `len` times
    Repeat { len: u8, src_pos: u8 },
}

impl BitPermutationPart {
    pub fn trunc(self, max_len: u8) -> Self {
        match self {
            Self::Fixed { len, bits } => Self::Fixed { len: len.min(max_len), bits },
            Self::Slice { len, src_pos } => Self::Slice { len: len.min(max_len), src_pos },
            Self::Repeat { len, src_pos } => Self::Repeat { len: len.min(max_len), src_pos },
        }
    }
}

#[derive(Clone)]
pub struct BitExtract {
    /// The sequence of operations
    ops: SmallVec<[BitOp; 4]>,
    /// The destination bits that are written to;
    /// the other bits are guaranteed to always be zero
    dst_bits: u64,
    /// The total cost of the operations
    cost: Cell<u16>,
    /// The origin of this bit extract
    #[cfg(debug_assertions)]
    origin: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BitOp {
    /// No operation.
    Nop,
    /// Left shift.
    ShiftLeft(u8),
    /// Logical right shift.
    ShiftRight(u8),
    /// Arithmetic right shift.
    ArithRight(u8),
    /// Right rotation.
    RotateRight(u8),
    /// Bitwise AND.
    /// It is an invariant that `!mask & !used == 0`.
    And(PartialBits),
    /// Two shifts followed by a bitwise or;
    /// the smaller shift preceeds the larger shift, and may be zero.
    ShiftOr(u8, u8),
    /// Integer multiplication.
    Mul(u64),
}

/// Known and demanded bits.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct KDBits {
    /// Bits in the input known to be zero.
    pub zeros: u64,
    /// Bits in the output that are used.
    pub used: u64,
}

impl BitExtract {
    /// Creates an empty `BitExtract` that returns its input untransformed.
    pub fn new() -> Self {
        Default::default()
    }

    /// Annotates this `BitExtract` with origin information in debug builds.
    pub fn with_origin(mut self, origin: impl FnOnce() -> String) -> Self {
        #[cfg(debug_assertions)]
        {
            self.origin = origin();
        }
        self
    }

    pub fn shl(mut self, amt: u8) -> Self {
        self.push(BitOp::ShiftLeft(amt));
        self
    }

    pub fn shr(mut self, amt: u8) -> Self {
        self.push(BitOp::ShiftRight(amt));
        self
    }

    pub fn sar(mut self, amt: u8) -> Self {
        self.push(BitOp::ArithRight(amt));
        self
    }

    pub fn rol(mut self, amt: u8) -> Self {
        self.push(BitOp::RotateRight(0u8.wrapping_sub(amt) % 64));
        self
    }

    pub fn ror(mut self, amt: u8) -> Self {
        self.push(BitOp::RotateRight(amt % 64));
        self
    }

    pub fn and(mut self, mask: u64) -> Self {
        self.push(BitOp::And(PartialBits::full(mask)));
        self
    }

    pub fn mul(mut self, mask: u64) -> Self {
        self.push(BitOp::Mul(mask));
        self
    }

    /// Pushes an operation.
    pub fn push(&mut self, op: BitOp) {
        self.ops.push(op);
        self.dst_bits = op.apply(self.dst_bits);
    }

    pub fn optimised(mut self) -> Self {
        self.optimise();
        self
    }

    /// Optimises the `BitExtract` by fusing operations where possible.
    pub fn optimise(&mut self) {
        let mut kd_bits = Vec::<KDBits>::new();

        // println!("ORIGINAL {}", self.origin);
        // for &op in &self.ops {
        //     println!("    {op}");
        // }

        loop {
            self.ops.retain(|op| *op != BitOp::Nop);
            kd_bits.resize(self.ops.len(), Default::default());

            // Forward pass
            let mut zeros = 0;
            for (index, &op) in self.ops.iter().enumerate() {
                kd_bits[index].zeros = zeros;
                zeros = op.calc_known_zeros(zeros);
            }

            // Reverse pass
            let mut used = u64::MAX;
            for (index, &op) in self.ops.iter().enumerate().rev() {
                kd_bits[index].used = used;
                used = op.calc_used_bits(used);
            }

            // Fuse instructions
            let mut changed = false;
            for i in 0..self.ops.len().saturating_sub(1) {
                let [op1, op2] = self.ops.get_disjoint_mut([i, i + 1]).unwrap();
                if let Some(fused) = BitOp::try_fuse(*op1, *op2) {
                    *op1 = BitOp::Nop;
                    *op2 = fused;
                    changed = true;
                }
            }

            // Optimise instructions
            for (op, &kd_bits) in self.ops.iter_mut().zip(&kd_bits) {
                let optimised = op.optimise(kd_bits);
                changed |= optimised != *op;
                *op = optimised;
            }
            // fixme: fuse instructions

            if !changed {
                break;
            }
        }

        // println!("OPTIMISED {}", self.origin);
        // for &op in &self.ops {
        //     println!("    {op}");
        // }

        self.cost.set(0);
    }

    /// Returns the operations comprising this `BitExtract`.
    pub fn ops(&self) -> &[BitOp] {
        &self.ops
    }

    /// Returns the set of bits written by this `BitExtract`.
    pub fn dst_bits(&self) -> u64 {
        self.dst_bits
    }

    /// Calculates the total cost of the `BitExtract`.
    pub fn cost(&self) -> u16 {
        let mut cost = self.cost.get();
        if cost == 0 {
            // Count cost of each operation
            let mut prev = None;
            for op in &self.ops {
                cost += op.cost(prev);
                prev = Some(*op);
            }

            // Count cost of folding into the accumulator
            // fixme: this can be fused into the last op in some cases
            cost += 1;

            self.cost.set(cost);
        };

        cost
    }
}

impl Default for BitExtract {
    fn default() -> Self {
        Self {
            ops: SmallVec::new(),
            dst_bits: u64::MAX,
            cost: Cell::new(0),
            #[cfg(debug_assertions)]
            origin: "<default>".into(),
        }
    }
}

impl Display for BitExtract {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut sep = false;
        for op in &self.ops {
            if std::mem::replace(&mut sep, true) {
                write!(f, ", ")?;
            }
            write!(f, "{op}")?;
        }
        write!(f, " (cost = {})", self.cost())?;
        #[cfg(debug_assertions)]
        {
            write!(f, " (origin = {})", self.origin)?;
        }
        Ok(())
    }
}

impl Debug for BitExtract {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut dbg = f.debug_struct("BitExtract");
        dbg.field("ops", &self.ops);
        dbg.field("cost", &self.cost());
        #[cfg(debug_assertions)]
        {
            dbg.field("origin", &self.origin);
        }
        dbg.finish()
    }
}

impl BitOp {
    /// Returns the canonical operation that sets all bits to zero.
    fn set_to_zero() -> Self {
        Self::And(PartialBits::full(0))
    }

    /// Fuses two instructions, if possible.
    fn try_fuse(first: Self, second: Self) -> Option<Self> {
        match (first, second) {
            (Self::ShiftLeft(a), Self::ShiftLeft(b)) => Some(match a + b {
                sum @ 0..64 => Self::ShiftLeft(sum),
                _ => Self::set_to_zero(),
            }),
            (Self::ShiftRight(a), Self::ShiftRight(b)) => Some(match a + b {
                sum @ 0..64 => Self::ShiftRight(sum),
                _ => Self::set_to_zero(),
            }),
            (Self::ArithRight(a), Self::ArithRight(b)) => {
                let sum = (a + b).min(63);
                Some(Self::ArithRight(sum))
            }
            (Self::RotateRight(a), Self::RotateRight(b)) => {
                let sum = (a + b) % 64;
                Some(Self::RotateRight(sum))
            }
            (Self::And(a), Self::And(b)) => Some(Self::And(a & b)),
            _ => None,
        }
    }

    /// Optimise the operation, given the known input bits and demanded output bits.
    fn optimise(self, KDBits { zeros, used }: KDBits) -> Self {
        let result = match self {
            // If the input bits are all zero, no op has any effect
            _ if zeros == u64::MAX => Self::Nop,
            // If none of the output bits are needed, the operation can be elided
            _ if used == 0 => Self::Nop,
            // Operations with no effect can be reduced to nop
            Self::ShiftLeft(0) => Self::Nop,
            Self::ShiftRight(0) => Self::Nop,
            Self::ArithRight(0) => Self::Nop,
            Self::RotateRight(0) => Self::Nop,
            Self::ShiftOr(0, 0) => Self::Nop,
            Self::Mul(1) => Self::Nop,
            // An arithmetic right shift where the input's high bit
            // is known to be zero is equivalent to a logical right shift
            Self::ArithRight(amt) if zeros & high_bit() != 0 => Self::ShiftRight(amt),
            // Try to reduce a rotation to a left or right shift
            Self::RotateRight(amt) if !zeros << (64 - amt) == 0 => Self::ShiftRight(amt),
            Self::RotateRight(amt) if !zeros >> amt == 0 => Self::ShiftLeft(64 - amt),
            Self::RotateRight(amt) if used >> (64 - amt) == 0 => Self::ShiftRight(amt),
            Self::RotateRight(amt) if used << amt == 0 => Self::ShiftLeft(64 - amt),
            // There is no need to clear bits that are already zero or unused,
            // and a mask that clears no bits can be elided entirely
            Self::And(mask) => {
                let mask = mask.with_used(used & !zeros);
                match mask.zeros() {
                    0 => Self::Nop,
                    _ => Self::And(mask),
                }
            }
            // Strength-reduce multiplication where possible
            Self::Mul(mask) => match mask.count_ones() {
                1 => Self::ShiftLeft(mask.trailing_zeros() as u8),
                2 => {
                    let amt1 = mask.trailing_zeros() as u8;
                    let amt2 = (mask & (mask - 1)).trailing_zeros() as u8;
                    Self::ShiftOr(amt1, amt2)
                }
                _ => self,
            },
            // The operation can't be optimised
            _ => self,
        };
        result.validate();
        result
    }

    fn apply(self, value: u64) -> u64 {
        match self {
            Self::Nop => value,
            Self::ShiftLeft(amt) => value << amt,
            Self::ShiftRight(amt) => value >> amt,
            Self::ArithRight(amt) => ((value as i64) >> amt) as u64,
            Self::RotateRight(amt) => value.rotate_right(amt as u32),
            Self::And(mask) => value & mask.bits(),
            Self::ShiftOr(amt1, amt2) => (value << amt1) | (value << amt2),
            Self::Mul(mask) => value.wrapping_mul(mask),
        }
    }

    /// Propagates known zero bits from input to output.
    fn calc_known_zeros(self, input: u64) -> u64 {
        match self {
            Self::Nop => input,
            Self::ShiftLeft(amt) => input << amt | right_mask(amt),
            Self::ShiftRight(amt) => input >> amt | left_mask(amt),
            Self::ArithRight(amt) => ((input as i64) >> amt) as u64,
            Self::RotateRight(amt) => input.rotate_right(amt as u32),
            Self::And(mask) => input | mask.zeros(),
            Self::ShiftOr(a, b) => {
                let a_mask = Self::ShiftLeft(a).calc_known_zeros(input);
                let b_mask = Self::ShiftLeft(b).calc_known_zeros(input);
                a_mask & b_mask
            }
            Self::Mul(mask) => {
                let trailing_zeros = input.trailing_ones() + mask.trailing_zeros();
                let leading_zeros = input.leading_ones() + mask.leading_zeros();
                let lo_mask = right_mask(trailing_zeros.min(64) as u8);
                let hi_mask = left_mask(leading_zeros.saturating_sub(64) as u8);
                lo_mask | hi_mask
            }
        }
    }

    /// Propagates demanded bits from output to input.
    fn calc_used_bits(self, output: u64) -> u64 {
        match self {
            _ if output == 0 => 0,
            Self::Nop => output,
            Self::ShiftLeft(amt) => output >> amt,
            Self::ShiftRight(amt) => output << amt,
            Self::ArithRight(amt) => {
                let needs_sign = output & left_mask(amt) != 0;
                (output << amt) | needs_sign.then_some(high_bit()).unwrap_or(0)
            }
            Self::RotateRight(amt) => output.rotate_left(amt as u32),
            Self::And(mask) => output & mask.bits(),
            Self::ShiftOr(a, b) => {
                let a_mask = Self::ShiftLeft(a).calc_used_bits(output);
                let b_mask = Self::ShiftLeft(b).calc_used_bits(output);
                a_mask | b_mask
            }
            Self::Mul(_) => u64::MAX >> output.leading_zeros(),
        }
    }

    /// Computes the cost of this operation,
    /// accounting for any potential fusion with the preceeding operation.
    fn cost(&self, prev: Option<BitOp>) -> u16 {
        let isa = ISA;

        // fixme: account for instruction fusion
        // - shift and mask -> `ubfx` or `pext`
        // - others?
        // or: just model the fused ops directly?
        match self {
            Self::Nop => 0,
            Self::ShiftLeft(_) => 1,
            Self::ShiftRight(_) => 1,
            Self::ArithRight(_) => 1,
            Self::RotateRight(_) => 1,
            Self::And(mask) => match isa {
                Isa::AArch64 => 1 + cost_aarch64_logical_imm(*mask),
                _ => 2,
            },
            Self::ShiftOr(0, _) => match isa {
                Isa::AArch64 => 1,
                _ => 2,
            },
            Self::ShiftOr(_, _) => match isa {
                Isa::AArch64 => 2,
                _ => 3,
            },
            Self::Mul(mask) => match isa {
                Isa::AArch64 => 3 + cost_aarch64_mat_imm(PartialBits::full(*mask)),
                _ => 4,
            },
        }

        // fixme: determine whether mask can be elided by changing rotations to shifts
        // fixme: fix cost modelling for immediate instantiation

        // self.cost = [c_sh1, c_sar, c_sh2_and, c_mul, c_or].iter().sum();
    }

    #[cfg(debug_assertions)]
    pub fn validate(self) {
        debug_assert!(match self {
            Self::Nop => true,
            Self::ShiftLeft(amt) => amt < 64,
            Self::ShiftRight(amt) => amt < 64,
            Self::ArithRight(amt) => amt < 64,
            Self::RotateRight(amt) => amt < 64,
            Self::And(_) => true,
            Self::ShiftOr(a, b) => a < b && b < 64,
            Self::Mul(_) => true,
        })
    }
}

impl Display for BitOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Nop => write!(f, "nop"),
            Self::ShiftLeft(amt) => write!(f, "shl {amt}"),
            Self::ShiftRight(amt) => write!(f, "shr {amt}"),
            Self::ArithRight(amt) => write!(f, "sar {amt}"),
            Self::RotateRight(amt) => match amt {
                33..63 => write!(f, "rol {}", 64 - amt),
                _ => write!(f, "ror {amt}"),
            },
            Self::And(mask) => write!(f, "and {}", PrintBits(mask)),
            Self::ShiftOr(0, amt) => write!(f, "or (shl {amt})"),
            Self::ShiftOr(amt1, amt2) => write!(f, "shl {amt1}, or (shl {})", amt2 - amt1), // fixme
            Self::Mul(mask) => write!(f, "mul {}", PrintBits(mask)),
        }
    }
}

impl Debug for BitOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BitOp({self})")
    }
}

fn cost_aarch64_logical_imm(imm: PartialBits) -> u16 {
    if imm.min_bits() == 0 {
        return 0;
    }
    if is_aarch64_logical_immediate(imm.min_bits()) {
        return 0;
    }
    cost_aarch64_mat_imm(imm)
}

fn cost_aarch64_mat_imm(imm: PartialBits) -> u16 {
    // fixme: optimise with `min_bits` and `max_bits`
    let lane = |v: u64, i: u8| (v >> 16 * i) & 0xffff;
    let lanes = (0..4).map(|i| (lane(imm.bits(), i), lane(imm.used(), i)));
    let non_zero_lanes = lanes.clone().filter(|&(b, u)| b & u != 0).count();
    let non_ones_lanes = lanes.clone().filter(|&(b, u)| !b & u != 0).count();
    non_zero_lanes.min(non_ones_lanes.max(1)) as u16
}

const fn high_bit() -> u64 {
    1u64.rotate_right(1)
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
    pub fn new() -> Self {
        Self { len: 0, fixed: 0, rot_masks: Default::default() }
    }

    pub fn len(&self) -> u8 {
        self.len
    }

    pub fn push(&mut self, part: BitPermutationPart) {
        match part {
            BitPermutationPart::Fixed { len, bits } => {
                self.fixed |= bits << self.len;
                self.len += len;
            }
            BitPermutationPart::Slice { len, src_pos } => {
                let src_mask = make_mask(src_pos, len);
                let rol = self.len + 64 - src_pos;
                *self.rot_masks.entry(rol % 64).or_insert(0) |= src_mask;
                self.len += len;
            }
            BitPermutationPart::Repeat { len, src_pos } => {
                let src_mask = 1 << src_pos;
                let first_rol = self.len + 64 - src_pos;
                for rol in first_rol..(first_rol + len) {
                    *self.rot_masks.entry(rol % 64).or_insert(0) |= src_mask;
                }
                self.len += len;
            }
        }
    }

    pub fn from_parts(parts: impl IntoIterator<Item = BitPermutationPart>) -> Self {
        let mut result = Self::new();
        for part in parts {
            result.push(part);
        }
        result
    }

    pub fn compile(&self) -> (u64, Vec<BitExtract>) {
        // Generate shift candidates
        let mut shifts = vec![];
        let mut bits_used_once = 0;
        let mut bits_used_many = 0;

        for (&rol, &src_mask) in &self.rot_masks {
            let dst_mask = src_mask.rotate_left(rol as u32);
            shifts.push(ShiftExtract { rol, dst_mask });
            bits_used_many |= bits_used_once & src_mask;
            bits_used_once |= src_mask;
        }

        // Merge rotation groups with identical source masks
        let mut groups = HashMap::new();
        for (&rol, &src_mask) in &self.rot_masks {
            *groups.entry(src_mask).or_insert(0) |= 1u64 << rol;
        }

        // Generate repeat candidates
        let mut repeats = vec![];

        for (&src_mask, &rol_mask) in &groups {
            if rol_mask.count_ones() > 1 && src_mask.count_ones() > 1 {
                repeats.push(RepeatExtract { src_mask, rol_mask });
            }
        }

        // Generate broadcast candidates
        let mut broadcasts = vec![];

        while bits_used_many != 0 {
            let src_pos = bits_used_many.trailing_zeros() as u8;
            let src_mask = 1 << src_pos;

            let mut dst_mask = 0;
            for (&src_mask2, &rol_mask) in &groups {
                if src_mask2 & src_mask != 0 {
                    dst_mask |= rol_mask;
                }
            }
            dst_mask = dst_mask.rotate_left(src_pos as u32);
            broadcasts.push(BroadcastExtract { src_pos, dst_mask });

            bits_used_many &= bits_used_many - 1;
        }

        // Merge candidates
        let shift_broadcasts = shifts
            .iter()
            .flat_map(|shift| broadcasts.iter().map(move |broadcast| (shift, broadcast)))
            .flat_map(|(shift, broadcast)| {
                let &ShiftExtract { rol, dst_mask: sh_dst_mask } = shift;
                let &BroadcastExtract { src_pos, dst_mask: bc_dst_mask } = broadcast;

                // Prune shifts that only cover broadcast bit
                if sh_dst_mask & !bc_dst_mask == 0 {
                    return None;
                }

                // Combine the destination masks
                let dst_mask = sh_dst_mask | bc_dst_mask;

                // First, rotate the word to place the broadcasted bit at the top
                let ex_ror = src_pos + 1;

                // Next, arithmetic shift right by the minimum amount that covers all needed output bits
                let sar_lz = bc_dst_mask
                    .rotate_right((src_pos + rol) as u32)
                    .leading_zeros();
                let ex_sar = (63 - sar_lz) as u8;

                // Verify that this combination is feasible
                let bc_mask = right_mask(ex_sar).rotate_left((src_pos + 1 + rol) as u32);
                let clobber_mask = bc_mask & !bc_dst_mask;
                if clobber_mask & sh_dst_mask != 0 {
                    return None;
                }

                // Final rotatation to satisfy the demanded net rotation
                let ex_rol = ((rol as i32) + (src_pos as i32 + 1) + (63 - sar_lz as i32)) as u8;

                let extract = BitExtract::new().with_origin(|| format!("shl {rol} + bc {src_pos}"));
                Some(extract.ror(ex_ror).sar(ex_sar).rol(ex_rol).and(dst_mask))
            })
            .collect_vec();
        let shifts = shifts.into_iter().map(|ShiftExtract { rol, dst_mask }| {
            let extract = BitExtract::new().with_origin(|| format!("shl {rol}"));
            extract.rol(rol).and(dst_mask)
        });
        let broadcasts = broadcasts
            .into_iter()
            .map(|BroadcastExtract { src_pos, dst_mask }| {
                let extract = BitExtract::new().with_origin(|| format!("bc {src_pos}"));
                let sar = (63 - dst_mask.trailing_zeros()) as u8;
                extract.shl(63 - src_pos).sar(sar).and(dst_mask)
            });
        let repeats = repeats
            .into_iter()
            // fixme: other rotations
            .flat_map(|r| [(r, 0), (r, r.src_mask.trailing_zeros() as u8)])
            .map(|(RepeatExtract { src_mask, rol_mask }, shr)| {
                BitExtract::new()
                    .with_origin(|| {
                        // fixme: better formatting
                        format!(
                            "rep {} {} {}",
                            PrintBits(src_mask),
                            PrintBits(rol_mask),
                            shr
                        )
                    })
                    .shr(shr)
                    .and(src_mask >> shr)
                    .mul(rol_mask.rotate_left(shr as u32))
            });
        let candidates = shifts
            .chain(broadcasts)
            .chain(repeats)
            .chain(shift_broadcasts)
            .map(BitExtract::optimised)
            .collect_vec();

        // println!("CANDIDATES");
        // println!("----------");
        // for candidate in &candidates {
        //     println!("{candidate}");
        // }
        // println!("");

        // Find minimum cost
        let mut candidates = candidates;
        let extracts: Vec<_> = min_cost_cover(&candidates)
            .into_iter()
            .map(|idx| std::mem::take(&mut candidates[idx]))
            .collect();

        // println!("EXTRACTS");
        // println!("----------");
        // for extract in &extracts {
        //     println!("{extract}");
        // }
        // println!("");

        // fixme: better API
        (self.fixed, extracts)
    }

    pub fn exec(&self, src: u64) -> u64 {
        let mut dst = self.fixed;
        for (&rol, &src_mask) in &self.rot_masks {
            dst |= (src & src_mask).rotate_left(rol as u32);
        }
        dst
    }
}

#[inline(always)]
fn make_mask(pos: u8, len: u8) -> u64 {
    match (pos, len) {
        (_, 0) => 0,
        (0, 64) => u64::MAX,
        _ => ((1 << len) - 1) << pos,
    }
}

fn right_mask(len: u8) -> u64 {
    match len {
        64 => u64::MAX,
        _ => (1 << len) - 1,
    }
}

fn left_mask(len: u8) -> u64 {
    match len {
        64 => u64::MAX,
        _ => !(u64::MAX >> len),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Isa {
    AArch64,
    #[default]
    Unknown,
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_basic_permutation() {
        let permutation = BitPermutation::from_parts([
            BitPermutationPart::Fixed { len: 2, bits: 0b11 },
            BitPermutationPart::Slice { len: 3, src_pos: 5 },
            BitPermutationPart::Slice { len: 3, src_pos: 5 },
            BitPermutationPart::Slice { len: 2, src_pos: 5 },
        ]);

        assert_eq!(permutation.exec(0b00_000_00000), 0b00_000_000_11);
        assert_eq!(permutation.exec(0b11_111_11111), 0b11_111_111_11);
        assert_eq!(permutation.exec(0b00_010_01010), 0b10_010_010_11);
        assert_eq!(permutation.exec(0b11_101_10101), 0b01_101_101_11);
        assert_eq!(permutation.exec(0b01_001_11000), 0b01_001_001_11);
        assert_eq!(permutation.exec(0b10_110_00111), 0b10_110_110_11);
    }

    #[test]
    fn test_permutation_mask_merge() {
        let p1 = [BitPermutationPart::Slice { len: 10, src_pos: 4 }];
        let p2 = [
            BitPermutationPart::Slice { len: 5, src_pos: 4 },
            BitPermutationPart::Slice { len: 5, src_pos: 9 },
        ];
        assert_eq!(
            BitPermutation::from_parts(p1),
            BitPermutation::from_parts(p2)
        );
    }

    #[test]
    fn fuse_rotates() {
        let extract = BitExtract::new().ror(12).rol(6).rol(9).ror(5);
        assert_eq!(extract.ops().len(), 4);
        let extract = extract.optimised();
        assert_eq!(extract.ops(), &[BitOp::RotateRight(2)]);
    }
}
