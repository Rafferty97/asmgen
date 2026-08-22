//! Criterion benchmark for `lower_bit_permutation`.
//!
//! Corpus generation mirrors the fuzzer exactly (same base seed, same
//! `seed_from_u64(BASE_SEED ^ i)` scheme), so a case index reported here can be
//! fed straight back into `fuzz()`'s `START` to reproduce it.
//!
//!   cargo bench --bench lower_bit_permutation
//!   ASMGEN_DIST=1 cargo bench --bench lower_bit_permutation
//!
//! The second form additionally prints a per-permutation latency distribution
//! (percentiles + the slowest cases with their fuzz indices).

use std::env;
use std::hint::black_box;
use std::time::{Duration, Instant};

use arbitrary::{Arbitrary, Unstructured};
use cranelift_codegen::settings;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rand::{Rng, SeedableRng};
use rand_xoshiro::Xoshiro256PlusPlus;

use cranelift_codegen::ir::{AbiParam, Function, InstBuilder, Signature, UserFuncName, types};
use cranelift_codegen::isa::{self, CallConv};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};

use asmgen::bit_permutation::BitPermutation;
use asmgen::codegen::lower_bit_permutation;

const BASE_SEED: u64 = 0x1234_9ABC_DEF0_FEFC;
const CORPUS_LEN: usize = 2048;
const DIST_REPS: usize = 25;

// ---------------------------------------------------------------------------
// Corpus
// ---------------------------------------------------------------------------

struct Case {
    /// Fuzz case index — reproduce with `fuzz()`'s `START = index`.
    index: u64,
    permutation: BitPermutation,
    bits: usize,
}

fn make_case(index: u64) -> Option<Case> {
    let mut buf = [0u8; 4096];
    let mut rng = Xoshiro256PlusPlus::seed_from_u64(BASE_SEED ^ index);
    rng.fill_bytes(&mut buf);
    let mut u = Unstructured::new(&buf);
    let permutation = BitPermutation::arbitrary(&mut u).ok()?;
    let bits = permutation.len() as usize;
    Some(Case { index, permutation, bits })
}

fn corpus(n: usize) -> Vec<Case> {
    (0u64..).filter_map(make_case).take(n).collect()
}

/// Round a permutation width up to the next power of two, giving seven buckets
/// over 1..=64. Swap this for part count if you have an accessor — that's
/// likely the better independent variable for candidate selection.
fn width_bucket(bits: usize) -> Option<u32> {
    if bits == 0 {
        None
    } else {
        Some((bits as u32).next_power_of_two())
    }
}

// ---------------------------------------------------------------------------
// Timing harness
// ---------------------------------------------------------------------------

fn u64_to_u64_sig() -> Signature {
    let mut sig = Signature::new(CallConv::SystemV);
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    sig
}

/// Time a single lowering. Builder construction, block setup, `finalize` and
/// teardown all sit outside the measured window.
fn time_one(permutation: &BitPermutation) -> Duration {
    let mut fb_ctx = FunctionBuilderContext::new();
    let mut func = Function::with_name_signature(UserFuncName::user(0, 0), u64_to_u64_sig());
    let mut builder = FunctionBuilder::new(&mut func, &mut fb_ctx);

    let target = target_lexicon::Triple::host();
    let isa = isa::lookup(target).unwrap();
    let isa = isa
        .finish(settings::Flags::new(settings::builder()))
        .unwrap();
    let frontend_config = isa.frontend_config();

    let block = builder.create_block();
    builder.append_block_params_for_function_params(block);
    builder.switch_to_block(block);
    builder.seal_block(block);
    let input = builder.block_params(block)[0];

    let start = Instant::now();
    let output = lower_bit_permutation(&mut builder, black_box(input), black_box(permutation));
    let elapsed = start.elapsed();

    builder.ins().return_(&[output]);
    builder.finalize(frontend_config);
    black_box(func);

    elapsed
}

/// One untimed pass so the allocator has grown and any lazily-initialised
/// tables are populated before the first measurement.
fn warm(corpus: &[Case]) {
    for case in corpus {
        black_box(time_one(&case.permutation));
    }
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

/// Mean cost per permutation across the whole corpus. One Criterion iteration
/// is a full pass, so every sample covers an identical set of inputs and the
/// reported CI reflects measurement noise rather than corpus sampling. Read the
/// per-element throughput line, not the per-iteration time.
fn bench_corpus_mean(c: &mut Criterion) {
    let corpus = corpus(CORPUS_LEN);
    warm(&corpus);

    let mut g = c.benchmark_group("lower_bit_permutation");
    g.throughput(Throughput::Elements(corpus.len() as u64));
    g.bench_function("corpus_pass", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                for case in &corpus {
                    total += time_one(&case.permutation);
                }
            }
            total
        })
    });
    g.finish();
}

/// Sweep over permutation width. Each bucket is a full pass over its members.
fn bench_by_width(c: &mut Criterion) {
    let corpus = corpus(CORPUS_LEN);
    warm(&corpus);

    let mut g = c.benchmark_group("lower_bit_permutation/by_width");
    for width in [1u32, 2, 4, 8, 16, 32, 64] {
        let bucket: Vec<&Case> = corpus
            .iter()
            .filter(|c| width_bucket(c.bits) == Some(width))
            .collect();
        if bucket.is_empty() {
            continue;
        }

        g.throughput(Throughput::Elements(bucket.len() as u64));
        g.bench_with_input(BenchmarkId::from_parameter(width), &bucket, |b, bucket| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    for case in bucket.iter() {
                        total += time_one(&case.permutation);
                    }
                }
                total
            })
        });
    }
    g.finish();
}

// ---------------------------------------------------------------------------
// Distribution report
// ---------------------------------------------------------------------------

fn percentile(sorted: &[(Duration, u64, usize)], q: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let idx = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[idx].0
}

/// Criterion aggregates; it will not tell you that case 41 337 is 400x the
/// median. This pass takes the median of `DIST_REPS` timings per permutation,
/// then reports the spread across permutations.
fn dist_report(_c: &mut Criterion) {
    if env::var_os("ASMGEN_DIST").is_none() {
        return;
    }

    let corpus = corpus(CORPUS_LEN);
    warm(&corpus);

    let mut samples: Vec<(Duration, u64, usize)> = corpus
        .iter()
        .map(|case| {
            let mut reps: Vec<Duration> = (0..DIST_REPS)
                .map(|_| time_one(&case.permutation))
                .collect();
            reps.sort_unstable();
            (reps[DIST_REPS / 2], case.index, case.bits)
        })
        .collect();
    samples.sort_unstable();

    let sum: Duration = samples.iter().map(|s| s.0).sum();
    let mean = sum / samples.len() as u32;

    println!("\nlower_bit_permutation: per-permutation distribution");
    println!("  n     = {}", samples.len());
    println!("  mean  = {mean:?}");
    for (label, q) in [
        ("min ", 0.00),
        ("p25 ", 0.25),
        ("p50 ", 0.50),
        ("p75 ", 0.75),
        ("p90 ", 0.90),
        ("p99 ", 0.99),
        ("max ", 1.00),
    ] {
        println!("  {label} = {:?}", percentile(&samples, q));
    }

    println!("  slowest cases (fuzz index / width / median):");
    for (dur, index, bits) in samples.iter().rev().take(10) {
        println!("    #{index:<8} {bits:>2} bits  {dur:?}");
    }
    println!();
}

criterion_group!(benches, dist_report, bench_corpus_mean, bench_by_width);
criterion_main!(benches);
