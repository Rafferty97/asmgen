use asmgen::bit_permutation::{BitPermutation, BitPermutationPart};
use asmgen::codegen::compile_bit_permutation;

fn main() {
    env_logger::init();

    let perm = BitPermutation::from_parts([
        BitPermutationPart::Repeat { len: 16, src_pos: 0 },
        BitPermutationPart::Repeat { len: 16, src_pos: 16 },
    ]);

    let code = compile_bit_permutation(&perm);
    println!("{code}");
    // movz		w6, #0x1, lsl #0x0
    // movk		w6, #0x1, lsl #0x10
    // and		x6, x0, x6
    // movz		x7, #0xffff, lsl #0x0
    // madd		x0, x6, x7, xzr
    // ret		x30

    let perm = BitPermutation::from_parts([
        BitPermutationPart::Slice { len: 16, src_pos: 10 },
        BitPermutationPart::Slice { len: 16, src_pos: 10 },
    ]);

    let code = compile_bit_permutation(&perm);
    println!("{code}");
    // ubfm		x4, x0, #10, #63
    // and		x4, x4, #0xffff
    // orr		x0, x4, x4, lsl #16
    // ret		x30

    let perm = BitPermutation::from_parts([
        BitPermutationPart::Repeat { len: 62, src_pos: 17 },
        BitPermutationPart::Repeat { len: 2, src_pos: 0 },
    ]);

    let code = compile_bit_permutation(&perm);
    println!("{code}");
    // ubfm		x7, x0, #18, #17
    // sbfm		x7, x7, #63, #63
    // and		x7, x7, #0x3fffffffffffffff
    // ubfm		x8, x0, #1, #0
    // sbfm		x8, x8, #1, #63
    // orr		x0, x7, x8
    // ret		x30

    let i_imm = BitPermutation::from_parts([
        BitPermutationPart::Slice { len: 12, src_pos: 20 },
        BitPermutationPart::Repeat { len: 52, src_pos: 31 },
    ]);

    let code = compile_bit_permutation(&i_imm);
    println!("{code}");
    // ubfm		x3, x0, #32, #31
    // sbfm		x0, x3, #52, #63
    // ret		x30

    let s_imm = BitPermutation::from_parts([
        BitPermutationPart::Slice { len: 5, src_pos: 7 },
        BitPermutationPart::Slice { len: 7, src_pos: 25 },
        BitPermutationPart::Repeat { len: 52, src_pos: 31 },
    ]);

    let code = compile_bit_permutation(&s_imm);
    println!("{code}");
    // ubfm		x9, x0, #7, #63
    // movz		w10, #0x1f, lsl #0x0
    // movk		w10, #0x100, lsl #0x10
    // and		x9, x9, x10
    // ubfm		x10, x0, #32, #31
    // sbfm		x10, x10, #52, #63
    // and		x10, x10, #0xffffffffffffffe0
    // orr		x0, x9, x10
    // ret		x30

    let b_imm = BitPermutation::from_parts([
        BitPermutationPart::Fixed { len: 1, bits: 0 },
        BitPermutationPart::Slice { len: 4, src_pos: 8 },
        BitPermutationPart::Slice { len: 6, src_pos: 25 },
        BitPermutationPart::Slice { len: 1, src_pos: 7 },
        BitPermutationPart::Repeat { len: 52, src_pos: 31 },
    ]);

    let code = compile_bit_permutation(&b_imm);
    println!("{code}");
    // ubfm		x14, x0, #7, #63
    // movz		w15, #0x1e, lsl #0x0
    // movk		w15, #0x100, lsl #0x10
    // and		x14, x14, x15
    // movz		x15, #0x800, lsl #0x0
    // movk		x15, #0x8, lsl #0x20
    // and		x15, x15, x0, lsl #4
    // orr		x14, x14, x15
    // ubfm		x15, x0, #32, #31
    // sbfm		x15, x15, #52, #63
    // movn		x0, #0x81f, lsl #0x0
    // and		x15, x15, x0
    // orr		x0, x14, x15
    // ret		x30

    let u_imm = BitPermutation::from_parts([
        BitPermutationPart::Fixed { len: 12, bits: 0 },
        BitPermutationPart::Slice { len: 20, src_pos: 12 },
        BitPermutationPart::Repeat { len: 32, src_pos: 31 },
    ]);
    // sbfm		x3, x0, #0, #31
    // and		x0, x3, #0xfffffffffffff000
    // ret		x30

    let code = compile_bit_permutation(&u_imm);
    println!("{code}");

    let j_imm = BitPermutation::from_parts([
        BitPermutationPart::Fixed { len: 1, bits: 0 },
        BitPermutationPart::Slice { len: 4, src_pos: 21 },
        BitPermutationPart::Slice { len: 6, src_pos: 25 },
        BitPermutationPart::Slice { len: 1, src_pos: 20 },
        BitPermutationPart::Slice { len: 8, src_pos: 12 },
        BitPermutationPart::Repeat { len: 52, src_pos: 31 },
    ]);

    let code = compile_bit_permutation(&j_imm);
    println!("{code}");
    // ubfm		x15, x0, #9, #63
    // movz		w1, #0x800, lsl #0x0
    // movk		w1, #0x40, lsl #0x10
    // and		x15, x15, x1
    // movz		w1, #0xf000, lsl #0x0
    // movk		w1, #0x800f, lsl #0x10
    // and		x1, x0, x1
    // orr		x15, x15, x1
    // ubfm		x0, x0, #32, #31
    // sbfm		x0, x0, #52, #63
    // movn		x1, #0xf801, lsl #0x0
    // movk		x1, #0xfff0, lsl #0x10
    // and		x0, x0, x1
    // orr		x0, x15, x0
    // ret		x30
}
