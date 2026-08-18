use cranelift_codegen::ir::{InstBuilder, Value};
use cranelift_frontend::FunctionBuilder;

use crate::bit_permutation::{BitExtract, BitOp, BitPermutation};

pub fn compile_bit_permutation(permutation: &BitPermutation) -> String {
    use cranelift_codegen::ir::types::I64;
    use cranelift_codegen::ir::{AbiParam, InstBuilder};
    use cranelift_codegen::isa;
    use cranelift_codegen::settings::{self, Configurable};
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
    use cranelift_jit::{JITBuilder, JITModule};
    use cranelift_module::{Linkage, Module};

    let mut flags = settings::builder();
    flags.set("use_colocated_libcalls", "false").unwrap();
    flags.set("is_pic", "false").unwrap();
    flags.set("opt_level", "speed").unwrap();

    let target = target_lexicon::Triple::host();
    // let target = target_lexicon::Triple::from_str("riscv64gc-unknown-unknown").unwrap();
    // let target = target_lexicon::Triple::from_str("x86_64-unknown-unknown").unwrap();
    let isa = isa::lookup(target).unwrap();
    let isa = isa.finish(settings::Flags::new(flags)).unwrap();
    let frontend_config = isa.frontend_config();

    let jit_builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
    let mut module = JITModule::new(jit_builder);

    let mut ctx = module.make_context();
    ctx.func.signature.params.push(AbiParam::new(I64));
    ctx.func.signature.returns.push(AbiParam::new(I64));
    ctx.set_disasm(true);

    let func_id = module
        .declare_function("my_func", Linkage::Export, &ctx.func.signature)
        .unwrap();

    let mut fb_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);

    let entry = builder.create_block();
    builder.append_block_params_for_function_params(entry);
    builder.switch_to_block(entry);
    builder.seal_block(entry);

    let arg = builder.block_params(entry)[0];
    let result = lower_bit_permutation(&mut builder, arg, permutation);
    builder.ins().return_(&[result]);

    builder.finalize(frontend_config);

    module.define_function(func_id, &mut ctx).unwrap();
    let output = ctx.compiled_code().unwrap().code_buffer().to_vec();
    let output = format!("{}", Code(output));

    module.clear_context(&mut ctx);
    unsafe { module.free_memory() };

    output
}

pub fn lower_bit_permutation(
    builder: &mut FunctionBuilder,
    src: Value,
    permutation: &BitPermutation,
) -> Value {
    use cranelift_codegen::ir::types::I64;

    let (fixed, extracts) = permutation.compile();

    let mut result = builder.ins().iconst(I64, fixed as i64);
    for extract in extracts {
        let bits = lower_bit_extract(builder, src, &extract);
        result = builder.ins().bor(result, bits);
    }
    result
}

fn lower_bit_extract(builder: &mut FunctionBuilder, value: Value, extract: &BitExtract) -> Value {
    let mut value = value;
    for &op in extract.ops() {
        value = lower_bit_op(builder, value, op);
    }
    value
}

fn lower_bit_op(builder: &mut FunctionBuilder, value: Value, op: BitOp) -> Value {
    match op {
        BitOp::ShiftLeft(amt) => builder.ins().ishl_imm_u(value, amt as i64),
        BitOp::ShiftRight(amt) => builder.ins().ushr_imm_u(value, amt as i64),
        BitOp::ArithRight(amt) => builder.ins().sshr_imm_u(value, amt as i64),
        BitOp::RotateRight(amt) => builder.ins().rotr_imm_u(value, amt as i64),
        BitOp::And { mask, used } => builder.ins().band_imm_u(value, (mask & used) as i64), // fixme: not always optimal
        BitOp::ShiftOr(0, amt) => {
            let shifted = builder.ins().ishl_imm_u(value, amt as i64);
            builder.ins().bor(value, shifted)
        }
        BitOp::ShiftOr(amt1, amt2) => {
            let shifted1 = builder.ins().ishl_imm_u(value, amt1 as i64);
            let shifted2 = builder.ins().ishl_imm_u(value, amt2 as i64);
            builder.ins().bor(shifted1, shifted2)
        }
        BitOp::Mul(mask) => builder.ins().imul_imm_u(value, mask as i64),
    }
}

struct Code(Vec<u8>);

impl std::fmt::Display for Code {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for offset in (0..self.0.len()).step_by(4) {
            let ins = u32::from_le_bytes([
                self.0[offset],
                self.0[offset + 1],
                self.0[offset + 2],
                self.0[offset + 3],
            ]);
            match disarm64::decoder_full::decode(ins) {
                Some(ins) => writeln!(f, "{ins}")?,
                None => writeln!(f, "{{bad}}")?,
            }
        }
        Ok(())
    }
}

