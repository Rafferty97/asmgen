use std::cell::Cell;
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::u64;

use arbitrary::Arbitrary;
use fnv::FnvHashMap;
use itertools::Itertools;
use smallvec::SmallVec;

use crate::util::Ratio;
use crate::util::aarch64::is_aarch64_logical_immediate;
use crate::util::{PartialBits, PrimIntExt, PrintBits};
use crate::util::{iter_set_bits, left_mask, middle_mask, right_mask};

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
    /// Copies the bit pattern to two or more places.
    Copy(u64),
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

    pub fn copy(mut self, mask: u64) -> Self {
        self.push(BitOp::Copy(mask));
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
            // Two sucessive shifts in the same direction can be fused.
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
            // Two opposed shifts are equivalent to a mask.
            // Note that a left shift followed by an arithmetic right shift
            // cannot be simplified even though the reverse can be.
            (Self::ShiftLeft(a), Self::ShiftRight(b)) if a == b => {
                Some(Self::And(PartialBits::full(u64::MAX >> a)))
            }
            (Self::ShiftRight(a) | Self::ArithRight(a), Self::ShiftLeft(b)) if a == b => {
                Some(Self::And(PartialBits::full(u64::MAX << a)))
            }
            (Self::RotateRight(a), Self::RotateRight(b)) => {
                let sum = (a + b) % 64;
                Some(Self::RotateRight(sum))
            }
            // Two successive masks can be fused.
            (Self::And(a), Self::And(b)) => Some(Self::And(a & b)),
            // A shift/mask pair that can be rewritten as two shifts can sometimes
            // be lowered to better machine code, and will never be worse.
            (Self::And(m), Self::ShiftLeft(k)) => Self::try_fuse_shift_and_mask(64 - k, m << k),
            (Self::ShiftLeft(k), Self::And(m)) => {
                let mask = m & PartialBits::MAX << k;
                Self::try_fuse_shift_and_mask(64 - k, mask)
            }
            (Self::And(m), Self::ShiftRight(k)) => Self::try_fuse_shift_and_mask(k, m >> k),
            (Self::ShiftRight(k), Self::And(m)) => {
                let mask = m & PartialBits::MAX << k;
                Self::try_fuse_shift_and_mask(k, mask)
            }
            _ => None,
        }
    }

    fn try_fuse_shift_and_mask(ror: u8, mask: PartialBits) -> Option<Self> {
        None // todo
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
            Self::Copy(1) => Self::Nop,
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
            // Strength-reduce a degenerate copy to a shift left
            Self::Copy(mask) if mask.is_power_of_two() => {
                Self::ShiftLeft(mask.trailing_zeros() as u8)
            }
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
            Self::Copy(mask) => value.wrapping_mul(mask),
        }
    }

    /// Propagates known zero bits from input to output.
    fn calc_known_zeros(self, input: u64) -> u64 {
        match self {
            Self::Nop => input,
            Self::ShiftLeft(amt) => input << amt | right_mask::<u64>(amt),
            Self::ShiftRight(amt) => input >> amt | left_mask::<u64>(amt),
            Self::ArithRight(amt) => ((input as i64) >> amt) as u64,
            Self::RotateRight(amt) => input.rotate_right(amt as u32),
            Self::And(mask) => input | mask.zeros(),
            Self::Copy(mask) => iter_set_bits(mask)
                .map(|shl| Self::ShiftLeft(shl).calc_known_zeros(input))
                .fold(0, |acc, mask| acc & mask),
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
                let needs_sign = output & left_mask::<u64>(amt) != 0;
                (output << amt) | needs_sign.then_some(high_bit()).unwrap_or(0)
            }
            Self::RotateRight(amt) => output.rotate_left(amt as u32),
            Self::And(mask) => output & mask.bits(),
            Self::Copy(mask) => iter_set_bits(mask)
                .map(|shl| Self::ShiftLeft(shl).calc_used_bits(output))
                .fold(0, |acc, mask| acc | mask),
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
            Self::Copy(mask) => match (mask.count_ones(), mask.leading_zeros()) {
                (0, _) => 1,
                (1, 0) => 0,
                (1, _) => 1,
                (2, 0) => match isa {
                    Isa::AArch64 => 1,
                    _ => 2,
                },
                (2, _) => match isa {
                    Isa::AArch64 => 2,
                    _ => 3,
                },
                _ => match isa {
                    Isa::AArch64 => 3 + cost_aarch64_mat_imm(PartialBits::full(*mask)),
                    _ => 4,
                },
            },
        }

        // fixme: determine whether mask can be elided by changing rotations to shifts
        // fixme: fix cost modelling for immediate instantiation

        // self.cost = [c_sh1, c_sar, c_sh2_and, c_mul, c_or].iter().sum();
    }

    pub fn validate(self) {
        #[cfg(debug_assertions)]
        debug_assert!(match self {
            Self::Nop => true,
            Self::ShiftLeft(amt) => amt < 64,
            Self::ShiftRight(amt) => amt < 64,
            Self::ArithRight(amt) => amt < 64,
            Self::RotateRight(amt) => amt < 64,
            Self::And(_) => true,
            Self::Copy(_) => true,
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
            Self::Copy(0) => write!(f, "dup <none>"),
            Self::Copy(mask) => write!(f, "dup {}", iter_set_bits(mask).join(", ")),
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
    let lanes = |v: u64| (0..4).map(move |i| (v >> 16 * i) & 0xffff);
    let non_zero_lanes = lanes(imm.min_bits()).filter(|&bits| bits != 0).count();
    let non_full_lanes = lanes(imm.max_bits()).filter(|&bits| bits != 0xffff).count();
    non_zero_lanes.min(non_full_lanes.max(1)) as u16
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
                let src_mask = middle_mask::<u64>(src_pos, len);
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
                let bc_mask = right_mask::<u64>(ex_sar).rotate_left((src_pos + 1 + rol) as u32);
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
                    .copy(rol_mask.rotate_left(shr as u32))
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
        let extracts = min_cost_cover(candidates);

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

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Isa {
    AArch64,
    #[default]
    Unknown,
}

fn min_cost_cover(mut candidates: Vec<BitExtract>) -> Vec<BitExtract> {
    // The bits we must cover are exactly those some candidate can supply.
    let universe: u64 = candidates.iter().fold(0, |acc, c| acc | c.dst_bits());
    if universe == 0 {
        // No candidate covers anything: nothing needs covering, empty solution.
        return Vec::new();
    }

    // Initial state
    let mut cover = 0;
    let mut chosen = Vec::<BitExtract>::new();

    while cover != universe {
        // Filter out dead candidates
        candidates.retain(|c| !cover.covers(c.dst_bits()));

        // Score the remaining candidates
        let candidate_costs = candidates.iter().map(|c| {
            let added_bits = (c.dst_bits() & !cover).count_ones() as u16;
            let added_cost = c.cost();
            let saved_cost = chosen
                .iter()
                .filter(|d| c.dst_bits().covers(d.dst_bits()))
                .map(|d| d.cost())
                .sum::<u16>();
            let net_cost = added_cost.saturating_sub(saved_cost);
            (c, Ratio::new(net_cost as i32, added_bits as i32))
        });

        // Pick the best
        let (best, _) = candidate_costs.min_by_key(|&(_, cost)| cost).unwrap();

        // Add it to the set and remove subsumed candidates
        chosen.retain(|c| !best.dst_bits().covers(c.dst_bits()));
        chosen.push(best.clone());

        // Update the new cover
        cover |= best.dst_bits();
    }

    chosen
}

impl<'a> Arbitrary<'a> for BitPermutation {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let mut result = Self::new();
        let len = u.int_in_range(0..=64)?;

        while result.len() < len {
            let part = BitPermutationPart::arbitrary(u)?.trunc(len - result.len());
            result.push(part);
        }

        Ok(result)
    }
}

impl<'a> Arbitrary<'a> for BitPermutationPart {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let len = u.int_in_range(1..=64)?;
        Ok(match u.int_in_range(0..=2)? {
            0 => Self::Fixed { len, bits: u.arbitrary()? },
            1 => Self::Slice { len, src_pos: u.int_in_range(0..=64 - len)? },
            2 => Self::Repeat { len, src_pos: u.int_in_range(0..=63)? },
            _ => unreachable!(),
        })
    }
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

    #[test]
    fn test_cost_aarch64_mat_imm() {
        // Zero: no nonzero lanes, movz path needs nothing (xzr).
        let bits = PartialBits::full(0);
        assert_eq!(cost_aarch64_mat_imm(bits), 0);

        // All-ones: movn path covers every lane, but the movn itself still costs 1.
        let bits = PartialBits::full(!0);
        assert_eq!(cost_aarch64_mat_imm(bits), 1);

        // Single nonzero lane, each position: one movz.
        let bits = PartialBits::full(0x0000_0000_0000_1234);
        assert_eq!(cost_aarch64_mat_imm(bits), 1);

        let bits = PartialBits::full(0x0000_0000_1234_0000);
        assert_eq!(cost_aarch64_mat_imm(bits), 1);

        let bits = PartialBits::full(0x0000_1234_0000_0000);
        assert_eq!(cost_aarch64_mat_imm(bits), 1);

        let bits = PartialBits::full(0x1234_0000_0000_0000);
        assert_eq!(cost_aarch64_mat_imm(bits), 1);

        // Single non-0xffff lane: movn + 0 movk beats movz + 3 movk.
        let bits = PartialBits::full(0xffff_ffff_ffff_1234);
        assert_eq!(cost_aarch64_mat_imm(bits), 1);

        let bits = PartialBits::full(0x1234_ffff_ffff_ffff);
        assert_eq!(cost_aarch64_mat_imm(bits), 1);

        // Two nonzero lanes (the 0x00010001 mask from the multiply-broadcast case).
        let bits = PartialBits::full(0x0000_0000_0001_0001);
        assert_eq!(cost_aarch64_mat_imm(bits), 2);

        // Two non-0xffff lanes.
        let bits = PartialBits::full(0xffff_1234_5678_ffff);
        assert_eq!(cost_aarch64_mat_imm(bits), 2);

        // Three lanes each way; neither path wins, both cost 3.
        let bits = PartialBits::full(0x1234_5678_0000_ffff);
        assert_eq!(cost_aarch64_mat_imm(bits), 3);

        // Four nonzero, four non-0xffff: 4 either way.
        let bits = PartialBits::full(0x1234_5678_9abc_def0);
        assert_eq!(cost_aarch64_mat_imm(bits), 4);

        // A logical-immediate-shaped value still costs by lane count here; folding
        // is decided by the caller, not this function.
        let bits = PartialBits::full(0x0000_0000_0000_ffff);
        assert_eq!(cost_aarch64_mat_imm(bits), 1);

        // Everything free: min_bits == 0 (movz path costs 0), so the min is 0.
        let bits = PartialBits::full(0xdead_beef_dead_beef).with_used(0);
        assert_eq!(cost_aarch64_mat_imm(bits), 0);

        // One fully-free lane, rest constrained to zero: the free lane can go to 0.
        let bits = PartialBits::full(0).with_used(0xffff_ffff_ffff_0000);
        assert_eq!(cost_aarch64_mat_imm(bits), 0);

        // One fully-free lane, rest constrained to 0xffff: free lane goes to 0xffff,
        // so the movn path sees zero non-full lanes and costs 1.
        let bits = PartialBits::full(0xffff_ffff_ffff_ffff).with_used(0xffff_ffff_ffff_0000);
        assert_eq!(cost_aarch64_mat_imm(bits), 1);

        // Free bits *within* a lane that also has demanded ones: the lane is nonzero
        // under min_bits regardless, so it still needs a movz.
        let bits = PartialBits::full(0x0000_0000_0000_1234).with_used(0x0000_0000_0000_ff00);
        assert_eq!(cost_aarch64_mat_imm(bits), 1);

        // Free bits within a lane whose demanded bits are all zero: min_bits clears
        // the lane entirely, so it costs nothing on the movz path.
        let bits = PartialBits::full(0).with_used(0x0000_0000_0000_00ff);
        assert_eq!(cost_aarch64_mat_imm(bits), 0);

        // Freedom lets the movn path win: three lanes are demanded-0xffff, the
        // fourth is free and resolves to 0xffff.
        let bits = PartialBits::full(0xffff_ffff_ffff_0000).with_used(0xffff_ffff_ffff_0000);
        assert_eq!(cost_aarch64_mat_imm(bits), 1);

        // Partial freedom that can't rescue either path: two lanes demand a mix of
        // ones and zeros, so both min_bits and max_bits leave them dirty.
        let bits = PartialBits::full(0x0000_1234_5678_0000).with_used(0x0000_ffff_ffff_0000);
        assert_eq!(cost_aarch64_mat_imm(bits), 2);
    }
}
