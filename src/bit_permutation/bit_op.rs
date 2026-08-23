use std::fmt::Debug;

use arbitrary::Arbitrary;

use super::*;
use crate::util::{BitRun, Bits6, PartialBits, iter_set_bits, left_mask, right_mask};

#[derive(Clone, Copy, PartialEq, Eq, Arbitrary)]
pub enum BitOp {
    /// No operation.
    Nop,
    /// Left shift.
    ShiftLeft(Bits6),
    /// Logical right shift.
    ShiftRight(Bits6),
    /// Arithmetic right shift.
    ArithRight(Bits6),
    /// Right rotation.
    RotateRight(Bits6),
    /// Bitwise AND.
    /// It is an invariant that `!mask & !used == 0`.
    And(PartialBits),
    /// Copies the bit pattern to two or more places.
    Copy(u64),
}

impl BitOp {
    /// Returns the canonical operation that sets all bits to zero.
    fn set_to_zero() -> Self {
        Self::And(PartialBits::full(0))
    }

    /// Optimise the operation, given the known input bits and demanded output bits.
    pub fn optimise(self, KDBits { zeros, used }: KDBits) -> Self {
        log::trace!("OPT:   Trying to optimise \"{self}\" (zeros={zeros:#x}, used = {used:#x})");

        let elide = |reason: &str| {
            log::trace!("OPT:     Elided, because {reason}");
            Self::Nop
        };
        let elide_nop = || elide("this operation doesn't change its input");

        let rewrite = |new_op: Self, reason: &str| {
            log::trace!("OPT:     Rewritten to \"{new_op}\", because {reason}");
            new_op
        };

        let try_reduce_rot_to_shift = |ror: Bits6| {
            let rol = ror.neg();
            if used & !zeros & left_mask::<u64>(ror.into()) == 0 {
                let reason = format!("the high {ror} bits are zero or unused");
                return Some(rewrite(Self::ShiftRight(ror), &reason));
            };
            if used & !zeros & right_mask::<u64>(rol.into()) == 0 {
                let reason = format!("the low {rol} bits are zero or unused");
                return Some(rewrite(Self::ShiftLeft(rol), &reason));
            };
            None
        };

        match self {
            Self::Nop => self,
            // If the input bits are all zero, no op has any effect
            _ if zeros == u64::MAX => elide("input is always zero"),
            // If none of the output bits are needed, the operation can be elided
            _ if used == 0 => elide("output isn't used"),
            // Operations with no effect can be reduced to nop
            Self::ShiftLeft(Bits6::ZERO) => elide_nop(),
            Self::ShiftRight(Bits6::ZERO) => elide_nop(),
            Self::ArithRight(Bits6::ZERO) => elide_nop(),
            Self::RotateRight(Bits6::ZERO) => elide_nop(),
            Self::Copy(1) => elide_nop(),
            // An arithmetic right shift where the input's high bit
            // is known to be zero is equivalent to a logical right shift
            Self::ArithRight(amt) if zeros & high_bit() != 0 => {
                let reason = "the input's high bit is always zero";
                rewrite(Self::ShiftRight(amt), reason)
            }
            // Try to reduce a rotation to a left or right shift
            Self::RotateRight(amt) if let Some(op) = try_reduce_rot_to_shift(amt) => op,
            // There is no need to clear bits that are already zero or unused,
            // and a mask that clears no bits can be elided entirely
            Self::And(mask) => match mask.with_used(used & !zeros) {
                m if m.zeros() == 0 => elide("all input bits are zero or unused"),
                m if m != mask => rewrite(Self::And(mask), "some input bits are zero or unused"),
                _ => self,
            },
            // Strength-reduce a degenerate copy to a shift left
            Self::Copy(mask) if mask.count_ones() == 1 => {
                let reason = format!("{mask:#x} has only one set bit");
                rewrite(Self::ShiftLeft(mask.trailing_zeros().into()), &reason)
            }
            // The operation can't be optimised
            _ => self,
        }
    }

    pub fn exec(self, value: u64) -> u64 {
        match self {
            Self::Nop => value,
            Self::ShiftLeft(amt) => value << amt,
            Self::ShiftRight(amt) => value >> amt,
            Self::ArithRight(amt) => ((value as i64) >> amt) as u64,
            Self::RotateRight(amt) => value.rotate_right(amt.into()),
            Self::And(mask) => value & mask.bits(),
            Self::Copy(mask) => value.wrapping_mul(mask),
        }
    }

