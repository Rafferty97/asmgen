use itertools::Itertools;
use thiserror::Error;

use crate::bit_permutation::{BitExtract, PermInput, compile_permutation};

pub mod bit_permutation;
pub mod codegen;
pub mod peephole;
pub mod target;
pub mod util;

pub struct BitwiseMap {
    /// Number of bits in the source bit vector.
    src_len: usize,
    /// Number of bits in the destination bit vector.
    dst_len: usize,
    /// Number of variables in `data`.
    num_vars: usize,
    /// The data, with 7 bytes of trailing zeros for padding.
    ///
    /// Layout:
    /// - src XOR mask
    /// - dst XOR mast
    /// - each variable, comprising:
    ///   - length (u16)
    ///   - src offset mask
    ///   - dst offset mask
    data: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
struct BitwiseMapVar<'a> {
    len: u16,
    src_offsets: BitsView<'a>,
    dst_offsets: BitsView<'a>,
}

impl BitwiseMap {
    pub fn new<'a, I>(src_xor_mask: BitsView, dst_xor_mask: BitsView, vars: I) -> Self
    where
        I: IntoIterator<Item = BitwiseMapVar<'a>>,
    {
        let src_len = src_xor_mask.len;
        let dst_len = dst_xor_mask.len;
        let mut num_vars = 0;
        let mut data = vec![];

        data.extend(src_xor_mask.data());
        data.extend(src_xor_mask.data());

        for var in vars {
            assert_eq!(var.src_offsets.len(), src_len);
            assert_eq!(var.dst_offsets.len(), dst_len);
            data.extend(var.len.to_le_bytes());
            data.extend(var.src_offsets.data());
            data.extend(var.dst_offsets.data());
            num_vars += 1;
        }

        Self { src_len, dst_len, num_vars, data }
    }

    pub fn invert(&self) -> Self {
        Self::new(
            self.dst_xor_mask(),
            self.src_xor_mask(),
            self.vars().map(|v| v.invert()),
        )
    }

    pub fn src_xor_mask(&self) -> BitsView {
        self.bits().extract(0, self.src_len)
    }

    pub fn dst_xor_mask(&self) -> BitsView {
        self.bits().extract(self.src_len.div_ceil(8), self.dst_len)
    }

    pub fn get_var(&self, index: usize) -> BitwiseMapVar {
        let src_bytes = self.src_len.div_ceil(8);
        let dst_bytes = self.dst_len.div_ceil(8);
        let offset = (index + 1) * (src_bytes + dst_bytes) + index * 2;
        let len = u16::from_le_bytes([self.data[offset], self.data[offset + 1]]);
        let src_offsets = self.bits().extract(offset + 2, self.src_len);
        let dst_offsets = self.bits().extract(offset + 2 + src_bytes, self.dst_len);
        BitwiseMapVar { len, src_offsets, dst_offsets }
    }

    pub fn vars(&self) -> impl Iterator<Item = BitwiseMapVar> {
        (0..self.num_vars).map(|i| self.get_var(i as usize))
    }

    pub fn to_permutation(&self, rev: bool) -> (BitsView, Vec<BitExtract>) {
        // needs:
        // - shift candidates -> iterate vars, use first src offset, emit dst offsets
        // - broadcast candidates -> vars with popcnt(dst) > 1
        // - repeat candidates

        let xor_mask = match rev {
            false => self.dst_xor_mask(),
            true => self.src_xor_mask(),
        };
        let extracts = compile_permutation(self.vars().map(|v| v.to_perm_input(rev)));

        (xor_mask, extracts)
    }

    pub fn src_constraints() {
        todo!()
    }

    pub fn dst_constraints() {
        todo!()
    }

    fn bits(&self) -> BitsView {
        BitsView::from_bytes(&self.data)
    }

    fn extract_bits(&self, p: usize, n: usize) -> u64 {
        debug_assert!(n <= 64);
        let mut buf = [0; 8];
        buf.copy_from_slice(&self.data[p..][..8]);
        u64::from_le_bytes(buf)
    }
}

impl<'a> BitwiseMapVar<'a> {
    pub fn invert(&self) -> Self {
        Self {
            len: self.len,
            src_offsets: self.dst_offsets,
            dst_offsets: self.src_offsets,
        }
    }

    fn to_perm_input(&self, rev: bool) -> bit_permutation::PermInput {
        // fixme: truncation, assumes 64-bits, etc

        let (src_offsets, dst_offsets) = match rev {
            false => (self.src_offsets, self.dst_offsets),
            true => (self.dst_offsets, self.src_offsets),
        };

        let len = self.len as u8;
        let src_pos = src_offsets.leading_zeros() as u8;
        let dst_mask = dst_offsets.try_into().unwrap();

        PermInput { len, src_pos, dst_mask }
    }
}

#[derive(Clone, Copy, Debug)]
struct BitsView<'a> {
    len: usize,
    data: &'a [u8],
}

impl<'a> BitsView<'a> {
    pub fn from_bytes(data: &'a [u8]) -> Self {
        Self { len: 8 * data.len(), data }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    pub fn extract(&self, pos: usize, len: usize) -> Self {
        let data = &self.data[pos..][..len.div_ceil(8)];
        Self { len, data }
    }

    pub fn leading_zeros(&self) -> u32 {
        match self.data().iter().find_position(|&&b| b != 0) {
            Some((index, &byte)) => 8 * (index as u32) + byte.leading_zeros(),
            None => self.len as u32,
        }
    }

    pub fn trailing_zeros(&self) -> u32 {
        match self.data().iter().rev().find_position(|&&b| b != 0) {
            Some((index, &byte)) => 8 * (index as u32) + byte.trailing_zeros(),
            None => self.len as u32,
        }
    }
}

#[derive(Error, Debug)]
#[error("BitView is too large for type")]
struct FromBitsViewError;

impl TryFrom<BitsView<'_>> for u64 {
    type Error = FromBitsViewError;

    fn try_from(value: BitsView) -> Result<Self, FromBitsViewError> {
        if value.len > u64::BITS as usize {
            return Err(FromBitsViewError);
        }
        let mut buf = [0; 8];
        buf[..value.data.len()].copy_from_slice(value.data);
        Ok(u64::from_le_bytes(buf))
    }
}
