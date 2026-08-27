use std::fmt::Debug;

use arbitrary::Arbitrary;

use super::*;
use crate::target::aarch64::immediate::cost_aarch64_immediate;
use crate::util::{Bits, PartialBits, iter_set_bits, left_mask, right_mask};

type Bits6 = Bits<6>;

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
        log::trace!("OPT:   Trying to optimise \"{self}\" (zeros={zeros:#x}, used={used:#x})");

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
            let zeros = used & !zeros.rotate_right(ror.into());
            let rol = ror.neg();
            if zeros & left_mask::<u64>(ror.into()) == 0 {
                let reason = format!("the high {ror} output bits are zero or unused");
                return Some(rewrite(Self::ShiftRight(ror), &reason));
            };
            if zeros & right_mask::<u64>(rol.into()) == 0 {
                let reason = format!("the low {rol} output bits are zero or unused");
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
                m if m != mask => rewrite(Self::And(m), "some input bits are zero or unused"),
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
            Self::Copy(mask) => iter_set_bits(mask).map(|k| value << k).fold(0, |a, b| a | b),
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
            Self::And(mask) => output & !mask.zeros(),
            Self::Copy(mask) => iter_set_bits(mask)
                .map(|shl| Self::ShiftLeft(shl.into()).calc_used_bits(output))
                .fold(0, |acc, mask| acc | mask),
        }
    }

    /// Computes the cost of this operation,
    /// accounting for any potential fusion with the preceeding operation.
    pub fn cost(&self, prev: Option<BitOp>) -> u16 {
        const INS_COST: u16 = 16;
        const IMM_COST: u16 = 1;

        let isa = ISA;

        match self {
            Self::Nop => 0,
            Self::ShiftLeft(_) => INS_COST,
            Self::ShiftRight(_) | Self::ArithRight(_) => match prev {
                Some(Self::ShiftLeft(_)) => 0,
                _ => INS_COST,
            },
            Self::RotateRight(_) => INS_COST,
            Self::And(mask) => match isa {
                Isa::AArch64 => INS_COST + cost_aarch64_immediate(*mask, true) * IMM_COST,
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
                        let insts = cost_aarch64_immediate((*mask).into(), false);
                        3 * INS_COST + insts * IMM_COST
                    }
                    _ => 4 * INS_COST,
                },
            },
        }
    }
}

impl BitOp {
    /// Fuses two instructions, if possible.
    pub fn try_fuse(first: Self, second: Self) -> [Self; 2] {
        log::trace!("OPT:   Trying to fuse \"{first}\", \"{second}\"");

        let preserve = [first, second];

        let fuse = |op: Self, reason: &str| {
            log::trace!("OPT:     Fused into \"{op}\", because {reason}");
            [Self::Nop, op]
        };

        let sum_shifts = |a: Bits6, b: Bits6, ctor: fn(Bits6) -> BitOp| match a.checked_add(b) {
            Some(sum) => fuse(ctor(sum), &format!("{a} + {b} = {sum}")),
            None => fuse(
                Self::set_to_zero(),
                &format!("all bits are shifted out ({a} + {b} >= 64)"),
            ),
        };

        let shifts_to_mask = |op: Self| fuse(op, "the two shifts are equivalent to a mask");

        match (first, second) {
            // Two sucessive shifts in the same direction can be fused.
            (Self::ShiftLeft(a), Self::ShiftLeft(b)) => sum_shifts(a, b, Self::ShiftLeft),
            (Self::ShiftRight(a), Self::ShiftRight(b)) => sum_shifts(a, b, Self::ShiftRight),
            (Self::ArithRight(a), Self::ArithRight(b)) => match a.saturating_add(b) {
                sum @ Bits6::MAX => fuse(Self::ArithRight(sum), "{a} + {b} >= 63"),
                sum => fuse(Self::ArithRight(sum), "{a} + {b} = {sum}"),
            },
            // Two opposed shifts are equivalent to a mask.
            // Note that a left shift followed by an arithmetic right shift
            // cannot be simplified even though the reverse can be.
            (Self::ShiftLeft(a), Self::ShiftRight(b)) if a == b => {
                shifts_to_mask(Self::And(PartialBits::full(u64::MAX >> a)))
            }
            (Self::ShiftRight(a) | Self::ArithRight(a), Self::ShiftLeft(b)) if a == b => {
                shifts_to_mask(Self::And(PartialBits::full(u64::MAX << a)))
            }
            (Self::RotateRight(a), Self::RotateRight(b)) => {
                let sum = a.wrapping_add(b);
                let reason = format!("{a} + {b} = {sum} (mod 64)");
                fuse(Self::RotateRight(sum), &reason)
            }
            // Two successive masks can be fused.
            (Self::And(a), Self::And(b)) => fuse(Self::And(a & b), "masks can always be fused"),
            // A shift/mask pair that can be rewritten as two shifts can sometimes
            // be lowered to better machine code, and will never be worse.
            (Self::And(mask), Self::ShiftLeft(shl)) => {
                Self::try_fuse_mask_and_shift(mask, shl.into(), 0).unwrap_or(preserve)
            }
            (Self::ShiftLeft(shl), Self::And(mask)) => {
                Self::try_fuse_mask_and_shift(mask >> shl, shl.into(), 0).unwrap_or(preserve)
            }
            (Self::And(mask), Self::ShiftRight(shr)) => {
                Self::try_fuse_mask_and_shift(mask, 0, shr.into()).unwrap_or(preserve)
            }
            (Self::ShiftRight(shr), Self::And(mask)) => {
                Self::try_fuse_mask_and_shift(mask << shr, 0, shr.into()).unwrap_or(preserve)
            }
            _ => preserve,
        }
    }

