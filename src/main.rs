use crate::bit_permutation::{BitPermutation, BitPermutationPart};
use crate::codegen::compile_bit_permutation;

mod bit_permutation;
mod codegen;

fn main() {
    let i_imm = BitPermutation::new([
        BitPermutationPart::Slice { len: 1, src_pos: 20, repeats: 1 },
        BitPermutationPart::Slice { len: 4, src_pos: 21, repeats: 1 },
        BitPermutationPart::Slice { len: 6, src_pos: 25, repeats: 1 },
        BitPermutationPart::Slice { len: 1, src_pos: 31, repeats: 21 },
        // BitPermutationPart::Slice { len: 1, src_pos: 31, repeats: 53 },
    ]);

    let code = compile_bit_permutation(&i_imm);
    println!("{code}");

    let opt_code = compile_bit_permutation(&i_imm.optimised());
    println!("{opt_code}");
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
