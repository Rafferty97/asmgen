use asmgen::aarch64::immediate::{aarch64_logical_immediates, is_aarch64_logical_immediate};
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use itertools::Itertools;
use rand::seq::SliceRandom;

fn bench_right_mask(c: &mut Criterion) {
    let mut inputs = aarch64_logical_immediates()
        .iter()
        .flat_map(|&i| [i, i ^ 0xabcd])
        .collect_vec();
    inputs.shuffle(&mut rand::rng());

    c.bench_function("is_aarch64_logical_immediate", |b| {
        let mut iter = inputs.iter().cycle().copied();
        b.iter(|| {
            let input = iter.next().unwrap();
            is_aarch64_logical_immediate(black_box(input))
        })
    });
}

criterion_group!(benches, bench_right_mask);
criterion_main!(benches);