    fn try_fuse_mask_and_shift(mask: PartialBits, shl: u8, shr: u8) -> Option<[Self; 2]> {
        debug_assert!(shl == 0 || shr == 0);
        log::trace!("OPT:      (effective pre-mask: {})", PrintBits(mask));
        log::trace!("OPT:      (shift: shl {shl}, shr {shr})");

        let fuse = |op: Self, reason: &str| {
            log::trace!("OPT:     Fused into \"{op}\", because {reason}");
            Some([Self::Nop, op])
        };

        let rewrite = |op1: Self, op2: Self, reason: &str| {
            log::trace!("OPT:     Rewritten to \"{op1}, {op2}\", because {reason}");
            Some([op1, op2])
        };

        let mask = mask.with_used((!0 >> shl) & (!0 << shr));
        let (zeros, ones) = (mask.zeros(), mask.ones());

        if zeros == 0 {
            let op = match (shl, shr) {
                (_, 0) => Self::ShiftLeft(shl.into()),
                (0, _) => Self::ShiftRight(shr.into()),
                _ => unreachable!(),
            };
            return fuse(op, "no bits that survive the shift need to be cleared");
        }

        if ones == 0 {
            let op = Self::set_to_zero();
            return fuse(op, "no bits that survive the shift need to be preserved");
        }

        // Left mask (clears contiguous low bits)
        if zeros.leading_zeros() + ones.trailing_zeros() >= 64 {
            let len = ones.trailing_zeros() as u8;
            debug_assert!(len >= shr);

            let shr_op = Self::ShiftRight(len.into());
            let shl_op = Self::ShiftLeft((len + shl - shr).into());
            let reason = format!("the mask is equivalent to clearing the low {len} bits");
            return rewrite(shr_op, shl_op, &reason);
        }

        // Right mask (clears contiguous high bits)
        if zeros.trailing_zeros() + ones.leading_zeros() >= 64 {
            let len = ones.leading_zeros() as u8;
            debug_assert!(len >= shr);

            let shl_op = Self::ShiftLeft(len.into());
            let shr_op = Self::ShiftRight((len + shr - shl).into());
            let reason = format!("the mask is equivalent to clearing the high {len} bits");
            return rewrite(shl_op, shr_op, &reason);
        }

        // The mask cannot be elided or rewritten to a shift
        None
    }
}

impl Display for BitOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Nop => write!(f, "nop"),
            Self::ShiftLeft(amt) => write!(f, "shl {amt}"),
            Self::ShiftRight(amt) => write!(f, "shr {amt}"),
            Self::ArithRight(amt) => write!(f, "sar {amt}"),
            Self::RotateRight(amt) => write!(f, "ror {amt}"),
            Self::And(mask) => write!(f, "and {}", PrintBits(mask)),
            Self::Copy(0) => write!(f, "dup <none>"),
            Self::Copy(mask) => write!(f, "dup [{}]", iter_set_bits(mask).join(", ")),
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

#[cfg(test)]
mod test {
    use super::*;
    use crate::bit_permutation::BitExtract;

