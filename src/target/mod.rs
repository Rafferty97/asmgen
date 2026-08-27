use crate::util::PartialBits;

pub mod aarch64;
pub mod x86;

pub trait CostModel {
    fn imm(&self, imm: PartialBits) -> Cost;

    fn shift(&self) -> Cost {
        self.logical_rr()
    }

    fn fused_shl_shr(&self) -> Cost {
        self.shift().seq(self.shift())
    }

    fn logical_rr(&self) -> Cost;

    fn shifted_logical_rr(&self, shift: RegShift) -> Cost {
        let _ = shift;
        self.shift().seq(self.logical_rr())
    }

    fn logical_rimm(&self, imm: PartialBits) -> Cost {
        self.imm(imm).seq(self.logical_rr())
    }

    fn arith_rr(&self) -> Cost;

    fn shifted_arith_rr(&self, shift: RegShift) -> Cost {
        let _ = shift;
        self.shift().seq(self.arith_rr())
    }

    fn arith_rimm(&self, imm: PartialBits) -> Cost {
        self.imm(imm).seq(self.arith_rr())
    }

    fn imul_rr(&self) -> Cost;

    fn imul_rimm(&self, imm: PartialBits) -> Cost {
        let imm = self.imm(imm);
        let imul_rr = self.imul_rr();
        imm.seq(imul_rr)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RegShift {
    ShiftLeft,
    ShiftRight,
    ArithRight,
    RotateRight,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cost {
    pub size: u16,
    pub latency: u16,
    pub rtp: u16,
}

impl Cost {
    pub fn seq(self, rhs: Self) -> Self {
        Self {
            size: self.size + rhs.size,
            latency: self.latency + rhs.latency,
            rtp: self.rtp + rhs.rtp,
        }
    }

    pub fn par(self, rhs: Self) -> Self {
        Self {
            size: self.size + rhs.size,
            latency: self.latency.max(rhs.latency),
            rtp: self.rtp + rhs.rtp,
        }
    }
}
