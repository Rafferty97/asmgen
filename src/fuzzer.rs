use arbitrary::{Arbitrary, Unstructured};
use rand::{Rng, SeedableRng};

use crate::bit_permutation::{BitPermutation, BitPermutationPart};
use crate::codegen::{lower_bit_permutation, test_u64_to_u64};

pub fn fuzz() {
    const BASE_SEED: u64 = 0x1234_9ABC_DEF0_FEFC;
    let mut buf = [0u8; 4096];

    const START: Option<u64> = Some(14_380);

    for i in START.unwrap_or(0)..100_000 {
        println!("Test case {i}...");
        let mut rng = rand_xoshiro::Xoshiro256PlusPlus::seed_from_u64(BASE_SEED ^ i);
        rng.fill_bytes(&mut buf);
        let mut u = Unstructured::new(&buf);

        let permutation: BitPermutation = u.arbitrary().unwrap();
        let input: u64 = u.arbitrary().unwrap();

        test_u64_to_u64(
            |builder, input| lower_bit_permutation(builder, input, &permutation),
            |permute| {
                let expected = permutation.exec(input);
                let actual = permute(input);
                assert_eq!(expected, actual);
            },
        );
    }
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