    #[test]
    fn fuzz_case_1() {
        env_logger::try_init().ok();

        let extract = BitExtract::new()
            .and(0x1919191909090909 & 0xc9c9c9c9c9c9c9c9)
            .copy(0xc9c9c9c9c9c9c9c9)
            .and(0x10101000048c1c9);
        let input = 0xf8aaa2f732;

        let result = extract.exec(input);
        let result2 = extract.optimised().exec(input);

        assert_eq!(result, result2);
    }

    #[test]
    fn fuzz_case_2() {
        env_logger::try_init().ok();

        let extract = BitExtract::new().sar(9.into()).and(0xfe00000000000000).shr(7.into());
        let input = 439177129557;

        let result = extract.exec(input);
        let result2 = extract.optimised().exec(input);

        assert_eq!(result, result2);
    }

    #[test]
    fn fuzz_case_3() {
        env_logger::try_init().ok();

        let extract = BitExtract::new()
            .sar(59.into())
            .shl(13.into())
            .sar(16.into())
            .shl(63.into())
            .shr(51.into())
            .and(0x7ffffffffff)
            .shl(51.into())
            .shr(63.into())
            .copy(0xffffffffffffffff);
        let input = 18446742982821412863;

        let result = extract.exec(input);
        let result2 = extract.optimised().exec(input);

        assert_eq!(result, result2);
    }

    #[test]
    fn fuse_shifts_with_masks() {
        env_logger::try_init().ok();

        // Mask then right shift: mask only clears bits the shift discards.
        let mask = BitOp::And(0xffffffffffffff81.into());
        let shift = BitOp::ShiftRight(7.into());
        assert_eq!(BitOp::try_fuse(mask, shift), [BitOp::Nop, shift]);

        // Mask then right shift: mask clears contiguous low bits.
        let mask = BitOp::And(0xfffffffffffff000.into());
        let shift = BitOp::ShiftRight(5.into());
        assert_eq!(
            BitOp::try_fuse(mask, shift),
            [BitOp::ShiftRight(12.into()), BitOp::ShiftLeft(7.into())]
        );

        // Mask then right shift: mask cannot be elided.
        let mask = BitOp::And(0xfffffffffff08000.into());
        let shift = BitOp::ShiftRight(7.into());
        assert_eq!(BitOp::try_fuse(mask, shift), [mask, shift]);

        // Mask then left shift: mask only clears bits the shift discards.
        let mask = BitOp::And(0x00ffffffffffffff.into());
        let shift = BitOp::ShiftLeft(8.into());
        assert_eq!(BitOp::try_fuse(mask, shift), [BitOp::Nop, shift]);

        // Mask then left shift: mask clears contiguous high bits.
        let mask = BitOp::And(0x000fffffffffffff.into());
        let shift = BitOp::ShiftLeft(5.into());
        assert_eq!(
            BitOp::try_fuse(mask, shift),
            [BitOp::ShiftLeft(12.into()), BitOp::ShiftRight(7.into())]
        );

        // Mask then left shift: mask cannot be elided.
        let mask = BitOp::And(0x00010fffffffffff.into());
        let shift = BitOp::ShiftLeft(7.into());
        assert_eq!(BitOp::try_fuse(mask, shift), [mask, shift]);

        // Right shift then mask: shift already zeroed the masked-off bits.
        let shift = BitOp::ShiftRight(8.into());
        let mask = BitOp::And(0x00ffffffffffffff.into());
        assert_eq!(BitOp::try_fuse(shift, mask), [BitOp::Nop, shift]);

        // Right shift then mask: mask cannot be elided.
        let shift = BitOp::ShiftRight(7.into());
        let mask = BitOp::And(0x00fffffffffffff0.into());
        assert_eq!(BitOp::try_fuse(shift, mask), [shift, mask]);

        // Left shift then mask: shift already zeroed the masked-off bits.
        let shift = BitOp::ShiftLeft(8.into());
        let mask = BitOp::And(0xffffffffffffff00.into());
        assert_eq!(BitOp::try_fuse(shift, mask), [BitOp::Nop, shift]);

        // Left shift then mask: mask cannot be elided.
        let shift = BitOp::ShiftLeft(7.into());
        let mask = BitOp::And(0x0fffffffffffff00.into());
        assert_eq!(BitOp::try_fuse(shift, mask), [shift, mask]);
    }
}
