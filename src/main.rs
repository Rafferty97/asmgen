use asmgen::bit_permutation::BitPermutation;
use asmgen::codegen::compile_bit_permutation;

fn main() {
    let perm = BitPermutation::from_parts([]);
    let code = compile_bit_permutation(&perm);
    println!("{code}");
}
