use num_traits::PrimInt;

#[inline(always)]
pub fn left_mask<I: PrimInt>(len: usize) -> I {
    match len.into() {
        ..64 => !(I::max_value() >> len),
        64.. => I::max_value(),
    }
}

#[inline(always)]
pub fn right_mask<I: PrimInt>(len: usize) -> I {
    match len.into() {
        ..64 => (I::one() << len) - I::one(),
        64.. => I::max_value(),
    }
}

#[inline(always)]
pub fn middle_mask<I: PrimInt>(pos: usize, len: usize) -> I {
    match (pos, len) {
        (..64, ..64) => ((I::one() << len) - I::one()) << pos,
        (..64, 64..) => I::max_value() << pos,
        (64.., _) => I::zero(),
    }
}
