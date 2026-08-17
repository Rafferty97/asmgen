use std::cell::Cell;
use std::cmp::Ordering;
use std::fmt::{Debug, Display};
use std::ops::{BitAnd, Shr};
use std::u64;

use fnv::FnvHashMap;
use itertools::Itertools;
use smallvec::SmallVec;

use crate::playground::min_cost_cover;

// fixme: investigate cranelift lowering:
// - fuse shift and mask into `ubfx` or `pext`

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct BitPermutation {
    pub len: u8,
    pub fixed: u64,
    pub rot_masks: FnvHashMap<u8, u64>,
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

#[derive(Clone)]
pub struct BitExtract {
    /// The sequence of operations
    ops: SmallVec<[BitOp; 4]>,
    /// The destination bits that are written to;
    /// the other bits are guaranteed to always be zero
    dst_bits: u64,
    /// The total cost of the operations
    cost: Cell<u16>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BitOp {
    /// Left shift.
    ShiftLeft(u8),
    /// Logical right shift.
    ShiftRight(u8),
    /// Arithmetic right shift.
    ArithRight(u8),
    /// Right rotation.
    RotateRight(u8),
    /// Bitwise AND.
    Mask(u64),
    /// Two shifts followed by a bitwise or;
    /// the smaller shift preceeds the larger shift, and may be zero.
    ShiftOr(u8, u8),
    /// Integer multiplication.
    Mul(u64),
}

impl BitExtract {
    /// Creates an empty `BitExtract` that returns its input untransformed.
    pub fn new() -> Self {
        Default::default()
    }

    pub fn shl(mut self, amt: u8) -> Self {
        self.push(BitOp::ShiftLeft(amt));
        self
    }

    pub fn rol(mut self, amt: u8) -> Self {
        self.push(BitOp::RotateRight((64 - amt) % 64));
        self
    }

    pub fn ror(mut self, amt: u8) -> Self {
        self.push(BitOp::RotateRight(amt));
        self
    }

    pub fn mask(mut self, mask: u64) -> Self {
        self.push(BitOp::Mask(mask));
        self
    }

    /// Pushes an operation.
    pub fn push(&mut self, op: BitOp) {
        if op.is_nop() {
            return;
        }
        self.ops.push(op);
        self.dst_bits = op.apply(self.dst_bits);
    }

    pub fn optimised(mut self) -> Self {
        self.optimise();
        self
    }

    /// Optimises the `BitExtract` by fusing operations where possible.
    pub fn optimise(&mut self) {
        // fixme: removing mask requires backwards traversal

        self.ops.push(BitOp::nop());

        let mut curr = BitOp::nop();
        let mut dst_bits = u64::MAX;

        for op in &mut self.ops {
            // todo: attempt to fuse `curr` with `op`
            // - fuse adjacent rotations
            // - fuse adjacent shifts in same direction
            // - other rotate/shift fusions?

            // Optimise and emit `curr`
            curr = curr.optimise(dst_bits);
            dst_bits = curr.apply(dst_bits);
            std::mem::swap(op, &mut curr);
        }

        debug_assert!(curr.is_nop());
        debug_assert_eq!(dst_bits, self.dst_bits);

        self.ops.retain(|op| !op.is_nop());
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
        Self { ops: SmallVec::new(), dst_bits: u64::MAX, cost: Cell::new(0) }
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
        Ok(())
    }
}

impl Debug for BitExtract {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut dbg = f.debug_tuple("BitExtract");
        for op in &self.ops {
            dbg.field(op);
        }
        dbg.finish()
    }
}

impl BitOp {
    /// Returns the canonical "nop" operation
    fn nop() -> Self {
        Self::ShiftLeft(0)
    }

    fn is_nop(self) -> bool {
        match self {
            Self::ShiftLeft(0) => true,
            Self::ShiftRight(0) => true,
            Self::ArithRight(0) => true,
            Self::RotateRight(0) => true,
            Self::Mask(u64::MAX) => true,
            Self::ShiftOr(0, 0) => true,
            Self::Mul(1) => true,
            _ => false,
        }
    }

    /// Optimise the operation.
    ///
    /// `src_bits` specifies which bits in the input are guaranteed to be zero.
    fn optimise(self, src_bits: u64) -> Self {
        match self {
            // If the source bits are all zero, every op has no effect
            _ if src_bits == 0 => Self::nop(),
            // An arithmetic right shift where the input's high bit
            // is known to be zero is equivalent to a logical right shift
            Self::ArithRight(amt) if src_bits & high_bit() == 0 => Self::ShiftRight(amt),
            // Try to reduce a rotation to a left or right shift,
            // checking the degenerate zero case first to avoid a panic
            Self::RotateRight(0) => Self::nop(),
            Self::RotateRight(amt) if src_bits << (64 - amt) == 0 => Self::ShiftRight(amt),
            Self::RotateRight(amt) if src_bits >> amt == 0 => Self::ShiftLeft(64 - amt),
            // A mask that clears bits that are already zero is redundant
            Self::Mask(mask) if !mask & src_bits == 0 => Self::nop(),
            // Check for bit copies that have no effect
            Self::ShiftOr(_, _) | Self::Mul(_) if self.apply(src_bits) == src_bits => Self::nop(),
            // Strength-reduce multiplication where possible
            Self::Mul(mask) => match mask.count_ones() {
                0 => Self::Mask(0),
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
        }
    }

    fn apply(self, value: u64) -> u64 {
        match self {
            Self::ShiftLeft(amt) => value << amt,
            Self::ShiftRight(amt) => value >> amt,
            Self::ArithRight(amt) => ((value as i64) >> amt) as u64,
            Self::RotateRight(amt) => value.rotate_right(amt as u32),
            Self::Mask(mask) => value & mask,
            Self::ShiftOr(amt1, amt2) => (value << amt1) | (value << amt2),
            Self::Mul(mask) => value.wrapping_mul(mask),
        }
    }

    /// Computes which source bits are used given `dst_bits`,
    /// a mask denoting which destination bits are used.
    fn calc_src_bits(self, dst_bits: u64) -> u64 {
        match self {
            _ if dst_bits == 0 => 0,
            Self::ShiftLeft(amt) => dst_bits >> amt,
            Self::ShiftRight(amt) => dst_bits << amt,
            Self::ArithRight(amt) => {
                let needs_sign = dst_bits & left_mask(amt) != 0;
                (dst_bits << amt) | needs_sign.then_some(high_bit()).unwrap_or(0)
            }
            Self::RotateRight(amt) => dst_bits.rotate_left(amt as u32),
            Self::Mask(mask) => dst_bits & mask,
            Self::ShiftOr(amt1, amt2) => (dst_bits >> amt1) | (dst_bits >> amt2),
            Self::Mul(_) => u64::MAX >> dst_bits.leading_zeros(), // fixme: conservative but correct
        }
    }

    /// Computes the cost of this operation,
    /// accounting for any potential fusion with the preceeding operation.
    ///
    /// This method assumes that `!self.is_nop()`, as otherwise the cost would be zero.
    fn cost(&self, prev: Option<BitOp>) -> u16 {
        // fixme: account for instruction fusion
        // - shift and mask -> `ubfx` or `pext`
        // - others?
        match self {
            Self::ShiftLeft(_) => 1,
            Self::ShiftRight(_) => 1,
            Self::ArithRight(_) => 1,
            Self::RotateRight(_) => 1,
            Self::Mask(_) => 1,
            Self::ShiftOr(0, _) => 2, // fixme: depends on arch, for ARM this is 1
            Self::ShiftOr(_, _) => 3, // fixme: depends on arch, for ARM this is 2
            Self::Mul(_) => 3,
        }

        // fixme: determine whether mask can be elided by changing rotations to shifts
        // fixme: fix cost modelling for immediate instantiation

        // let c_sh1 = self.sh1.cost();
        // let c_sar = if self.sar != 0 { 1 } else { 0 };
        // let c_sh2 = self.sh2.cost();
        // let c_and = if self.and != u64::MAX { 1 } else { 0 };
        // let c_sh2_and = match (self.sh2, is_right_mask(self.and), c_sh2 + c_and) {
        //     // (RotateOrShift::ShiftRight(_), true, 2..) => 1,
        //     _ => c_sh2 + c_and,
        // };

        // self.cost = [c_sh1, c_sar, c_sh2_and, c_mul, c_or].iter().sum();
    }
}

impl Display for BitOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::ShiftLeft(amt) => write!(f, "shl {amt}"),
            Self::ShiftRight(amt) => write!(f, "shr {amt}"),
            Self::ArithRight(amt) => write!(f, "sar {amt}"),
            Self::RotateRight(amt) => match amt {
                33..63 => write!(f, "rol {}", 64 - amt),
                _ => write!(f, "ror {amt}"),
            },
            Self::Mask(mask) => write!(f, "and {}", PrintBinary(mask)),
            Self::ShiftOr(0, amt) => write!(f, "or (shl {amt})"),
            Self::ShiftOr(amt1, amt2) => write!(f, "shl {amt1}, or (shl {})", amt2 - amt1), // fixme
            Self::Mul(mask) => write!(f, "mul {}", PrintBinary(mask)),
        }
    }
}

