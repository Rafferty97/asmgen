pub struct Func {
    pub blocks: Vec<Block>,
}

pub struct BlockId(pub u32);

pub struct Block {
    pub id: BlockId,
    pub instrs: Vec<Instr>,
}

pub enum Instr {}