    /// Propagates known zero bits from input to output.
    pub fn calc_known_zeros(self, input: u64) -> u64 {
        match self {
            Self::Nop => input,
            Self::ShiftLeft(amt) => input << amt | right_mask::<u64>(amt.into()),
            Self::ShiftRight(amt) => input >> amt | left_mask::<u64>(amt.into()),
            Self::ArithRight(amt) => ((input as i64) >> amt) as u64,
            Self::RotateRight(amt) => input.rotate_right(amt.into()),
            Self::And(mask) => input | mask.zeros(),
            Self::Copy(mask) => iter_set_bits(mask)
                .map(|shl| Self::ShiftLeft(shl.into()).calc_known_zeros(input))
                .fold(0, |acc, mask| acc & mask),
        }
    }

    /// Propagates demanded bits from output to input.
    pub fn calc_used_bits(self, output: u64) -> u64 {
        match self {
            _ if output == 0 => 0,
            Self::Nop => output,
            Self::ShiftLeft(amt) => output >> amt,
            Self::ShiftRight(amt) => output << amt,
            Self::ArithRight(amt) => {
                let needs_sign = output & left_mask::<u64>(amt.into()) != 0;
                (output << amt) | needs_sign.then_some(high_bit()).unwrap_or(0)
            }
            Self::RotateRight(amt) => output.rotate_left(amt.into()),
            Self::And(mask) => output & mask.bits(),
            Self::Copy(mask) => iter_set_bits(mask)
                .map(|shl| Self::ShiftLeft(shl.into()).calc_used_bits(output))
                .fold(0, |acc, mask| acc | mask),
        }
    }

