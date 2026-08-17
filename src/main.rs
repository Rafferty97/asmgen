use crate::bit_permutation::{BitPermutation, BitPermutationPart};
use crate::codegen::compile_bit_permutation;

mod bit_permutation;
mod codegen;
mod fuzzer;
mod playground;

fn main() {
    fuzzer::fuzz();
}

fn main2() {
    let i_imm = BitPermutation::from_parts([
        BitPermutationPart::Slice { len: 12, src_pos: 20 },
        BitPermutationPart::Repeat { len: 20, src_pos: 31 },
        BitPermutationPart::Repeat { len: 32, src_pos: 31 },
    ]);

    let code = compile_bit_permutation(&i_imm);
    println!("{code}");

    let perm = BitPermutation::from_parts([
        BitPermutationPart::Repeat { len: 16, src_pos: 0 },
        BitPermutationPart::Repeat { len: 16, src_pos: 16 },
    ]);
    // extr		x3, x0, x0, #32
    // sbfm		x0, x3, #52, #63
    // ret		x30

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
}

struct Decoder {
    blocks: Vec<Block>,
}

struct Block {
    insts: Vec<Inst>,
}

enum Inst {
    Read(BitExtract),
    Advance(u32),
    WriteStr(String),
    WriteInt { value: InstId, base: u8, signed: bool },
}

struct BlockId(pub u32);

struct InstId(pub u32);

struct BitExtract(Vec<BitExtractPart>);

enum BitExtractPart {
    Read { offset: u32, len: u32 },
    Fixed(BitPattern),
}

struct BitPattern {
    len: u32,
    data: Vec<u8>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum Value {
    Scalar(u64),
    Variant(u64, Box<Value>),
    Tuple(Vec<Value>),
}
