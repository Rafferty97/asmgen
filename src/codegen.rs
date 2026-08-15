use std::cmp::Ordering;

use cranelift_codegen::ir::{InstBuilder, Value};
use cranelift_frontend::FunctionBuilder;

use crate::bit_permutation::{BitExtract, BitPermutation};

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

    let isa = isa::lookup(target_lexicon::Triple::host()).unwrap();
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
    let output = ctx.compiled_code().unwrap().vcode.clone().unwrap();

    module.clear_context(&mut ctx);
    unsafe { module.free_memory() };

    output
}

fn lower_bit_permutation(
    builder: &mut FunctionBuilder,
    src: Value,
    permutation: &BitPermutation,
) -> Value {
    use cranelift_codegen::ir::types::I64;

    let mut result = builder.ins().iconst(I64, permutation.fixed as i64);

    for &extract in &permutation.extracts {
        let bits = lower_bit_extract(builder, src, extract);
        result = builder.ins().bor(result, bits);
    }

    result
}

fn lower_bit_extract(builder: &mut FunctionBuilder, src: Value, extract: BitExtract) -> Value {
    let bits = builder.ins().band_imm_u(src, extract.mask as i64);
    shift_bits(builder, bits, 0, extract.shift as i64)
}

// fn lower_bit_extract(builder: &mut FunctionBuilder, src: Value, extract: BitExtract) -> Value {
//     let (src_pos, src_len) = (extract.src_pos as i64, extract.src_len as i64);
//     let (dst_pos, dst_len) = (extract.dst_pos as i64, extract.dst_len as i64);

//     match (dst_len / src_len, dst_len % src_len) {
//         // No repetition
//         (0, _) | (1, 0) => {
//             let mask = ((1 << dst_len) - 1) << src_pos;
//             let bits = builder.ins().band_imm_u(src, mask);
//             shift_bits(builder, bits, src_pos, dst_pos)
//         }
//         // Repetition with two shift and masks
//         (1, _) => {
//             let rep1 = lower_bit_extract(builder, src, extract.nth_repeat(0));
//             let rep2 = lower_bit_extract(builder, src, extract.nth_repeat(1));
//             builder.ins().bor(rep1, rep2)
//         }
//         // Repetition with a mask and two shifts
//         (2, 0) => {
//             let mask = ((1 << src_len) - 1) << src_pos;
//             let bits = builder.ins().band_imm_u(src, mask);
//             let rep1 = shift_bits(builder, bits, src_pos, dst_pos);
//             let rep2 = shift_bits(builder, bits, src_pos, dst_pos + src_len);
//             builder.ins().bor(rep1, rep2)
//         }
//         // Repetition with multiply
//         (_, _) => {
//             let bits = builder.ins().ushr_imm_u(src, src_pos);
//             let bits = builder.ins().band_imm_u(bits, (1 << src_len) - 1);
//             let multiplicand = (0..dst_len)
//                 .step_by(src_len as usize)
//                 .map(|i| 1 << i)
//                 .fold(0, |a, b| a | b);
//             let bits = builder.ins().imul_imm_u(bits, multiplicand << dst_pos);
//             if (dst_len % src_len) != 0 && (dst_pos + dst_len) < 64 {
//                 let mask = ((1 << dst_len) - 1) << dst_pos;
//                 builder.ins().band_imm_u(bits, mask)
//             } else {
//                 bits
//             }
//         }
//     }
// }

fn shift_bits(builder: &mut FunctionBuilder, src: Value, src_pos: i64, dst_pos: i64) -> Value {
    match src_pos.cmp(&dst_pos) {
        Ordering::Equal => src,
        Ordering::Less => builder.ins().ishl_imm_u(src, dst_pos - src_pos),
        Ordering::Greater => builder.ins().ushr_imm_u(src, src_pos - dst_pos),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use cranelift_codegen::ir::{InstBuilder, Value};
    use cranelift_frontend::FunctionBuilder;

    #[test]
    fn basic_test() {
        test_u64_to_u64(
            |builder, input| builder.ins().imul_imm_u(input, 2),
            |func| assert_eq!(func(12), 24),
        );
    }

    #[test]
    fn lower_bit_extract_test() {
        let extract = BitExtract { mask: 0b111_00000, shift: -3 };
        let cases = [
            (0b00_000_00000, 0b000_00),
            (0b11_111_11111, 0b111_00),
            (0b00_010_01010, 0b010_00),
            (0b11_101_10101, 0b101_00),
            (0b01_001_11000, 0b001_00),
            (0b10_110_00111, 0b110_00),
        ];
        test_bit_extract(extract, &cases);
    }

    fn test_bit_extract(extract: BitExtract, cases: &[(u64, u64)]) {
        test_u64_to_u64(
            |builder, input| lower_bit_extract(builder, input, extract),
            |exec| {
                for &(input, output) in cases {
                    assert_eq!(exec(input), output);
                }
            },
        );
    }

    fn test_u64_to_u64(
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
        let result = body(&mut builder, arg);
        builder.ins().return_(&[result]);

        builder.finalize(frontend_config);

        module.define_function(func_id, &mut ctx).unwrap();
        module.clear_context(&mut ctx);

        module.finalize_definitions().unwrap();
        let code_ptr = module.get_finalized_function(func_id);
        let func = unsafe { std::mem::transmute::<_, fn(u64) -> u64>(code_ptr) };

        tests(&func);

        unsafe { module.free_memory() };
    }
}
