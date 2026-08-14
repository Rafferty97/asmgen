mod hir;
mod layout;
mod mir;

fn main() {
    let spec = hir::Spec {
        root: hir::UnitId(0),
        units: vec![hir::Unit {
            id: hir::UnitId(0),
            name: None,
            kind: hir::UnitKind::Fixed(hir::BitPattern {
                len: hir::BitCount(4),
                data: vec![0b1010],
            }),
        }],
    };

    println!("Hello, world!");
}
