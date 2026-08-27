use super::*;

pub struct X86 {}

impl CostModel for X86 {
    fn imm(&self, imm: PartialBits) -> Cost {
        let _ = imm;
        todo!()
    }

    fn logical_rr(&self) -> Cost {
        todo!()
    }

    fn arith_rr(&self) -> Cost {
        todo!()
    }

    fn imul_rr(&self) -> Cost {
        todo!()
    }
}
