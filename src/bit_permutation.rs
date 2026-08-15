use std::cmp::Ordering;

#[derive(Clone, Debug)]
pub struct BitPermutation {
    pub len: u8,
    pub fixed: u64,
    pub signed_shift: u8,
    pub extracts: Vec<BitExtract>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BitPermutationPart {
    Fixed { len: u8, bits: u64 },
    Slice { len: u8, src_pos: u8, src_len: u8 },
}

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct BitExtract {
    pub src_pos: u8,
    pub src_len: u8,
    pub dst_pos: u8,
    pub dst_len: u8,
    pub offset: u8,
}

impl BitPermutation {
    pub fn new(parts: impl IntoIterator<Item = BitPermutationPart>) -> Self {
        let mut fixed = 0;
        let mut extracts = vec![];

        let mut dst_pos = 0;
        for part in parts.into_iter() {
            match part {
                BitPermutationPart::Fixed { len, bits } => {
                    fixed |= bits << dst_pos;
                    dst_pos += len;
                }
                BitPermutationPart::Slice { len, src_pos, src_len } => {
                    extracts.push(BitExtract {
                        src_pos,
                        src_len,
                        dst_pos,
                        dst_len: len,
                        offset: 0,
                    });
                    dst_pos += len;
                }
            }
        }

        Self { len: dst_pos, fixed, signed_shift: 0, extracts }
    }

    pub fn optimised(mut self) -> Self {
        self.optimise();
        self
    }

    pub fn optimise(&mut self) {
        // Convert repeated bit to signed shift
        if let Some(shift) = self
            .extracts
            .last()
            .filter(|e| e.src_len == 1 && e.dst_pos + e.dst_len >= self.len)
            .map(|e| (u64::BITS as u8 - e.dst_pos) - 1)
            .filter(|shift| *shift > 0)
        {
            self.fixed <<= shift;
            self.signed_shift += shift;
            self.extracts.iter_mut().for_each(|e| e.shift_left(shift));
        }

        // Merge and prune extracts
        for i in 0..(self.extracts.len() - 1) {
            let [left, right] = &mut self.extracts[i..][..2] else {
                unreachable!()
            };
            if let Some(merged) = left.try_merge(*right) {
                *left = BitExtract::default();
                *right = merged;
            }
        }
        self.extracts.retain(|e| !e.is_empty());
    }

    pub fn exec(&self, src: u64) -> u64 {
        let mut dst = self.fixed;
        for extract in &self.extracts {
            dst |= extract.exec(src);
        }
        dst = ((dst as i64) >> self.signed_shift) as u64;
        dst
    }
}

impl BitExtract {
    pub fn simple(src_pos: u8, dst_pos: u8, len: u8) -> Self {
        Self { src_pos, src_len: len, dst_pos, dst_len: len, offset: 0 }
    }

    pub fn is_empty(self) -> bool {
        self.dst_len == 0
    }

    pub fn wraps(self) -> bool {
        self.offset + self.dst_len > self.src_len
    }

    pub fn normalise(mut self) -> Self {
        self.offset = self.offset % self.src_len;
        if !self.wraps() {
            self.src_pos += self.offset;
            self.offset = 0;
        }
        self
    }

    pub fn num_repeats(self) -> u8 {
        (self.dst_len + self.offset).div_ceil(self.src_len)
    }

    pub fn nth_repeat(self, n: u8) -> Self {
        let Self { src_pos, src_len, dst_pos, dst_len, offset } = self;

        if n == 0 && offset > 0 {
            return Self::simple(src_pos + offset, dst_pos - offset, src_len - offset);
        }

        let dst_offset = (n * self.src_len) - offset;
        Self::simple(
            src_pos,
            dst_pos + dst_offset,
            src_len.min(dst_len - dst_offset),
        )
    }

    pub fn shift_left(&mut self, amount: u8) {
        self.dst_pos += amount;
        self.dst_len = self.dst_len.min(u64::BITS as u8 - self.dst_pos);
    }

    pub fn try_merge(self, other: Self) -> Option<Self> {
        println!("try merge: {self:?}, {other:?}");

        // Destination ranges must be contiguous
        if other.dst_pos != self.dst_pos + self.dst_len {
            return None;
        }

        // case 1: repeat, repeat
        //   sources must coincide, tail_len == 0
        //   extend dst_len
        // case 2: repeat, no repeat
        //   ?
        //   extend dst_len
        // case 3: no repeat, repeat
        //
        // case 4: no repeat, no repeat
        //   ?
        //   extend or repeat

        // Source offset must continue from same position
        let tail_len = self.dst_len % self.src_len;
        if other.src_pos != self.src_pos + tail_len {
            return None;
        }

        // The source ranges must be identical, or the extension must not wrap
        let same_src = self.src_pos == other.src_pos && self.src_len == other.src_len;
        let no_src_wrap = tail_len + other.dst_len <= self.src_len;
        let no_dst_wrap = other.dst_len <= other.src_len;
        if !same_src && !(no_src_wrap && no_dst_wrap) {
            return None;
        }

        Some(Self { dst_len: self.dst_len + other.dst_len, ..self })
    }

    pub fn exec(self, src: u64) -> u64 {
        let Self { src_pos, src_len, dst_pos, dst_len, offset } = self;
        let masked = src & make_mask(src_pos, src_len);

        let mut bits = 0;
        let start = (dst_pos as i32) - (offset as i32);
        let end = (dst_pos + dst_len) as i32;
        for pos in (start..end).step_by(src_len as usize) {
            bits |= shift_bits(masked, src_pos as i32, pos);
        }

        bits & make_mask(dst_pos, dst_len)
    }
}

fn make_mask(pos: u8, len: u8) -> u64 {
    if len == 0 {
        return 0;
    }
    ((1 << len) - 1) << pos
}

fn shift_bits(bits: u64, src_pos: i32, dst_pos: i32) -> u64 {
    match src_pos.cmp(&dst_pos) {
        Ordering::Equal => bits,
        Ordering::Less => bits << (dst_pos - src_pos),
        Ordering::Greater => bits >> (src_pos - dst_pos),
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_bit_permutation() {
        let permutation = BitPermutation {
            len: 10,
            fixed: 0b11,
            signed_shift: 0,
            extracts: vec![BitExtract {
                src_pos: 5,
                src_len: 3,
                dst_pos: 2,
                dst_len: 8,
                offset: 0,
            }],
        };

        assert_eq!(permutation.exec(0b00_000_00000), 0b00_000_000_11);
        assert_eq!(permutation.exec(0b11_111_11111), 0b11_111_111_11);
        assert_eq!(permutation.exec(0b00_010_01010), 0b10_010_010_11);
        assert_eq!(permutation.exec(0b11_101_10101), 0b01_101_101_11);
        assert_eq!(permutation.exec(0b01_001_11000), 0b01_001_001_11);
        assert_eq!(permutation.exec(0b10_110_00111), 0b10_110_110_11);
    }

    #[test]
    fn test_riscv_immediates() {
        // Decode permutations
        let i_imm = BitPermutation::new([
            BitPermutationPart::Slice { len: 1, src_pos: 20, src_len: 1 },
            BitPermutationPart::Slice { len: 4, src_pos: 21, src_len: 4 },
            BitPermutationPart::Slice { len: 6, src_pos: 25, src_len: 6 },
            BitPermutationPart::Slice { len: 21, src_pos: 31, src_len: 1 },
        ]);
        let i_imm = i_imm.optimised();
    }
}