    /// Computes the cost of this operation,
    /// accounting for any potential fusion with the preceeding operation.
    pub fn cost(&self, prev: Option<BitOp>) -> u16 {
        let isa = ISA;

        const INS_COST: u16 = 16;
        const IMM_COST: u16 = 1;

        match self {
            Self::Nop => 0,
            Self::ShiftLeft(_) => INS_COST,
            Self::ShiftRight(_) | Self::ArithRight(_) => match prev {
                Some(Self::ShiftLeft(_)) => 0,
                _ => INS_COST,
            },
            Self::RotateRight(_) => INS_COST,
            Self::And(mask) => match isa {
                Isa::AArch64 => INS_COST + cost_aarch64_logical_imm(*mask) * IMM_COST,
                _ => 2 * INS_COST,
            },
            Self::Copy(mask) => match (mask.count_ones(), mask.leading_zeros()) {
                (0, _) => INS_COST,
                (1, 0) => 0,
                (1, _) => INS_COST,
                (2, 0) => match isa {
                    Isa::AArch64 => INS_COST,
                    _ => 2 * INS_COST,
                },
                (2, _) => match isa {
                    Isa::AArch64 => 2 * INS_COST,
                    _ => 3 * INS_COST,
                },
                _ => match isa {
                    Isa::AArch64 => {
                        let ins = 3 * INS_COST;
                        let imm = cost_aarch64_mat_imm(PartialBits::full(*mask)) * IMM_COST;
                        ins + imm
                    }
                    _ => 4 * INS_COST,
                },
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum RewriteResult {
    Preserve,
    One(BitOp),
    Two(BitOp, BitOp),
}

impl BitOp {
    /// Fuses two instructions, if possible.
    pub fn try_fuse(first: Self, second: Self) -> RewriteResult {
        use RewriteResult::*;

        match (first, second) {
            // Two sucessive shifts in the same direction can be fused.
            (Self::ShiftLeft(a), Self::ShiftLeft(b)) => One(match a.checked_add(b) {
                Some(sum) => Self::ShiftLeft(sum),
                None => Self::set_to_zero(),
            }),
            (Self::ShiftRight(a), Self::ShiftRight(b)) => One(match a.checked_add(b) {
                Some(sum) => Self::ShiftRight(sum),
                None => Self::set_to_zero(),
            }),
            (Self::ArithRight(a), Self::ArithRight(b)) => {
                One(Self::ArithRight(a.saturating_add(b)))
            }
            // Two opposed shifts are equivalent to a mask.
            // Note that a left shift followed by an arithmetic right shift
            // cannot be simplified even though the reverse can be.
            (Self::ShiftLeft(a), Self::ShiftRight(b)) if a == b => {
                One(Self::And(PartialBits::full(u64::MAX >> a)))
            }
            (Self::ShiftRight(a) | Self::ArithRight(a), Self::ShiftLeft(b)) if a == b => {
                One(Self::And(PartialBits::full(u64::MAX << a)))
            }
            (Self::RotateRight(a), Self::RotateRight(b)) => {
                One(Self::RotateRight(a.wrapping_add(b)))
            }
            // Two successive masks can be fused.
            (Self::And(a), Self::And(b)) => One(Self::And(a & b)),
            // A shift/mask pair that can be rewritten as two shifts can sometimes
            // be lowered to better machine code, and will never be worse.
            (Self::And(mask), Self::ShiftLeft(shl)) => {
                Self::try_fuse_shl_and_mask(shl, mask << shl)
            }
            (Self::ShiftLeft(shl), Self::And(mask)) => {
                Self::try_fuse_shl_and_mask(shl, mask & (PartialBits::MAX << shl))
            }
            (Self::And(mask), Self::ShiftRight(shr)) => {
                Self::try_fuse_shr_and_mask(shr, mask >> shr)
            }
            (Self::ShiftRight(shr), Self::And(mask)) => {
                Self::try_fuse_shr_and_mask(shr, mask & (PartialBits::MAX >> shr))
            }
            _ => Preserve,
        }
    }

    fn try_fuse_shl_and_mask(shl: Bits6, mask: PartialBits) -> RewriteResult {
        use RewriteResult::*;

        if shl == Bits6::ZERO {
            return One(BitOp::And(mask));
        }

        match mask.as_bit_run() {
            BitRun::Left { min: 0, .. } | BitRun::Right { min: 0, .. } => One(BitOp::set_to_zero()),
            BitRun::Left { min, max } => match max >= 64 - u8::from(shl) {
                true => One(Self::ShiftLeft(shl)),
                false => Two(
                    Self::ShiftRight((64 + min - u8::from(shl)).into()),
                    Self::ShiftLeft((64 - min).into()),
                ),
            },
            BitRun::Right { min, max } => match max == 64 {
                true => One(Self::ShiftLeft(shl)),
                false => Two(
                    Self::ShiftLeft((64 + u8::from(shl) - min).into()),
                    Self::ShiftRight((64 - min).into()),
                ),
            },
            BitRun::None => Preserve,
        }
    }

    fn try_fuse_shr_and_mask(shr: Bits6, mask: PartialBits) -> RewriteResult {
        use RewriteResult::*;

        if shr == Bits6::ZERO {
            return One(BitOp::And(mask));
        }

        match mask.as_bit_run() {
            BitRun::Left { min: 0, .. } | BitRun::Right { min: 0, .. } => One(BitOp::set_to_zero()),
            BitRun::Right { min, max } => match max >= 64 - u8::from(shr) {
                true => One(Self::ShiftRight(shr)),
                false => Two(
                    Self::ShiftLeft((64 + min - u8::from(shr)).into()),
                    Self::ShiftRight((64 - min).into()),
                ),
            },
            BitRun::Left { min, max } => match max == 64 {
                true => One(Self::ShiftRight(shr)),
                false => Two(
                    Self::ShiftRight((64 + u8::from(shr) - min).into()),
                    Self::ShiftLeft((64 - min).into()),
                ),
            },
            BitRun::None => Preserve,
        }
    }
}

impl Display for BitOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Nop => write!(f, "nop"),
            Self::ShiftLeft(amt) => write!(f, "shl {amt}"),
            Self::ShiftRight(amt) => write!(f, "shr {amt}"),
            Self::ArithRight(amt) => write!(f, "sar {amt}"),
            Self::RotateRight(amt) => match amt.neg() < amt {
                true => write!(f, "rol {}", amt.neg()),
                false => write!(f, "ror {amt}"),
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

const fn high_bit() -> u64 {
    1u64.rotate_right(1)
}
