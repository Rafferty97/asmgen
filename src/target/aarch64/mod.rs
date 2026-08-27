use self::immediate::cost_aarch64_immediate;
use super::*;

pub mod immediate;

pub struct AArch64 {
    alu_rtp: u16,
    imul_rtp: u16,
}

impl CostModel for AArch64 {
    fn imm(&self, imm: PartialBits) -> Cost {
        let insts = cost_aarch64_immediate(imm, false);
        Cost { size: insts, latency: 0, rtp: insts * self.alu_rtp }
    }

    fn fused_shl_shr(&self) -> Cost {
        self.shift()
    }

    fn logical_rr(&self) -> Cost {
        Cost { size: 1, latency: 1, rtp: self.alu_rtp }
    }

    fn shifted_logical_rr(&self, shift: RegShift) -> Cost {
        let _ = shift;
        self.logical_rr()
    }

    fn logical_rimm(&self, imm: PartialBits) -> Cost {
        let insts = 1 + cost_aarch64_immediate(imm, true);
        Cost { size: insts, latency: 1, rtp: insts * self.alu_rtp }
    }

    fn arith_rr(&self) -> Cost {
        todo!()
    }

    fn shifted_arith_rr(&self, shift: RegShift) -> Cost {
        match shift {
            RegShift::ShiftLeft => self.arith_rr(),
            RegShift::ShiftRight => self.arith_rr(),
            RegShift::ArithRight => self.arith_rr(),
            RegShift::RotateRight => self.shift().seq(self.arith_rr()),
        }
    }

    fn arith_rimm(&self, imm: PartialBits) -> Cost {
        let insts = 1 + cost_aarch64_immediate(imm, false);
        Cost { size: insts, latency: 1, rtp: insts * self.alu_rtp }
    }

    fn imul_rr(&self) -> Cost {
        Cost { size: 1, latency: 3, rtp: self.imul_rtp }
    }
}