impl Debug for BitOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BitOp({self})")
    }
}

const fn high_bit() -> u64 {
    1u64.rotate_right(1)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct BitExtractOld {
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
    pub dst_bits: u64,
}

impl Default for BitExtractOld {
    fn default() -> Self {
        Self {
            sh1: RotateOrShift::None,
            sar: 0,
            sh2: RotateOrShift::None,
            and: u64::MAX,
            mul: 1,
            cost: 0,
            dst_bits: 0,
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
    pub fn new() -> Self {
        Self { len: 0, fixed: 0, rot_masks: Default::default() }
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
        let shifts = self
            .rot_masks
            .iter()
            .map(|(&rol, &src_mask)| {
                let dst_mask = src_mask.rotate_left(rol as u32);
                ShiftExtract { rol, dst_mask }
            })
            .collect_vec();

        let candidates = shifts
            .into_iter()
            .map(|ShiftExtract { rol, dst_mask }| {
                BitExtract::new().rol(rol).mask(dst_mask).optimised()
            })
            .collect_vec();

        // // Generate broadcast candidates

        // // Join rotation groups with identical source masks
        // let mut groups = HashMap::new();
        // for (&rol, &src_mask) in &self.rot_masks {
        //     *groups.entry(src_mask).or_insert(0) |= 1u64 << rol;
        // }

        // // Generate candidates
        // // fixme: add debug info to trace where each candidate comes from
        // let mut broadcasts = vec![];
        // let mut repeats = vec![];

        // for (src_mask, rol_mask) in groups {
        //     let one_bit_src = src_mask.count_ones() == 1;
        //     let multi_bit_dst = rol_mask.count_ones() > 1;

        //     if one_bit_src && multi_bit_dst {
        //         let src_pos = src_mask.trailing_zeros() as u8;
        //         let dst_mask = rol_mask.rotate_left(src_pos as u32);
        //         broadcasts.push(BroadcastExtract { src_pos, dst_mask });
        //     } else {
        //         if multi_bit_dst {
        //             repeats.push(RepeatExtract { src_mask, rol_mask });
        //         }
        //     }
        // }

        // let mut candidates = vec![];

        // for shift in &shifts {
        //     candidates.push(BitExtractOld::new_shift(*shift));
        // }

        // for broadcast in broadcasts {
        //     candidates.push(BitExtractOld::new_broadcast(broadcast));

        //     for shift in &shifts {
        //         candidates.extend(BitExtractOld::try_new_shift_broadcast(*shift, broadcast));
        //     }
        // }

        // for repeat in repeats {
        //     // fixme: try all plausible rotations
        //     candidates.push(BitExtractOld::new_repeat(repeat, 0));
        //     candidates.push(BitExtractOld::new_repeat(
        //         repeat,
        //         repeat.src_mask.trailing_zeros() as u8,
        //     ));
        // }

        println!("CANDIDATES");
        println!("----------");
        for candidate in &candidates {
            println!("{candidate}");
        }
        println!("");

        // Find minimum cost
        let mut candidates = candidates;
        let extracts: Vec<_> = min_cost_cover(&candidates)
            .into_iter()
            .map(|idx| std::mem::take(&mut candidates[idx]))
            .collect();

        println!("CHOSEN EXTRACTS");
        println!("----------");
        for extract in &extracts {
            println!("{extract}");
        }
        println!("");

        // todo: exploit known-zero bits (will need API to express)

        // todo
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

impl BitExtractOld {
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
        let dst_bits = dst_mask;

        Self { sh1, sar, sh2, and, mul, cost, dst_bits }
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
        let dst_bits = dst_mask;

        Self { sh1, sar, sh2, and, mul, cost, dst_bits }
    }

    fn new_repeat(extract: RepeatExtract, shr: u8) -> Self {
        let RepeatExtract { src_mask, rol_mask } = extract;

        let sh1 = RotateOrShift::None;
        let sar = 0;
        let sh2 = RotateOrShift::new_shr(shr);
        let and = src_mask.shr(shr);
        let mul = rol_mask.rotate_left(shr as u32);
        let cost = 0;
        let dst_bits = and.wrapping_mul(mul);

        Self { sh1, sar, sh2, and, mul, cost, dst_bits }
    }

    fn try_new_shift_broadcast(shift: ShiftExtract, broadcast: BroadcastExtract) -> Option<Self> {
        let ShiftExtract { rol, dst_mask: sh_dst_mask } = shift;
        let BroadcastExtract { src_pos, dst_mask: bc_dst_mask } = broadcast;

        let dst_mask = sh_dst_mask | bc_dst_mask;

        // Broadcast determines initial rotation
        let sh1 = RotateOrShift::new_rol(63 - src_pos, u64::MAX);

        // Shift determines final rotation, and therefore arithmetic shift distance
        let sar_lz = bc_dst_mask
            .rotate_right((src_pos + rol) as u32)
            .leading_zeros();
        let sar = (63 - sar_lz) as u8;

        // Shift determines final rotation
        let rol = (rol as i32) - sh1.net_rol() + (sar as i32);
        let sh2 = RotateOrShift::new_rol((rol % 64) as u8, u64::MAX);

        let and = dst_mask;
        let mul = 1;
        let cost = 0;
        let cover = dst_mask;

        Some(Self { sh1, sar, sh2, and, mul, cost, dst_bits: cover })
    }

    fn calc_cost(&mut self) {
        unimplemented!()
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

impl Debug for BitExtractOld {
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
        write!(f, "\ncover = {}", PrintBinary(self.dst_bits))
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

#[inline(always)]
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
        let permutation = BitPermutation::from_parts([
            BitPermutationPart::Fixed { len: 2, bits: 0b11 },
            BitPermutationPart::Slice { len: 3, src_pos: 5 },
            BitPermutationPart::Slice { len: 3, src_pos: 5 },
            BitPermutationPart::Slice { len: 2, src_pos: 5 },
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
    fn test_riscv_immediates() {
        // Decode permutations
        let i_imm = BitPermutation::from_parts([
            BitPermutationPart::Slice { len: 12, src_pos: 20 },
            BitPermutationPart::Repeat { len: 32, src_pos: 31 },
        ]);

        // todo
    }
}
