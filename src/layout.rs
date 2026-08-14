use fnv::FnvHashMap;

use crate::hir::{Spec, Unit, UnitId};

#[derive(Default, Debug)]
pub struct LayoutTables {
    pub size_align: FnvHashMap<UnitId, SizeAlign>,
    pub discriminant_offset: FnvHashMap<UnitId, u32>,
    pub variant_offset: FnvHashMap<(UnitId, UnitId), u32>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SizeAlign {
    size: u32,
    align: u32,
}

pub fn compute_layouts(spec: &Spec) -> LayoutTables {
    let mut tables = LayoutTables::default();
    let ctx = LayoutCtx {
        units: &spec.units,
        tables: &mut tables,
    };
    compute_unit_layout(ctx, &spec.units[spec.root.0 as usize]);
    tables
}

struct LayoutCtx<'a> {
    units: &'a [Unit],
    tables: &'a mut LayoutTables,
}

fn compute_unit_layout<'a>(ctx: LayoutCtx<'a>, unit: &Unit) {
    // todo
}
