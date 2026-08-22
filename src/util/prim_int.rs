use num_traits::PrimInt;

pub trait PrimIntExt: PrimInt {
    fn covers(self, rhs: Self) -> bool {
        self | rhs == self
    }
}

impl<I: PrimInt> PrimIntExt for I {}
