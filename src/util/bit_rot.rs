#[derive(Clone, Copy, PartialEq, Eq, Debug)]
/// A bit rotation of a 64-bit word.
/// Internally represented as a right rotation between 0 and 63.
pub struct BitRot(u8);

impl BitRot {
    pub fn new_rol(amt: u8) -> Self {
        Self(0u8.wrapping_sub(amt) % 64)
    }

    pub fn new_ror(amt: u8) -> Self {
        Self(amt % 64)
    }

    pub fn is_nop(self) -> bool {
        self.0 == 0
    }

    pub fn rol(self) -> u8 {
        0u8.wrapping_sub(self.0) % 64
    }

    pub fn ror(self) -> u8 {
        self.0
    }
}
