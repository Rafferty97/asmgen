use std::fmt::Display;
use std::ops::{Add, Shl, Shr, Sub};

use arbitrary::Arbitrary;

/// An integer that is `N` bits wide.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Bits<const N: u32>(u32);

impl<const N: u32> Bits<N> {
    pub const ZERO: Self = Self(0);
    pub const MIN: Self = Self(0);
    pub const MAX: Self = Self((1 << N) - 1);

    pub fn new(value: u32) -> Self {
        Self(value % (1 << N))
    }

    pub fn try_new(value: u32) -> Option<Self> {
        (value <= Self::MAX.0).then_some(Self(value))
    }

    pub fn neg(self) -> Self {
        Self::new(0u32.wrapping_sub(self.0))
    }

    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        Self::try_new(self.0.checked_add(rhs.0)?)
    }

    pub fn checked_sub(self, rhs: Self) -> Option<Self> {
        Self::try_new(self.0.checked_sub(rhs.0)?)
    }

    pub fn wrapping_add(self, rhs: Self) -> Self {
        Self::new(self.0.wrapping_add(rhs.0))
    }

    pub fn wrapping_sub(self, rhs: Self) -> Self {
        Self::new(self.0.wrapping_sub(rhs.0))
    }

    pub fn saturating_add(self, rhs: Self) -> Self {
        Self::new(self.0.saturating_add(rhs.0).min(Self::MAX.0))
    }

    pub fn saturating_sub(self, rhs: Self) -> Self {
        Self::new(self.0.saturating_sub(rhs.0))
    }
}

impl<const N: u32> Add for Bits<N> {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        cfg_select! {
            debug_assertions => self.checked_add(rhs).expect("integer overflow"),
            _ => self.wrapping_add(rhs),
        }
    }
}

impl<const N: u32> Sub for Bits<N> {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        cfg_select! {
            debug_assertions => self.checked_sub(rhs).expect("integer underflow"),
            _ => self.wrapping_sub(rhs),
        }
    }
}

impl<const N: u32> Display for Bits<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

macro_rules! arith_impls {
    ($ty:ty) => {
        impl<const N: u32> From<Bits<N>> for $ty {
            fn from(value: Bits<N>) -> $ty {
                assert!(N <= <$ty>::BITS);
                value.0 as $ty
            }
        }

        impl<const N: u32> From<$ty> for Bits<N> {
            fn from(value: $ty) -> Bits<N> {
                Bits::new(value as u32)
            }
        }

        impl<const N: u32> Shl<Bits<N>> for $ty {
            type Output = $ty;

            fn shl(self, rhs: Bits<N>) -> $ty {
                self << rhs.0
            }
        }

        impl<const N: u32> Shr<Bits<N>> for $ty {
            type Output = $ty;

            fn shr(self, rhs: Bits<N>) -> $ty {
                self >> rhs.0
            }
        }
    };
}

arith_impls!(u8);
arith_impls!(u16);
arith_impls!(u32);
arith_impls!(u64);
arith_impls!(usize);
arith_impls!(i8);
arith_impls!(i16);
arith_impls!(i32);
arith_impls!(i64);
arith_impls!(isize);

impl<const N: u32> Arbitrary<'_> for Bits<N> {
    fn arbitrary(u: &mut arbitrary::Unstructured<'_>) -> arbitrary::Result<Self> {
        Ok(Self(u.int_in_range(Self::MIN.0..=Self::MAX.0)?))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    type Bits6 = Bits<6>;

    #[test]
    fn bits_max_value() {
        assert_eq!(Bits6::MIN.0, 0b000000);
        assert_eq!(Bits6::MAX.0, 0b111111);
    }

    #[test]
    fn bits_new() {
        assert_eq!(u8::from(Bits6::new(0)), 0);
        assert_eq!(u8::from(Bits6::new(63)), 63);
        assert_eq!(u8::from(Bits6::new(64)), 0);
        assert_eq!(u8::from(Bits6::new(68)), 4);

        assert_eq!(Bits6::try_new(0), Some(Bits6::new(0)));
        assert_eq!(Bits6::try_new(63), Some(Bits6::new(63)));
        assert_eq!(Bits6::try_new(64), None);
        assert_eq!(Bits6::try_new(68), None);
    }

    #[test]
    #[should_panic]
    fn invalid_into() {
        let _ = u8::from(Bits::<12>::new(40));
    }

    #[test]
    fn bits_neg() {
        assert_eq!(Bits6::neg(Bits6::new(0)), Bits6::new(0));
        assert_eq!(Bits6::neg(Bits6::new(1)), Bits6::new(63));
        assert_eq!(Bits6::neg(Bits6::new(2)), Bits6::new(62));
        assert_eq!(Bits6::neg(Bits6::new(62)), Bits6::new(2));
        assert_eq!(Bits6::neg(Bits6::new(63)), Bits6::new(1));
    }
}
