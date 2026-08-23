use std::cell::Cell;
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::u64;

use arbitrary::Arbitrary;
use fnv::FnvHashMap;
use itertools::Itertools;
use smallvec::SmallVec;

pub use self::bit_op::BitOp;
use crate::bit_permutation::bit_op::RewriteResult;
use crate::util::aarch64::is_aarch64_logical_immediate;
use crate::util::{BitRun, Ratio};
use crate::util::{PartialBits, PrimIntExt, PrintBits};
use crate::util::{iter_set_bits, left_mask, middle_mask, right_mask};

mod bit_op;

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
                match BitOp::try_fuse(*op1, *op2) {
                    RewriteResult::Preserve => {}
                    RewriteResult::One(fused) => {
                        *op1 = BitOp::Nop;
                        *op2 = fused;
                        changed = true;
                    }
                    RewriteResult::Two(first, second) => {
                        *op1 = first;
                        *op2 = second;
                        changed = true;
                    }
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
