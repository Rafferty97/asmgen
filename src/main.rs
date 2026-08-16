use crate::bit_permutation::{BitPermutation, BitPermutationPart};
use crate::codegen::compile_bit_permutation;

mod bit_permutation;
mod codegen;
mod playground;

fn main() {
    let i_imm = BitPermutation::new([
        BitPermutationPart::Slice { len: 11, src_pos: 20, repeats: 1 },
        BitPermutationPart::Slice { len: 1, src_pos: 31, repeats: 21 },
        BitPermutationPart::Slice { len: 1, src_pos: 31, repeats: 32 },
    ]);

    let code = compile_bit_permutation(&i_imm);
    println!("{code}");

    // let perm = BitPermutation::new([
    //     BitPermutationPart::Slice { len: 1, src_pos: 0, repeats: 16 },
    //     BitPermutationPart::Slice { len: 1, src_pos: 16, repeats: 16 },
    // ]);

    // let code = compile_bit_permutation(&perm);
    // println!("{code}");
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
