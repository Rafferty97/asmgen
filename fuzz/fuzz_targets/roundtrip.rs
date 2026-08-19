#![no_main]
use arbitrary::Arbitrary;
use asmgen::bit_permutation::BitPermutation;
use asmgen::codegen::{lower_bit_permutation, test_u64_to_u64};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: (BitPermutation, u64)| {
    let (perm, input) = input;
    test_u64_to_u64(
        |builder, input| lower_bit_permutation(builder, input, &perm),
        |permute| {
            println!("testing {input:?}");
            let expected = perm.exec(input);
            let actual = permute(input);
            println!("exp = {expected}, act = {actual}");
            assert_eq!(expected, actual);
        },
    );
});
