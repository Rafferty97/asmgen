use std::fmt::Debug;

use super::*;
use crate::util::{BitRun, PartialBits, iter_set_bits, left_mask, right_mask};

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

#[derive(Clone, Copy, Debug)]
pub enum RewriteResult {
    Preserve,
    One(BitOp),
    Two(BitOp, BitOp),
}

impl BitOp {
    /// Returns the canonical operation that sets all bits to zero.
    fn set_to_zero() -> Self {
        Self::And(PartialBits::full(0))
    }

    /// Fuses two instructions, if possible.
    pub fn try_fuse(first: Self, second: Self) -> RewriteResult {
        use RewriteResult::*;

        match (first, second) {
            // Two sucessive shifts in the same direction can be fused.
            (Self::ShiftLeft(a), Self::ShiftLeft(b)) => One(match a + b {
                sum @ 0..64 => Self::ShiftLeft(sum),
                _ => Self::set_to_zero(),
            }),
            (Self::ShiftRight(a), Self::ShiftRight(b)) => One(match a + b {
                sum @ 0..64 => Self::ShiftRight(sum),
                _ => Self::set_to_zero(),
            }),
            (Self::ArithRight(a), Self::ArithRight(b)) => {
                let sum = (a + b).min(63);
                One(Self::ArithRight(sum))
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
                let sum = (a + b) % 64;
                One(Self::RotateRight(sum))
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

    fn try_fuse_shl_and_mask(shl: u8, mask: PartialBits) -> RewriteResult {
        use RewriteResult::*;

        if shl == 0 {
            return One(BitOp::And(mask));
        }

        match mask.as_bit_run() {
            BitRun::Left { min: 0, .. } | BitRun::Right { min: 0, .. } => One(BitOp::set_to_zero()),
            BitRun::Left { min, max } => match max >= 64 - shl {
                true => One(Self::ShiftLeft(shl)),
                false => Two(Self::ShiftRight(64 + min - shl), Self::ShiftLeft(64 - min)),
            },
            BitRun::Right { min, max } => match max == 64 {
                true => One(Self::ShiftLeft(shl)),
                false => Two(Self::ShiftLeft(64 + shl - min), Self::ShiftRight(64 - min)),
            },
            BitRun::None => Preserve,
        }
    }

    fn try_fuse_shr_and_mask(shr: u8, mask: PartialBits) -> RewriteResult {
        use RewriteResult::*;

        if shr == 0 {
            return One(BitOp::And(mask));
        }

        match mask.as_bit_run() {
            BitRun::Left { min: 0, .. } | BitRun::Right { min: 0, .. } => One(BitOp::set_to_zero()),
            BitRun::Right { min, max } => match max >= 64 - shr {
                true => One(Self::ShiftRight(shr)),
                false => Two(Self::ShiftLeft(64 + min - shr), Self::ShiftRight(64 - min)),
            },
            BitRun::Left { min, max } => match max == 64 {
                true => One(Self::ShiftRight(shr)),
                false => Two(Self::ShiftRight(64 + shr - min), Self::ShiftLeft(64 - min)),
            },
            BitRun::None => Preserve,
        }
    }

    /// Optimise the operation, given the known input bits and demanded output bits.
    pub fn optimise(self, KDBits { zeros, used }: KDBits) -> Self {
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

    pub fn apply(self, value: u64) -> u64 {
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
    pub fn calc_known_zeros(self, input: u64) -> u64 {
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
    pub fn calc_used_bits(self, output: u64) -> u64 {
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
    pub fn cost(&self, prev: Option<BitOp>) -> u16 {
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

const fn high_bit() -> u64 {
    1u64.rotate_right(1)
}
