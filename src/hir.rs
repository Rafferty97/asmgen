pub struct Spec {
    pub root: UnitId,
    pub units: Vec<Unit>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct UnitId(pub u32);

pub struct Unit {
    pub id: UnitId,
    pub name: Option<String>,
    pub kind: UnitKind,
}

pub enum UnitKind {
    // Primitives
    Fixed(BitPattern),
    SignedInt(BitCount),
    UnsignedInt(BitCount),
    // Aggregates
    Enum(Vec<UnitId>),
    Compound(Vec<UnitId>),
    // Transforms
    BitPermute(BitPermute),
    FormatStr(FormatStr),
}

pub struct BitPattern {
    pub len: BitCount,
    pub data: Vec<u8>,
}

pub struct BitPermute {
    pub len: BitCount,
    pub parts: Vec<BitPermutePart>,
}

pub struct BitPermutePart {
    pub src_offset: BitPos,
    pub src_len: BitCount,
    pub dst_offset: BitPos,
}

pub struct FormatStr {
    pub lits: Vec<String>,
    pub vars: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BitPos(pub u16);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BitCount(pub u16);