pub fn test_u64_to_u64(
    body: impl FnOnce(&mut FunctionBuilder, Value) -> Value,
    tests: impl FnOnce(&dyn Fn(u64) -> u64),
) {
    use cranelift_codegen::ir::types::I64;
    use cranelift_codegen::ir::{AbiParam, InstBuilder};
    use cranelift_codegen::isa;
    use cranelift_codegen::settings::{self, Configurable};
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
    use cranelift_jit::{JITBuilder, JITModule};
    use cranelift_module::{Linkage, Module};

    let mut flags = settings::builder();
    flags.set("use_colocated_libcalls", "false").unwrap();
    flags.set("is_pic", "false").unwrap();
    flags.set("opt_level", "speed").unwrap();

    let isa = isa::lookup(target_lexicon::Triple::host()).unwrap();
    let isa = isa.finish(settings::Flags::new(flags)).unwrap();
    let frontend_config = isa.frontend_config();

    let jit_builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
    let mut module = JITModule::new(jit_builder);

    let mut ctx = module.make_context();
    ctx.func.signature.params.push(AbiParam::new(I64));
    ctx.func.signature.returns.push(AbiParam::new(I64));
    // ctx.set_disasm(true);

    let func_id = module
        .declare_function("my_func", Linkage::Export, &ctx.func.signature)
        .unwrap();

    let mut fb_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);

    let entry = builder.create_block();
    builder.append_block_params_for_function_params(entry);
    builder.switch_to_block(entry);
    builder.seal_block(entry);

    let arg = builder.block_params(entry)[0];
    let result = body(&mut builder, arg);
    builder.ins().return_(&[result]);

    builder.finalize(frontend_config);

    module.define_function(func_id, &mut ctx).unwrap();
    // let output = ctx.compiled_code().unwrap().code_buffer().to_vec();
    // let output = format!("{}", Code(output));
    // println!("{output}");
    module.clear_context(&mut ctx);

    module.finalize_definitions().unwrap();
    let code_ptr = module.get_finalized_function(func_id);
    let func = unsafe { std::mem::transmute::<_, fn(u64) -> u64>(code_ptr) };

    tests(&func);

    unsafe { module.free_memory() };
}

#[cfg(test)]
mod test {
    use crate::bit_permutation::BitPermutationPart;

    use super::*;
    use cranelift_codegen::ir::InstBuilder;

    #[test]
    fn basic_test() {
        test_u64_to_u64(
            |builder, input| builder.ins().imul_imm_u(input, 2),
            |func| assert_eq!(func(12), 24),
        );
    }

    #[test]
    fn fuzz_case1() {
        let mut perm = BitPermutation::new();
        perm.push(BitPermutationPart::Repeat { len: 62, src_pos: 17 });
        perm.push(BitPermutationPart::Repeat { len: 2, src_pos: 0 });
        test_bit_perm(&perm, &[0x3b0a083b850f0e0e]);
    }

    #[test]
    fn extract_riscv5_i_imm() {
        let mut perm = BitPermutation::new();
        perm.push(BitPermutationPart::Slice { len: 12, src_pos: 20 });
        perm.push(BitPermutationPart::Repeat { len: 52, src_pos: 31 });
        test_bit_perm(&perm, &[0, 0x3b0a083b850f0e0e]);
    }

    #[test]
    fn lower_bit_extract_test() {
        // let extract = BitExtract::ShiftMul { mask: 0b111_00000, rshift: 5, mul: 0b100 };
        let extract = BitExtract::new().shr(3).and(0b11100);
        let cases = [
            (0b00_000_00000, 0b000_00),
            (0b11_111_11111, 0b111_00),
            (0b00_010_01010, 0b010_00),
            (0b11_101_10101, 0b101_00),
            (0b01_001_11000, 0b001_00),
            (0b10_110_00111, 0b110_00),
        ];
        test_bit_extract(&extract, &cases);
    }

    fn test_bit_perm(perm: &BitPermutation, inputs: &[u64]) {
        // Common inputs for every test
        const INPUTS: &[u64] = &[0, u64::MAX];

        test_u64_to_u64(
            |builder, input| lower_bit_permutation(builder, input, perm),
            |exec| {
                for &input in inputs.iter().chain(INPUTS) {
                    let expected = perm.exec(input);
                    let actual = exec(input);
                    assert_eq!(expected, actual);
                }
            },
        );
    }

    fn test_bit_extract(extract: &BitExtract, cases: &[(u64, u64)]) {
        test_u64_to_u64(
            |builder, input| lower_bit_extract(builder, input, extract),
            |exec| {
                for &(input, output) in cases {
                    assert_eq!(exec(input), output);
                }
            },
        );
    }
}
