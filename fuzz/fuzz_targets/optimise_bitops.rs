#![no_main]
use arbitrary::Arbitrary;
use asmgen::bit_permutation::BitExtract;
use asmgen::codegen::{lower_bit_permutation, test_u64_to_u64};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: (BitExtract, u64)| {
    env_logger::try_init().ok();

    let (extract, input) = input;
    // println!("==== Test case ====");
    // println!("Extract: {extract}");
    // println!("Input: {input:#x}");

    let optimised = extract.clone().optimised();

    // println!("Original: {extract}");
    // println!("Optimised: {optimised}");

    let expected = extract.exec(input);
    let actual = optimised.exec(input);
    assert_eq!(expected, actual);
});
