//! Cranelift-backed JIT compiler for `CompiledGraph` block execution.
//!
//! Preserves bit-identical `binary64` semantics with the Rust interpreter:
//! - FMA contraction is disabled (`enable_fma = false` where supported).
//! - Transcendentals and complex processors call Rust `extern "C"` helpers.
//! - Frame 0 is peeled so non-modulated parameter validation runs in exact
//!   topological order matching the interpreter's first-error precedence.

use cranelift_codegen::ir::condcodes::FloatCC;
use cranelift_codegen::ir::types::{F64, I32, I64, I8};
use cranelift_codegen::ir::{AbiParam, InstBuilder, MemFlagsData, UserFuncName};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module};
use std::ffi::c_void;
use std::sync::Arc;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct JitExpressionInput {
    pub pitch_hz: f64,
    pub timbre: f64,
    pub pressure: f64,
}

#[repr(C)]
pub struct JitBlockContext {
    pub runtime_nodes_ptr: *const *mut c_void,
    pub runtime_params_ptr: *const *mut f64,
    pub inline_state_ptr: *mut u64,
    pub compiled_nodes_ptr: *const *const c_void,
    pub controls_ptr: *const f64,
    pub controls_len: usize,
    pub expressions_ptr: *const JitExpressionInput,
    pub input_ptr: *const [f64; 2],
    pub output_ptr: *mut [f64; 2],
    pub error_out: *mut c_void,
    pub start_frame: u64,
    pub frames: usize,
    pub rate: f64,
    pub settled: u8,
}

pub type JitRenderBlockFn = unsafe extern "C" fn(*mut JitBlockContext) -> usize;

/// Opaque audio source for a node input or modulation inside a `CompiledGraph`.
#[derive(Clone, Copy, Debug)]
pub enum JitSource {
    Input,
    Node(usize),
}

#[derive(Clone, Copy, Debug)]
pub struct JitControlBinding {
    pub parameter: usize,
    pub control: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct JitModSpec {
    pub mod_index: usize,
    pub parameter: usize,
    pub source: JitSource,
    pub depth: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct JitBoundSpec {
    pub parameter: usize,
    pub min: f64,
    pub max: f64,
    pub min_open: bool,
    pub max_open: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum JitNodeKind {
    Helper,
    Mix {
        channels: usize,
    },
    Gain {
        channels: usize,
    },
    Width,
    Timbre,
    Pressure,
    Key,
    Noise,
    VelocityOrRandom,
    OnePole {
        channels: usize,
        highpass: bool,
    },
    Svf {
        channels: usize,
        mode: u8,
        slot_cutoff: usize,
        slot_ratio: usize,
        slot_q: usize,
    },
    Pan,
    Balance,
}

#[derive(Clone, Debug)]
pub struct JitNodeSpec {
    pub kind: JitNodeKind,
    pub has_sample_modulations: bool,
    pub reads_input_later: bool,
    pub incoming: Vec<JitSource>,
    pub mod_sources: Vec<JitSource>,
    pub base_params: Vec<f64>,
    pub sample_controls: Vec<JitControlBinding>,
    pub sample_mods: Vec<JitModSpec>,
    pub bounds: Vec<JitBoundSpec>,
    /// Offset in `u64` words into `inline_state_ptr` for this node's packed state.
    pub inline_state_offset: usize,
    /// Function pointer: `extern "C" fn(compiled_node, runtime_node, frame, rate, pitch_hz, timbre, pressure, in_l, in_r, out_l, out_r, error_out) -> u8`
    pub eval_fn: usize,
    /// Function pointer for `!settled && !has_sample_modulations` static param setup at frame 0:
    /// `extern "C" fn(compiled_node, runtime_node, controls, controls_len, error_out) -> u8`
    pub setup_static_fn: usize,
}

#[derive(Clone, Debug)]
pub struct JitGraphSpec {
    pub order: Vec<usize>,
    pub nodes: Vec<JitNodeSpec>,
    pub output_node: usize,
    pub inline_state_words: usize,
}

/// Nonfinite error callback invoked by JIT-emitted checks.
pub type JitNonfiniteFn =
    unsafe extern "C" fn(compiled_node: *const c_void, error_out: *mut c_void);

/// Indexed error callback (for modulation/bound index).
pub type JitIndexedErrorFn = unsafe extern "C" fn(
    compiled_node: *const c_void,
    index: usize,
    value: f64,
    error_out: *mut c_void,
);

/// SVF cutoff/Q error callback.
pub type JitSvfErrorFn = unsafe extern "C" fn(
    compiled_node: *const c_void,
    value: f64,
    rate: f64,
    error_out: *mut c_void,
);

#[derive(Clone, Copy)]
pub struct JitErrorCallbacks {
    pub nonfinite_incoming: JitNonfiniteFn,
    pub nonfinite_output: JitNonfiniteFn,
    pub mod_product_nonfinite: JitIndexedErrorFn,
    pub mod_sum_nonfinite: JitIndexedErrorFn,
    pub param_out_of_range: JitIndexedErrorFn,
    pub onepole_invalid: JitNonfiniteFn,
    pub svf_cutoff_invalid: JitSvfErrorFn,
    pub svf_q_invalid: JitSvfErrorFn,
}

unsafe extern "C" fn jit_math_exp(x: f64) -> f64 {
    x.exp()
}
unsafe extern "C" fn jit_math_tan(x: f64) -> f64 {
    x.tan()
}
unsafe extern "C" fn jit_math_sincos(x: f64, out_cos: *mut f64, out_sin: *mut f64) {
    unsafe {
        *out_cos = x.cos();
        *out_sin = x.sin();
    }
}
unsafe extern "C" fn jit_math_cos(x: f64) -> f64 {
    x.cos()
}
unsafe extern "C" fn jit_math_log2(x: f64) -> f64 {
    x.log2()
}

struct JitModuleOwner {
    module: Option<JITModule>,
}

// Safety: After `finalize_definitions()`, the JIT code memory is immutable and
// read-only executable; `JITModule` is only dropped once all `Arc` references end.
unsafe impl Send for JitModuleOwner {}
unsafe impl Sync for JitModuleOwner {}

impl Drop for JitModuleOwner {
    fn drop(&mut self) {
        if let Some(module) = self.module.take() {
            unsafe {
                module.free_memory();
            }
        }
    }
}

pub struct JitCompiledGraph {
    _owner: Arc<JitModuleOwner>,
    func: JitRenderBlockFn,
    pub inline_state_words: usize,
}

impl std::fmt::Debug for JitCompiledGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JitCompiledGraph")
            .field("func", &(self.func as usize))
            .finish()
    }
}

// Safety: `func` points into `_owner`'s immutable finalized JIT code.
unsafe impl Send for JitCompiledGraph {}
unsafe impl Sync for JitCompiledGraph {}

impl JitCompiledGraph {
    /// Execute the JIT-compiled block kernel.
    ///
    /// # Safety
    /// All pointers inside `ctx` must remain valid for `ctx.frames` frames.
    pub unsafe fn call(&self, ctx: *mut JitBlockContext) -> usize {
        unsafe { (self.func)(ctx) }
    }
}

/// Compile a `JitGraphSpec` into native machine code using Cranelift.
/// Returns `None` if the host/W^X environment rejects JIT allocation or
/// codegen fails, allowing transparent fallback to the Rust interpreter.
pub fn compile_graph_jit(
    spec: &JitGraphSpec,
    errors: JitErrorCallbacks,
) -> Option<Arc<JitCompiledGraph>> {
    let mut flag_builder = settings::builder();
    let _ = flag_builder.set("opt_level", "speed");
    // Preserve strict IEEE-754 rounding without FMA contraction.
    let _ = flag_builder.set("enable_fma", "false");
    let _ = flag_builder.set("is_pic", "false");

    let isa_builder = cranelift_native::builder().ok()?;
    let isa = isa_builder
        .finish(settings::Flags::new(flag_builder))
        .ok()?;

    let builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
    let mut module = JITModule::new(builder);
    let ptr_type = module.target_config().pointer_type();

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(ptr_type)); // *mut JitBlockContext
    sig.returns.push(AbiParam::new(ptr_type)); // usize completed frames

    let func_id = module
        .declare_function("render_block_jit", Linkage::Local, &sig)
        .ok()?;

    let mut ctx = module.make_context();
    ctx.func.signature = sig;
    ctx.func.name = UserFuncName::user(0, func_id.as_u32());

    // Helper signatures
    let mut eval_sig = module.make_signature();
    for _ in 0..2 {
        eval_sig.params.push(AbiParam::new(ptr_type)); // compiled_node, runtime_node
    }
    eval_sig.params.push(AbiParam::new(I64)); // frame
    for _ in 0..6 {
        eval_sig.params.push(AbiParam::new(F64)); // rate, pitch, timbre, pressure, in_l, in_r
    }
    eval_sig.params.push(AbiParam::new(ptr_type)); // out_l_ptr
    eval_sig.params.push(AbiParam::new(ptr_type)); // out_r_ptr
    eval_sig.params.push(AbiParam::new(ptr_type)); // error_out
    eval_sig.returns.push(AbiParam::new(I8));

    let mut setup_sig = module.make_signature();
    for _ in 0..3 {
        setup_sig.params.push(AbiParam::new(ptr_type)); // compiled_node, runtime_node, controls
    }
    setup_sig.params.push(AbiParam::new(ptr_type)); // controls_len
    setup_sig.params.push(AbiParam::new(ptr_type)); // error_out
    setup_sig.returns.push(AbiParam::new(I8));

    let mut update_mod_sig = module.make_signature();
    for _ in 0..3 {
        update_mod_sig.params.push(AbiParam::new(ptr_type)); // compiled_node, runtime_node, controls
    }
    update_mod_sig.params.push(AbiParam::new(ptr_type)); // controls_len
    update_mod_sig.params.push(AbiParam::new(ptr_type)); // mod_values
    update_mod_sig.params.push(AbiParam::new(ptr_type)); // error_out
    update_mod_sig.returns.push(AbiParam::new(I8));

    let mut nonfinite_sig = module.make_signature();
    nonfinite_sig.params.push(AbiParam::new(ptr_type));
    nonfinite_sig.params.push(AbiParam::new(ptr_type));

    let mut indexed_err_sig = module.make_signature();
    indexed_err_sig.params.push(AbiParam::new(ptr_type));
    indexed_err_sig.params.push(AbiParam::new(ptr_type));
    indexed_err_sig.params.push(AbiParam::new(F64));
    indexed_err_sig.params.push(AbiParam::new(ptr_type));

    let mut svf_err_sig = module.make_signature();
    svf_err_sig.params.push(AbiParam::new(ptr_type));
    svf_err_sig.params.push(AbiParam::new(F64));
    svf_err_sig.params.push(AbiParam::new(F64));
    svf_err_sig.params.push(AbiParam::new(ptr_type));

    let mut math_f64_sig = module.make_signature();
    math_f64_sig.params.push(AbiParam::new(F64));
    math_f64_sig.returns.push(AbiParam::new(F64));

    let mut math_sincos_sig = module.make_signature();
    math_sincos_sig.params.push(AbiParam::new(F64));
    math_sincos_sig.params.push(AbiParam::new(ptr_type));
    math_sincos_sig.params.push(AbiParam::new(ptr_type));

    let mut fb_ctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);
        let eval_sig_ref = fb.import_signature(eval_sig);
        let setup_sig_ref = fb.import_signature(setup_sig);
        let _update_mod_sig_ref = fb.import_signature(update_mod_sig);
        let nonfinite_sig_ref = fb.import_signature(nonfinite_sig);
        let indexed_err_sig_ref = fb.import_signature(indexed_err_sig);
        let svf_err_sig_ref = fb.import_signature(svf_err_sig);
        let math_f64_sig_ref = fb.import_signature(math_f64_sig);
        let math_sincos_sig_ref = fb.import_signature(math_sincos_sig);

        let entry = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        fb.switch_to_block(entry);
        fb.seal_block(entry);

        let ctx_ptr = fb.block_params(entry)[0];
        let mem = MemFlagsData::new();

        // JitBlockContext field offsets (all pointer/u64/f64 sized on 64-bit)
        let ptr_bytes = ptr_type.bytes() as i32;
        let off_runtime_nodes = 0;
        let off_runtime_params = ptr_bytes;
        let off_inline_state = ptr_bytes * 2;
        let off_compiled_nodes = ptr_bytes * 3;
        let off_controls = ptr_bytes * 4;
        let off_controls_len = ptr_bytes * 5;
        let off_expressions = ptr_bytes * 6;
        let off_input = ptr_bytes * 7;
        let off_output = ptr_bytes * 8;
        let off_error_out = ptr_bytes * 9;
        let off_start_frame = ptr_bytes * 10;
        let off_frames = off_start_frame + 8;
        let off_rate = off_frames + ptr_bytes;
        let off_settled = off_rate + 8;

        let runtime_nodes_ptr = fb.ins().load(ptr_type, mem, ctx_ptr, off_runtime_nodes);
        let runtime_params_ptr = fb.ins().load(ptr_type, mem, ctx_ptr, off_runtime_params);
        let inline_state_ptr = fb.ins().load(ptr_type, mem, ctx_ptr, off_inline_state);
        let compiled_nodes_ptr = fb.ins().load(ptr_type, mem, ctx_ptr, off_compiled_nodes);
        let controls_ptr = fb.ins().load(ptr_type, mem, ctx_ptr, off_controls);
        let controls_len = fb.ins().load(ptr_type, mem, ctx_ptr, off_controls_len);
        let expressions_ptr = fb.ins().load(ptr_type, mem, ctx_ptr, off_expressions);
        let input_ptr = fb.ins().load(ptr_type, mem, ctx_ptr, off_input);
        let output_ptr = fb.ins().load(ptr_type, mem, ctx_ptr, off_output);
        let error_out = fb.ins().load(ptr_type, mem, ctx_ptr, off_error_out);
        let start_frame = fb.ins().load(I64, mem, ctx_ptr, off_start_frame);
        let frames = fb.ins().load(ptr_type, mem, ctx_ptr, off_frames);
        let rate = fb.ins().load(F64, mem, ctx_ptr, off_rate);
        let settled = fb.ins().load(I8, mem, ctx_ptr, off_settled);

        let zero_usize = fb.ins().iconst(ptr_type, 0);
        let one_usize = fb.ins().iconst(ptr_type, 1);

        // If frames == 0, return 0 immediately.
        let non_empty_blk = fb.create_block();
        let ret_zero_blk = fb.create_block();
        let has_frames = fb.ins().icmp(
            cranelift_codegen::ir::condcodes::IntCC::NotEqual,
            frames,
            zero_usize,
        );
        fb.ins()
            .brif(has_frames, non_empty_blk, &[], ret_zero_blk, &[]);

        fb.switch_to_block(ret_zero_blk);
        fb.seal_block(ret_zero_blk);
        fb.ins().return_(&[zero_usize]);

        fb.switch_to_block(non_empty_blk);
        fb.seal_block(non_empty_blk);

        // Stack slots for node output (out_l, out_r) and modulation inputs
        let out_l_slot = fb.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
            cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
            8,
            3,
        ));
        let out_r_slot = fb.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
            cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
            8,
            3,
        ));
        let out_l_addr = fb.ins().stack_addr(ptr_type, out_l_slot, 0);
        let out_r_addr = fb.ins().stack_addr(ptr_type, out_r_slot, 0);

        let max_mods = spec
            .nodes
            .iter()
            .map(|n| n.mod_sources.len())
            .max()
            .unwrap_or(0)
            .max(1);
        let mod_slot = fb.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
            cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
            (max_mods * 8) as u32,
            3,
        ));
        let _mod_addr = fb.ins().stack_addr(ptr_type, mod_slot, 0);

        // Helper closure to emit one full frame evaluation at `offset`.
        // `peel_static_setup`: if true, emit `if settled == 0 { setup_static_fn(...) }`
        // in topological order for `!has_sample_modulations` nodes.
        let emit_frame = |fb: &mut FunctionBuilder<'_>,
                          offset: cranelift_codegen::ir::Value,
                          peel_static_setup: bool| {
            let mut node_l: Vec<cranelift_codegen::ir::Value> =
                vec![fb.ins().f64const(0.0); spec.nodes.len()];
            let mut node_r: Vec<cranelift_codegen::ir::Value> =
                vec![fb.ins().f64const(0.0); spec.nodes.len()];

            // Load audio_input[offset] if input_ptr != 0
            let zero_f64 = fb.ins().f64const(0.0);
            let input_nonnull = fb.ins().icmp(
                cranelift_codegen::ir::condcodes::IntCC::NotEqual,
                input_ptr,
                zero_usize,
            );
            let load_in_blk = fb.create_block();
            let zero_in_blk = fb.create_block();
            let after_in_blk = fb.create_block();
            fb.append_block_param(after_in_blk, F64);
            fb.append_block_param(after_in_blk, F64);
            fb.ins()
                .brif(input_nonnull, load_in_blk, &[], zero_in_blk, &[]);

            fb.switch_to_block(load_in_blk);
            fb.seal_block(load_in_blk);
            let sixteen = fb.ins().iconst(ptr_type, 16);
            let in_off = fb.ins().imul(offset, sixteen);
            let in_elem = fb.ins().iadd(input_ptr, in_off);
            let loaded_in_l = fb.ins().load(F64, mem, in_elem, 0);
            let loaded_in_r = fb.ins().load(F64, mem, in_elem, 8);
            fb.ins()
                .jump(after_in_blk, &[loaded_in_l.into(), loaded_in_r.into()]);

            fb.switch_to_block(zero_in_blk);
            fb.seal_block(zero_in_blk);
            fb.ins()
                .jump(after_in_blk, &[zero_f64.into(), zero_f64.into()]);

            fb.switch_to_block(after_in_blk);
            fb.seal_block(after_in_blk);
            let ext_in_l = fb.block_params(after_in_blk)[0];
            let ext_in_r = fb.block_params(after_in_blk)[1];

            // Load expressions[offset] (24 bytes per JitExpressionInput)
            let twenty_four = fb.ins().iconst(ptr_type, 24);
            let expr_off = fb.ins().imul(offset, twenty_four);
            let expr_elem = fb.ins().iadd(expressions_ptr, expr_off);
            let pitch_hz = fb.ins().load(F64, mem, expr_elem, 0);
            let timbre = fb.ins().load(F64, mem, expr_elem, 8);
            let pressure = fb.ins().load(F64, mem, expr_elem, 16);

            let offset_i64 = if ptr_type == I64 {
                offset
            } else {
                fb.ins().uextend(I64, offset)
            };
            let frame = fb.ins().iadd(start_frame, offset_i64);

            for &node_idx in &spec.order {
                let node = &spec.nodes[node_idx];
                let byte_off = (node_idx * ptr_bytes as usize) as i32;
                let compiled_node = fb.ins().load(ptr_type, mem, compiled_nodes_ptr, byte_off);
                let runtime_node = fb.ins().load(ptr_type, mem, runtime_nodes_ptr, byte_off);

                // Frame-0 peeled static parameter setup in exact topological order.
                if peel_static_setup && !node.has_sample_modulations {
                    let do_setup_blk = fb.create_block();
                    let after_setup_blk = fb.create_block();
                    let zero_i8 = fb.ins().iconst(I8, 0);
                    let is_unsettled = fb.ins().icmp(
                        cranelift_codegen::ir::condcodes::IntCC::Equal,
                        settled,
                        zero_i8,
                    );
                    fb.ins()
                        .brif(is_unsettled, do_setup_blk, &[], after_setup_blk, &[]);

                    fb.switch_to_block(do_setup_blk);
                    fb.seal_block(do_setup_blk);
                    let setup_callee = fb.ins().iconst(ptr_type, node.setup_static_fn as i64);
                    let setup_call = fb.ins().call_indirect(
                        setup_sig_ref,
                        setup_callee,
                        &[
                            compiled_node,
                            runtime_node,
                            controls_ptr,
                            controls_len,
                            error_out,
                        ],
                    );
                    let setup_err = fb.inst_results(setup_call)[0];
                    let setup_ok = fb.ins().icmp(
                        cranelift_codegen::ir::condcodes::IntCC::Equal,
                        setup_err,
                        zero_i8,
                    );
                    let err_ret_blk = fb.create_block();
                    fb.ins()
                        .brif(setup_ok, after_setup_blk, &[], err_ret_blk, &[]);

                    fb.switch_to_block(err_ret_blk);
                    fb.seal_block(err_ret_blk);
                    fb.ins().return_(&[offset]);

                    fb.switch_to_block(after_setup_blk);
                    fb.seal_block(after_setup_blk);
                }

                // Inline per-sample modulation + bounds validation in SSA registers
                let mut mod_params: Option<Vec<cranelift_codegen::ir::Value>> = None;
                if node.has_sample_modulations {
                    let mut pvals: Vec<cranelift_codegen::ir::Value> = node
                        .base_params
                        .iter()
                        .map(|&v| fb.ins().f64const(v))
                        .collect();
                    for binding in &node.sample_controls {
                        let c_off = (binding.control * 8) as i32;
                        pvals[binding.parameter] = fb.ins().load(F64, mem, controls_ptr, c_off);
                    }
                    let inf = fb.ins().f64const(f64::INFINITY);
                    for m in &node.sample_mods {
                        let src_val = match m.source {
                            JitSource::Input => ext_in_l,
                            JitSource::Node(idx) => node_l[idx],
                        };
                        let depth_v = fb.ins().f64const(m.depth);
                        let prod = fb.ins().fmul(src_val, depth_v);
                        let prod_ord = fb.ins().fcmp(FloatCC::Ordered, prod, prod);
                        let prod_abs = fb.ins().fabs(prod);
                        let prod_ninf = fb.ins().fcmp(FloatCC::LessThan, prod_abs, inf);
                        let prod_ok = fb.ins().band(prod_ord, prod_ninf);
                        let ok_p = fb.create_block();
                        let fail_p = fb.create_block();
                        fb.ins().brif(prod_ok, ok_p, &[], fail_p, &[]);
                        fb.switch_to_block(fail_p);
                        fb.seal_block(fail_p);
                        let err_c = fb
                            .ins()
                            .iconst(ptr_type, errors.mod_product_nonfinite as usize as i64);
                        let m_idx_v = fb.ins().iconst(ptr_type, m.mod_index as i64);
                        fb.ins().call_indirect(
                            indexed_err_sig_ref,
                            err_c,
                            &[compiled_node, m_idx_v, zero_f64, error_out],
                        );
                        fb.ins().return_(&[offset]);
                        fb.switch_to_block(ok_p);
                        fb.seal_block(ok_p);

                        let sum = fb.ins().fadd(pvals[m.parameter], prod);
                        let sum_ord = fb.ins().fcmp(FloatCC::Ordered, sum, sum);
                        let sum_abs = fb.ins().fabs(sum);
                        let sum_ninf = fb.ins().fcmp(FloatCC::LessThan, sum_abs, inf);
                        let sum_ok = fb.ins().band(sum_ord, sum_ninf);
                        let ok_s = fb.create_block();
                        let fail_s = fb.create_block();
                        fb.ins().brif(sum_ok, ok_s, &[], fail_s, &[]);
                        fb.switch_to_block(fail_s);
                        fb.seal_block(fail_s);
                        let err_c = fb
                            .ins()
                            .iconst(ptr_type, errors.mod_sum_nonfinite as usize as i64);
                        let m_idx_v = fb.ins().iconst(ptr_type, m.mod_index as i64);
                        fb.ins().call_indirect(
                            indexed_err_sig_ref,
                            err_c,
                            &[compiled_node, m_idx_v, zero_f64, error_out],
                        );
                        fb.ins().return_(&[offset]);
                        fb.switch_to_block(ok_s);
                        fb.seal_block(ok_s);

                        pvals[m.parameter] = sum;
                    }
                    // Validate parameter bounds
                    for b in &node.bounds {
                        let val = pvals[b.parameter];
                        let ord = fb.ins().fcmp(FloatCC::Ordered, val, val);
                        let abs_v = fb.ins().fabs(val);
                        let ninf = fb.ins().fcmp(FloatCC::LessThan, abs_v, inf);
                        let is_fin = fb.ins().band(ord, ninf);
                        let min_v = fb.ins().f64const(b.min);
                        let max_v = fb.ins().f64const(b.max);
                        let min_ok = fb.ins().fcmp(
                            if b.min_open {
                                FloatCC::GreaterThan
                            } else {
                                FloatCC::GreaterThanOrEqual
                            },
                            val,
                            min_v,
                        );
                        let max_ok = fb.ins().fcmp(
                            if b.max_open {
                                FloatCC::LessThan
                            } else {
                                FloatCC::LessThanOrEqual
                            },
                            val,
                            max_v,
                        );
                        let rng_ok = fb.ins().band(min_ok, max_ok);
                        let all_ok = fb.ins().band(is_fin, rng_ok);
                        let ok_b = fb.create_block();
                        let fail_b = fb.create_block();
                        fb.ins().brif(all_ok, ok_b, &[], fail_b, &[]);
                        fb.switch_to_block(fail_b);
                        fb.seal_block(fail_b);
                        let err_c = fb
                            .ins()
                            .iconst(ptr_type, errors.param_out_of_range as usize as i64);
                        let p_idx_v = fb.ins().iconst(ptr_type, b.parameter as i64);
                        fb.ins().call_indirect(
                            indexed_err_sig_ref,
                            err_c,
                            &[compiled_node, p_idx_v, val, error_out],
                        );
                        fb.ins().return_(&[offset]);
                        fb.switch_to_block(ok_b);
                        fb.seal_block(ok_b);
                    }
                    if matches!(node.kind, JitNodeKind::Helper) {
                        let params_ptr = fb.ins().load(ptr_type, mem, runtime_params_ptr, byte_off);
                        for (pi, &pval) in pvals.iter().enumerate() {
                            fb.ins().store(mem, pval, params_ptr, (pi * 8) as i32);
                        }
                    }
                    mod_params = Some(pvals);
                }

                // Inline `sum_incoming` in SSA registers with exact `is_finite` check
                let mut sum_l = zero_f64;
                let mut sum_r = zero_f64;
                if !node.reads_input_later {
                    for src in &node.incoming {
                        let (sl, sr) = match src {
                            JitSource::Input => (ext_in_l, ext_in_r),
                            JitSource::Node(idx) => (node_l[*idx], node_r[*idx]),
                        };
                        sum_l = fb.ins().fadd(sum_l, sl);
                        let l_finite = fb.ins().fcmp(FloatCC::Ordered, sum_l, sum_l);
                        let l_abs = fb.ins().fabs(sum_l);
                        let inf = fb.ins().f64const(f64::INFINITY);
                        let l_not_inf = fb.ins().fcmp(FloatCC::LessThan, l_abs, inf);
                        let l_ok = fb.ins().band(l_finite, l_not_inf);
                        let ok_l_blk = fb.create_block();
                        let fail_l_blk = fb.create_block();
                        fb.ins().brif(l_ok, ok_l_blk, &[], fail_l_blk, &[]);

                        fb.switch_to_block(fail_l_blk);
                        fb.seal_block(fail_l_blk);
                        let nf_callee = fb
                            .ins()
                            .iconst(ptr_type, errors.nonfinite_incoming as usize as i64);
                        fb.ins().call_indirect(
                            nonfinite_sig_ref,
                            nf_callee,
                            &[compiled_node, error_out],
                        );
                        fb.ins().return_(&[offset]);

                        fb.switch_to_block(ok_l_blk);
                        fb.seal_block(ok_l_blk);

                        sum_r = fb.ins().fadd(sum_r, sr);
                        let r_finite = fb.ins().fcmp(FloatCC::Ordered, sum_r, sum_r);
                        let r_abs = fb.ins().fabs(sum_r);
                        let r_not_inf = fb.ins().fcmp(FloatCC::LessThan, r_abs, inf);
                        let r_ok = fb.ins().band(r_finite, r_not_inf);
                        let ok_r_blk = fb.create_block();
                        let fail_r_blk = fb.create_block();
                        fb.ins().brif(r_ok, ok_r_blk, &[], fail_r_blk, &[]);

                        fb.switch_to_block(fail_r_blk);
                        fb.seal_block(fail_r_blk);
                        let nf_callee = fb
                            .ins()
                            .iconst(ptr_type, errors.nonfinite_incoming as usize as i64);
                        fb.ins().call_indirect(
                            nonfinite_sig_ref,
                            nf_callee,
                            &[compiled_node, error_out],
                        );
                        fb.ins().return_(&[offset]);

                        fb.switch_to_block(ok_r_blk);
                        fb.seal_block(ok_r_blk);
                    }
                }

                let emit_finite_check =
                    |fb: &mut FunctionBuilder<'_>, val: cranelift_codegen::ir::Value| {
                        let ord = fb.ins().fcmp(FloatCC::Ordered, val, val);
                        let abs_v = fb.ins().fabs(val);
                        let inf = fb.ins().f64const(f64::INFINITY);
                        let not_inf = fb.ins().fcmp(FloatCC::LessThan, abs_v, inf);
                        let ok = fb.ins().band(ord, not_inf);
                        let ok_b = fb.create_block();
                        let fail_b = fb.create_block();
                        fb.ins().brif(ok, ok_b, &[], fail_b, &[]);

                        fb.switch_to_block(fail_b);
                        fb.seal_block(fail_b);
                        let nf_callee = fb
                            .ins()
                            .iconst(ptr_type, errors.nonfinite_output as usize as i64);
                        fb.ins().call_indirect(
                            nonfinite_sig_ref,
                            nf_callee,
                            &[compiled_node, error_out],
                        );
                        fb.ins().return_(&[offset]);

                        fb.switch_to_block(ok_b);
                        fb.seal_block(ok_b);
                    };

                let load_param = |fb: &mut FunctionBuilder<'_>,
                                  slot_idx: usize|
                 -> cranelift_codegen::ir::Value {
                    if let Some(pvals) = &mod_params {
                        pvals[slot_idx]
                    } else {
                        let params_ptr = fb.ins().load(ptr_type, mem, runtime_params_ptr, byte_off);
                        fb.ins().load(F64, mem, params_ptr, (slot_idx * 8) as i32)
                    }
                };
                let state_byte_off = (node.inline_state_offset * 8) as i32;

                match node.kind {
                    JitNodeKind::Mix { channels } => {
                        let out_l = if channels >= 1 { sum_l } else { zero_f64 };
                        let out_r = if channels >= 2 { sum_r } else { zero_f64 };
                        if channels >= 1 {
                            emit_finite_check(fb, out_l);
                        }
                        if channels >= 2 {
                            emit_finite_check(fb, out_r);
                        }
                        node_l[node_idx] = out_l;
                        node_r[node_idx] = out_r;
                    }
                    JitNodeKind::Timbre => {
                        emit_finite_check(fb, timbre);
                        node_l[node_idx] = timbre;
                        node_r[node_idx] = zero_f64;
                    }
                    JitNodeKind::Pressure => {
                        emit_finite_check(fb, pressure);
                        node_l[node_idx] = pressure;
                        node_r[node_idx] = zero_f64;
                    }
                    JitNodeKind::Key => {
                        let c440 = fb.ins().f64const(440.0);
                        let ratio = fb.ins().fdiv(pitch_hz, c440);
                        let log2_callee = fb
                            .ins()
                            .iconst(ptr_type, jit_math_log2 as *const () as usize as i64);
                        let call = fb
                            .ins()
                            .call_indirect(math_f64_sig_ref, log2_callee, &[ratio]);
                        let l2 = fb.inst_results(call)[0];
                        let c075 = fb.ins().f64const(0.75);
                        let out = fb.ins().fadd(l2, c075);
                        emit_finite_check(fb, out);
                        node_l[node_idx] = out;
                        node_r[node_idx] = zero_f64;
                    }
                    JitNodeKind::Noise => {
                        let level = load_param(fb, 0); // slot::NOISE_LEVEL == 0
                        let st = fb.ins().load(I32, mem, inline_state_ptr, state_byte_off);
                        let s13 = fb.ins().ishl_imm_u(st, 13);
                        let st1 = fb.ins().bxor(st, s13);
                        let s17 = fb.ins().ushr_imm_u(st1, 17);
                        let st2 = fb.ins().bxor(st1, s17);
                        let s5 = fb.ins().ishl_imm_u(st2, 5);
                        let next = fb.ins().bxor(st2, s5);
                        fb.ins().store(mem, next, inline_state_ptr, state_byte_off);
                        let next_f64 = fb.ins().fcvt_from_uint(F64, next);
                        let scale = fb.ins().f64const(2147483648.0);
                        let div = fb.ins().fdiv(next_f64, scale);
                        let one_f64 = fb.ins().f64const(1.0);
                        let norm = fb.ins().fsub(div, one_f64);
                        let out = fb.ins().fmul(norm, level);
                        emit_finite_check(fb, out);
                        node_l[node_idx] = out;
                        node_r[node_idx] = zero_f64;
                    }
                    JitNodeKind::VelocityOrRandom => {
                        // inline_state: [0]=val(f64), [1]=has_glide(u64), [2]=from(f64), [3]=start(u64)
                        let val = fb.ins().load(F64, mem, inline_state_ptr, state_byte_off);
                        let has_glide =
                            fb.ins()
                                .load(I64, mem, inline_state_ptr, state_byte_off + 8);
                        let zero_i64 = fb.ins().iconst(I64, 0);
                        let glide_flag = fb.ins().icmp(
                            cranelift_codegen::ir::condcodes::IntCC::NotEqual,
                            has_glide,
                            zero_i64,
                        );
                        let check_end_blk = fb.create_block();
                        let calc_blk = fb.create_block();
                        let plain_blk = fb.create_block();
                        let done_blk = fb.create_block();
                        fb.append_block_param(done_blk, F64);
                        fb.ins()
                            .brif(glide_flag, check_end_blk, &[], plain_blk, &[]);

                        fb.switch_to_block(check_end_blk);
                        fb.seal_block(check_end_blk);
                        let g_start =
                            fb.ins()
                                .load(I64, mem, inline_state_ptr, state_byte_off + 24);
                        let g_dur = fb.ins().iconst(I64, 4800);
                        let g_end = fb.ins().iadd(g_start, g_dur);
                        let in_glide = fb.ins().icmp(
                            cranelift_codegen::ir::condcodes::IntCC::UnsignedLessThan,
                            frame,
                            g_end,
                        );
                        fb.ins().brif(in_glide, calc_blk, &[], plain_blk, &[]);

                        fb.switch_to_block(calc_blk);
                        fb.seal_block(calc_blk);
                        let g_from = fb
                            .ins()
                            .load(F64, mem, inline_state_ptr, state_byte_off + 16);
                        let ge_start = fb.ins().icmp(
                            cranelift_codegen::ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
                            frame,
                            g_start,
                        );
                        let raw_diff = fb.ins().isub(frame, g_start);
                        let delta_u64 = fb.ins().select(ge_start, raw_diff, zero_i64);
                        let delta_f64 = fb.ins().fcvt_from_uint(F64, delta_u64);
                        let dur_f64 = fb.ins().f64const(4800.0);
                        let progress = fb.ins().fdiv(delta_f64, dur_f64);
                        let v_diff = fb.ins().fsub(val, g_from);
                        let scaled = fb.ins().fmul(v_diff, progress);
                        let glided = fb.ins().fadd(g_from, scaled);
                        fb.ins().jump(done_blk, &[glided.into()]);

                        fb.switch_to_block(plain_blk);
                        fb.seal_block(plain_blk);
                        fb.ins().jump(done_blk, &[val.into()]);

                        fb.switch_to_block(done_blk);
                        fb.seal_block(done_blk);
                        let out = fb.block_params(done_blk)[0];
                        emit_finite_check(fb, out);
                        node_l[node_idx] = out;
                        node_r[node_idx] = zero_f64;
                    }
                    JitNodeKind::Gain { channels } => {
                        let gain_level = load_param(fb, 0); // slot::GAIN_LEVEL == 0
                        let out_l = if channels >= 1 {
                            fb.ins().fmul(sum_l, gain_level)
                        } else {
                            zero_f64
                        };
                        let out_r = if channels >= 2 {
                            fb.ins().fmul(sum_r, gain_level)
                        } else {
                            zero_f64
                        };
                        if channels >= 1 {
                            emit_finite_check(fb, out_l);
                        }
                        if channels >= 2 {
                            emit_finite_check(fb, out_r);
                        }
                        node_l[node_idx] = out_l;
                        node_r[node_idx] = out_r;
                    }
                    JitNodeKind::Width => {
                        let width = load_param(fb, 0); // slot::WIDTH == 0
                        let two = fb.ins().f64const(2.0);
                        let sum_lr = fb.ins().fadd(sum_l, sum_r);
                        let diff_lr = fb.ins().fsub(sum_l, sum_r);
                        let mid = fb.ins().fdiv(sum_lr, two);
                        let side = fb.ins().fdiv(diff_lr, two);
                        let w_side = fb.ins().fmul(width, side);
                        let out_l = fb.ins().fadd(mid, w_side);
                        let out_r = fb.ins().fsub(mid, w_side);
                        emit_finite_check(fb, out_l);
                        emit_finite_check(fb, out_r);
                        node_l[node_idx] = out_l;
                        node_r[node_idx] = out_r;
                    }
                    JitNodeKind::Pan => {
                        let pan = load_param(fb, 0); // slot::PAN == 0
                        let inf = fb.ins().f64const(f64::INFINITY);
                        let in_ord = fb.ins().fcmp(FloatCC::Ordered, sum_l, sum_l);
                        let in_abs = fb.ins().fabs(sum_l);
                        let in_ninf = fb.ins().fcmp(FloatCC::LessThan, in_abs, inf);
                        let pan_ord = fb.ins().fcmp(FloatCC::Ordered, pan, pan);
                        let pan_abs = fb.ins().fabs(pan);
                        let pan_ninf = fb.ins().fcmp(FloatCC::LessThan, pan_abs, inf);
                        let ok_in = fb.ins().band(in_ord, in_ninf);
                        let ok_pan = fb.ins().band(pan_ord, pan_ninf);
                        let ok = fb.ins().band(ok_in, ok_pan);
                        let ok_b = fb.create_block();
                        let fail_b = fb.create_block();
                        fb.ins().brif(ok, ok_b, &[], fail_b, &[]);
                        fb.switch_to_block(fail_b);
                        fb.seal_block(fail_b);
                        let err_c = fb
                            .ins()
                            .iconst(ptr_type, errors.nonfinite_output as usize as i64);
                        fb.ins().call_indirect(
                            nonfinite_sig_ref,
                            err_c,
                            &[compiled_node, error_out],
                        );
                        fb.ins().return_(&[offset]);
                        fb.switch_to_block(ok_b);
                        fb.seal_block(ok_b);

                        let neg_one = fb.ins().f64const(-1.0);
                        let pos_one = fb.ins().f64const(1.0);
                        let cl1 = fb.ins().fmax(pan, neg_one);
                        let clamped = fb.ins().fmin(cl1, pos_one);
                        let plus_one = fb.ins().fadd(clamped, pos_one);
                        let pi = fb.ins().f64const(std::f64::consts::PI);
                        let four = fb.ins().f64const(4.0);
                        let num = fb.ins().fmul(plus_one, pi);
                        let theta = fb.ins().fdiv(num, four);

                        let sincos_callee = fb
                            .ins()
                            .iconst(ptr_type, jit_math_sincos as *const () as usize as i64);
                        fb.ins().call_indirect(
                            math_sincos_sig_ref,
                            sincos_callee,
                            &[theta, out_l_addr, out_r_addr],
                        );
                        let c_val = fb.ins().load(F64, mem, out_l_addr, 0);
                        let s_val = fb.ins().load(F64, mem, out_r_addr, 0);
                        let out_l = fb.ins().fmul(sum_l, c_val);
                        let out_r = fb.ins().fmul(sum_l, s_val);
                        emit_finite_check(fb, out_l);
                        emit_finite_check(fb, out_r);
                        node_l[node_idx] = out_l;
                        node_r[node_idx] = out_r;
                    }
                    JitNodeKind::Balance => {
                        let bal = load_param(fb, 0); // slot::BALANCE == 0
                        let one_f64 = fb.ins().f64const(1.0);
                        let frac_pi_2 = fb.ins().f64const(std::f64::consts::FRAC_PI_2);
                        let cos_callee = fb
                            .ins()
                            .iconst(ptr_type, jit_math_cos as *const () as usize as i64);

                        let emit_lowered =
                            |fb: &mut FunctionBuilder<'_>,
                             amt: cranelift_codegen::ir::Value|
                             -> cranelift_codegen::ir::Value {
                                let ge_one =
                                    fb.ins().fcmp(FloatCC::GreaterThanOrEqual, amt, one_f64);
                                let z_b = fb.create_block();
                                let c_b = fb.create_block();
                                let m_b = fb.create_block();
                                fb.append_block_param(m_b, F64);
                                fb.ins().brif(ge_one, z_b, &[], c_b, &[]);
                                fb.switch_to_block(z_b);
                                fb.seal_block(z_b);
                                fb.ins().jump(m_b, &[zero_f64.into()]);
                                fb.switch_to_block(c_b);
                                fb.seal_block(c_b);
                                let clamped = fb.ins().fmax(amt, zero_f64);
                                let arg = fb.ins().fmul(frac_pi_2, clamped);
                                let c =
                                    fb.ins().call_indirect(math_f64_sig_ref, cos_callee, &[arg]);
                                let r = fb.inst_results(c)[0];
                                fb.ins().jump(m_b, &[r.into()]);
                                fb.switch_to_block(m_b);
                                fb.seal_block(m_b);
                                fb.block_params(m_b)[0]
                            };
                        let gain_l = emit_lowered(fb, bal);
                        let neg_bal = fb.ins().fneg(bal);
                        let gain_r = emit_lowered(fb, neg_bal);
                        let out_l = fb.ins().fmul(sum_l, gain_l);
                        let out_r = fb.ins().fmul(sum_r, gain_r);
                        emit_finite_check(fb, out_l);
                        emit_finite_check(fb, out_r);
                        node_l[node_idx] = out_l;
                        node_r[node_idx] = out_r;
                    }
                    JitNodeKind::OnePole { channels, highpass } => {
                        // inline_state: [0]=prev0, [1]=prev1, [2]=cached_cutoff_bits, [3]=cached_a
                        let cutoff = load_param(fb, 0); // slot::ONEPOLE_CUTOFF == 0
                        let cutoff_bits = fb.ins().bitcast(I64, mem, cutoff);
                        let cached_bits =
                            fb.ins()
                                .load(I64, mem, inline_state_ptr, state_byte_off + 16);
                        let hit = fb.ins().icmp(
                            cranelift_codegen::ir::condcodes::IntCC::Equal,
                            cutoff_bits,
                            cached_bits,
                        );
                        let miss_b = fb.create_block();
                        let hit_b = fb.create_block();
                        let merge_b = fb.create_block();
                        fb.append_block_param(merge_b, F64);
                        fb.ins().brif(hit, hit_b, &[], miss_b, &[]);

                        fb.switch_to_block(hit_b);
                        fb.seal_block(hit_b);
                        let cached_a =
                            fb.ins()
                                .load(F64, mem, inline_state_ptr, state_byte_off + 24);
                        fb.ins().jump(merge_b, &[cached_a.into()]);

                        fb.switch_to_block(miss_b);
                        fb.seal_block(miss_b);
                        let inf = fb.ins().f64const(f64::INFINITY);
                        let c_ord = fb.ins().fcmp(FloatCC::Ordered, cutoff, cutoff);
                        let c_abs = fb.ins().fabs(cutoff);
                        let c_ninf = fb.ins().fcmp(FloatCC::LessThan, c_abs, inf);
                        let c_pos = fb.ins().fcmp(FloatCC::GreaterThan, cutoff, zero_f64);
                        let two = fb.ins().f64const(2.0);
                        let nyq = fb.ins().fdiv(rate, two);
                        let c_lt_nyq = fb.ins().fcmp(FloatCC::LessThan, cutoff, nyq);
                        let ok1 = fb.ins().band(c_ord, c_ninf);
                        let ok2 = fb.ins().band(c_pos, c_lt_nyq);
                        let c_ok = fb.ins().band(ok1, ok2);
                        let c_ok_b = fb.create_block();
                        let c_fail_b = fb.create_block();
                        fb.ins().brif(c_ok, c_ok_b, &[], c_fail_b, &[]);
                        fb.switch_to_block(c_fail_b);
                        fb.seal_block(c_fail_b);
                        let err_c = fb
                            .ins()
                            .iconst(ptr_type, errors.onepole_invalid as usize as i64);
                        fb.ins().call_indirect(
                            nonfinite_sig_ref,
                            err_c,
                            &[compiled_node, error_out],
                        );
                        fb.ins().return_(&[offset]);

                        fb.switch_to_block(c_ok_b);
                        fb.seal_block(c_ok_b);
                        // (-2.0 * PI * cutoff / rate).exp()
                        let neg_2pi = fb.ins().f64const(-2.0 * std::f64::consts::PI);
                        let num = fb.ins().fmul(neg_2pi, cutoff);
                        let arg = fb.ins().fdiv(num, rate);
                        let exp_callee = fb
                            .ins()
                            .iconst(ptr_type, jit_math_exp as *const () as usize as i64);
                        let exp_c = fb.ins().call_indirect(math_f64_sig_ref, exp_callee, &[arg]);
                        let new_a = fb.inst_results(exp_c)[0];
                        fb.ins()
                            .store(mem, cutoff_bits, inline_state_ptr, state_byte_off + 16);
                        fb.ins()
                            .store(mem, new_a, inline_state_ptr, state_byte_off + 24);
                        fb.ins().jump(merge_b, &[new_a.into()]);

                        fb.switch_to_block(merge_b);
                        fb.seal_block(merge_b);
                        let a_val = fb.block_params(merge_b)[0];
                        let one_f64 = fb.ins().f64const(1.0);
                        let one_minus_a = fb.ins().fsub(one_f64, a_val);

                        let mut out_lr = [zero_f64, zero_f64];
                        let in_lr = [sum_l, sum_r];
                        for ch in 0..channels {
                            let inp = in_lr[ch];
                            let prev = fb.ins().load(
                                F64,
                                mem,
                                inline_state_ptr,
                                state_byte_off + (ch as i32) * 8,
                            );
                            let t1 = fb.ins().fmul(one_minus_a, inp);
                            let t2 = fb.ins().fmul(a_val, prev);
                            let lp = fb.ins().fadd(t1, t2);
                            emit_finite_check(fb, lp);
                            fb.ins().store(
                                mem,
                                lp,
                                inline_state_ptr,
                                state_byte_off + (ch as i32) * 8,
                            );
                            out_lr[ch] = if highpass { fb.ins().fsub(inp, lp) } else { lp };
                            if highpass {
                                emit_finite_check(fb, out_lr[ch]);
                            }
                        }
                        node_l[node_idx] = out_lr[0];
                        node_r[node_idx] = out_lr[1];
                    }
                    JitNodeKind::Svf {
                        channels,
                        mode,
                        slot_cutoff,
                        slot_ratio,
                        slot_q,
                    } => {
                        // inline_state: [0..4]=s00,s01,s10,s11, [4]=cached_cutoff_bits, [5]=cached_q_bits, [6]=k, [7]=a1, [8]=a2, [9]=a3
                        let base_cutoff = load_param(fb, slot_cutoff);
                        let q = load_param(fb, slot_q);
                        let ratio = load_param(fb, slot_ratio);
                        let p_rat = fb.ins().fmul(pitch_hz, ratio);
                        let cutoff = fb.ins().fadd(p_rat, base_cutoff);

                        let c_bits = fb.ins().bitcast(I64, mem, cutoff);
                        let q_bits = fb.ins().bitcast(I64, mem, q);
                        let cc_bits =
                            fb.ins()
                                .load(I64, mem, inline_state_ptr, state_byte_off + 32);
                        let cq_bits =
                            fb.ins()
                                .load(I64, mem, inline_state_ptr, state_byte_off + 40);
                        let hit_c = fb.ins().icmp(
                            cranelift_codegen::ir::condcodes::IntCC::Equal,
                            c_bits,
                            cc_bits,
                        );
                        let hit_q = fb.ins().icmp(
                            cranelift_codegen::ir::condcodes::IntCC::Equal,
                            q_bits,
                            cq_bits,
                        );
                        let hit = fb.ins().band(hit_c, hit_q);

                        let hit_b = fb.create_block();
                        let miss_b = fb.create_block();
                        let merge_b = fb.create_block();
                        for _ in 0..4 {
                            fb.append_block_param(merge_b, F64);
                        }
                        fb.ins().brif(hit, hit_b, &[], miss_b, &[]);

                        fb.switch_to_block(hit_b);
                        fb.seal_block(hit_b);
                        let hk = fb
                            .ins()
                            .load(F64, mem, inline_state_ptr, state_byte_off + 48);
                        let ha1 = fb
                            .ins()
                            .load(F64, mem, inline_state_ptr, state_byte_off + 56);
                        let ha2 = fb
                            .ins()
                            .load(F64, mem, inline_state_ptr, state_byte_off + 64);
                        let ha3 = fb
                            .ins()
                            .load(F64, mem, inline_state_ptr, state_byte_off + 72);
                        fb.ins()
                            .jump(merge_b, &[hk.into(), ha1.into(), ha2.into(), ha3.into()]);

                        fb.switch_to_block(miss_b);
                        fb.seal_block(miss_b);
                        let inf = fb.ins().f64const(f64::INFINITY);
                        let two = fb.ins().f64const(2.0);
                        let nyq = fb.ins().fdiv(rate, two);
                        let c_ord = fb.ins().fcmp(FloatCC::Ordered, cutoff, cutoff);
                        let c_abs = fb.ins().fabs(cutoff);
                        let c_ninf = fb.ins().fcmp(FloatCC::LessThan, c_abs, inf);
                        let c_pos = fb.ins().fcmp(FloatCC::GreaterThan, cutoff, zero_f64);
                        let c_lt = fb.ins().fcmp(FloatCC::LessThan, cutoff, nyq);
                        let c_ok1 = fb.ins().band(c_ord, c_ninf);
                        let c_ok2 = fb.ins().band(c_pos, c_lt);
                        let c_ok = fb.ins().band(c_ok1, c_ok2);
                        let c_ok_b = fb.create_block();
                        let c_fail_b = fb.create_block();
                        fb.ins().brif(c_ok, c_ok_b, &[], c_fail_b, &[]);
                        fb.switch_to_block(c_fail_b);
                        fb.seal_block(c_fail_b);
                        let err_c = fb
                            .ins()
                            .iconst(ptr_type, errors.svf_cutoff_invalid as usize as i64);
                        fb.ins().call_indirect(
                            svf_err_sig_ref,
                            err_c,
                            &[compiled_node, cutoff, rate, error_out],
                        );
                        fb.ins().return_(&[offset]);

                        fb.switch_to_block(c_ok_b);
                        fb.seal_block(c_ok_b);
                        let q_ord = fb.ins().fcmp(FloatCC::Ordered, q, q);
                        let q_abs = fb.ins().fabs(q);
                        let q_ninf = fb.ins().fcmp(FloatCC::LessThan, q_abs, inf);
                        let q_pos = fb.ins().fcmp(FloatCC::GreaterThan, q, zero_f64);
                        let q_ok1 = fb.ins().band(q_ord, q_ninf);
                        let q_ok = fb.ins().band(q_ok1, q_pos);
                        let q_ok_b = fb.create_block();
                        let q_fail_b = fb.create_block();
                        fb.ins().brif(q_ok, q_ok_b, &[], q_fail_b, &[]);
                        fb.switch_to_block(q_fail_b);
                        fb.seal_block(q_fail_b);
                        let err_q = fb
                            .ins()
                            .iconst(ptr_type, errors.svf_q_invalid as usize as i64);
                        fb.ins().call_indirect(
                            svf_err_sig_ref,
                            err_q,
                            &[compiled_node, q, rate, error_out],
                        );
                        fb.ins().return_(&[offset]);

                        fb.switch_to_block(q_ok_b);
                        fb.seal_block(q_ok_b);
                        let pi = fb.ins().f64const(std::f64::consts::PI);
                        let one_f64 = fb.ins().f64const(1.0);
                        let pi_c = fb.ins().fmul(pi, cutoff);
                        let tan_arg = fb.ins().fdiv(pi_c, rate);
                        let tan_callee = fb
                            .ins()
                            .iconst(ptr_type, jit_math_tan as *const () as usize as i64);
                        let tan_c =
                            fb.ins()
                                .call_indirect(math_f64_sig_ref, tan_callee, &[tan_arg]);
                        let g = fb.inst_results(tan_c)[0];
                        let k = fb.ins().fdiv(one_f64, q);
                        let g_plus_k = fb.ins().fadd(g, k);
                        let g_gk = fb.ins().fmul(g, g_plus_k);
                        let denom = fb.ins().fadd(one_f64, g_gk);
                        let a1 = fb.ins().fdiv(one_f64, denom);
                        let a2 = fb.ins().fmul(g, a1);
                        let a3 = fb.ins().fmul(g, a2);
                        fb.ins()
                            .store(mem, c_bits, inline_state_ptr, state_byte_off + 32);
                        fb.ins()
                            .store(mem, q_bits, inline_state_ptr, state_byte_off + 40);
                        fb.ins()
                            .store(mem, k, inline_state_ptr, state_byte_off + 48);
                        fb.ins()
                            .store(mem, a1, inline_state_ptr, state_byte_off + 56);
                        fb.ins()
                            .store(mem, a2, inline_state_ptr, state_byte_off + 64);
                        fb.ins()
                            .store(mem, a3, inline_state_ptr, state_byte_off + 72);
                        fb.ins()
                            .jump(merge_b, &[k.into(), a1.into(), a2.into(), a3.into()]);

                        fb.switch_to_block(merge_b);
                        fb.seal_block(merge_b);
                        let kv = fb.block_params(merge_b)[0];
                        let a1v = fb.block_params(merge_b)[1];
                        let a2v = fb.block_params(merge_b)[2];
                        let a3v = fb.block_params(merge_b)[3];
                        let two = fb.ins().f64const(2.0);
                        let mut out_lr = [zero_f64, zero_f64];
                        let in_lr = [sum_l, sum_r];
                        for ch in 0..channels {
                            let inp = in_lr[ch];
                            let s0_off = state_byte_off + (ch as i32) * 16;
                            let s1_off = s0_off + 8;
                            let s0 = fb.ins().load(F64, mem, inline_state_ptr, s0_off);
                            let s1 = fb.ins().load(F64, mem, inline_state_ptr, s1_off);
                            let v3 = fb.ins().fsub(inp, s1);
                            let a1s0 = fb.ins().fmul(a1v, s0);
                            let a2v3 = fb.ins().fmul(a2v, v3);
                            let v1 = fb.ins().fadd(a1s0, a2v3);
                            let a2s0 = fb.ins().fmul(a2v, s0);
                            let a3v3 = fb.ins().fmul(a3v, v3);
                            let v2_first = fb.ins().fadd(s1, a2s0);
                            let v2 = fb.ins().fadd(v2_first, a3v3);
                            let two_v1 = fb.ins().fmul(two, v1);
                            let ns0 = fb.ins().fsub(two_v1, s0);
                            let two_v2 = fb.ins().fmul(two, v2);
                            let ns1 = fb.ins().fsub(two_v2, s1);
                            fb.ins().store(mem, ns0, inline_state_ptr, s0_off);
                            fb.ins().store(mem, ns1, inline_state_ptr, s1_off);
                            let out = match mode {
                                0 => v2,
                                1 => fb.ins().fmul(kv, v1),
                                _ => {
                                    let kv_v1 = fb.ins().fmul(kv, v1);
                                    let hp1 = fb.ins().fsub(inp, kv_v1);
                                    fb.ins().fsub(hp1, v2)
                                }
                            };
                            emit_finite_check(fb, out);
                            out_lr[ch] = out;
                        }
                        node_l[node_idx] = out_lr[0];
                        node_r[node_idx] = out_lr[1];
                    }
                    JitNodeKind::Helper => {
                        // Direct call to node's specialized `eval_fn`
                        let eval_callee = fb.ins().iconst(ptr_type, node.eval_fn as i64);
                        let call = fb.ins().call_indirect(
                            eval_sig_ref,
                            eval_callee,
                            &[
                                compiled_node,
                                runtime_node,
                                frame,
                                rate,
                                pitch_hz,
                                timbre,
                                pressure,
                                sum_l,
                                sum_r,
                                out_l_addr,
                                out_r_addr,
                                error_out,
                            ],
                        );
                        let err_flag = fb.inst_results(call)[0];
                        let zero_i8 = fb.ins().iconst(I8, 0);
                        let is_ok = fb.ins().icmp(
                            cranelift_codegen::ir::condcodes::IntCC::Equal,
                            err_flag,
                            zero_i8,
                        );
                        let ok_blk = fb.create_block();
                        let fail_blk = fb.create_block();
                        fb.ins().brif(is_ok, ok_blk, &[], fail_blk, &[]);

                        fb.switch_to_block(fail_blk);
                        fb.seal_block(fail_blk);
                        fb.ins().return_(&[offset]);

                        fb.switch_to_block(ok_blk);
                        fb.seal_block(ok_blk);
                        node_l[node_idx] = fb.ins().load(F64, mem, out_l_addr, 0);
                        node_r[node_idx] = fb.ins().load(F64, mem, out_r_addr, 0);
                    }
                }
            }

            // Write graph output to output_ptr[offset]
            let sixteen = fb.ins().iconst(ptr_type, 16);
            let out_off = fb.ins().imul(offset, sixteen);
            let out_elem = fb.ins().iadd(output_ptr, out_off);
            fb.ins().store(mem, node_l[spec.output_node], out_elem, 0);
            fb.ins().store(mem, node_r[spec.output_node], out_elem, 8);
        };

        // Frame 0 (peeled)
        emit_frame(&mut fb, zero_usize, true);

        // Loop for offset in 1..frames
        let loop_hdr = fb.create_block();
        let loop_body = fb.create_block();
        let loop_exit = fb.create_block();
        fb.append_block_param(loop_hdr, ptr_type);
        fb.ins().jump(loop_hdr, &[one_usize.into()]);

        fb.switch_to_block(loop_hdr);
        let offset = fb.block_params(loop_hdr)[0];
        let cond = fb.ins().icmp(
            cranelift_codegen::ir::condcodes::IntCC::UnsignedLessThan,
            offset,
            frames,
        );
        fb.ins().brif(cond, loop_body, &[], loop_exit, &[]);

        fb.switch_to_block(loop_body);
        fb.seal_block(loop_body);
        emit_frame(&mut fb, offset, false);
        let next_offset = fb.ins().iadd(offset, one_usize);
        fb.ins().jump(loop_hdr, &[next_offset.into()]);
        fb.seal_block(loop_hdr);

        fb.switch_to_block(loop_exit);
        fb.seal_block(loop_exit);
        fb.ins().return_(&[frames]);

        fb.finalize(module.target_config());
    }

    module.define_function(func_id, &mut ctx).ok()?;
    module.clear_context(&mut ctx);
    module.finalize_definitions().ok()?;
    let code_ptr = module.get_finalized_function(func_id);
    let func: JitRenderBlockFn = unsafe { std::mem::transmute(code_ptr) };

    Some(Arc::new(JitCompiledGraph {
        _owner: Arc::new(JitModuleOwner {
            module: Some(module),
        }),
        func,
        inline_state_words: spec.inline_state_words,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn dummy_nonfinite(_compiled: *const c_void, _err: *mut c_void) {}
    unsafe extern "C" fn dummy_indexed(
        _compiled: *const c_void,
        _index: usize,
        _val: f64,
        _err: *mut c_void,
    ) {
    }
    unsafe extern "C" fn dummy_svf(
        _compiled: *const c_void,
        _val: f64,
        _rate: f64,
        _err: *mut c_void,
    ) {
    }
    unsafe extern "C" fn dummy_setup(
        _compiled: *const c_void,
        _runtime: *mut c_void,
        _controls: *const f64,
        _controls_len: usize,
        _err: *mut c_void,
    ) -> u8 {
        0
    }
    unsafe extern "C" fn dummy_eval(
        _compiled: *const c_void,
        _runtime: *mut c_void,
        frame: u64,
        _rate: f64,
        _pitch: f64,
        _timbre: f64,
        _pressure: f64,
        in_l: f64,
        in_r: f64,
        out_l: *mut f64,
        out_r: *mut f64,
        _err: *mut c_void,
    ) -> u8 {
        unsafe {
            *out_l = in_l + frame as f64;
            *out_r = in_r - frame as f64;
        }
        0
    }

    #[test]
    fn jit_compiles_and_executes_block() {
        let spec = JitGraphSpec {
            order: vec![0],
            nodes: vec![JitNodeSpec {
                kind: JitNodeKind::Helper,
                has_sample_modulations: false,
                reads_input_later: false,
                incoming: vec![JitSource::Input],
                mod_sources: vec![],
                base_params: vec![],
                sample_controls: vec![],
                sample_mods: vec![],
                bounds: vec![],
                inline_state_offset: 0,
                eval_fn: dummy_eval as *const () as usize,
                setup_static_fn: dummy_setup as *const () as usize,
            }],
            output_node: 0,
            inline_state_words: 0,
        };
        let errors = JitErrorCallbacks {
            nonfinite_incoming: dummy_nonfinite,
            nonfinite_output: dummy_nonfinite,
            mod_product_nonfinite: dummy_indexed,
            mod_sum_nonfinite: dummy_indexed,
            param_out_of_range: dummy_indexed,
            onepole_invalid: dummy_nonfinite,
            svf_cutoff_invalid: dummy_svf,
            svf_q_invalid: dummy_svf,
        };
        let compiled =
            compile_graph_jit(&spec, errors).expect("Cranelift JIT should compile on this host");
        let mut rt = std::ptr::null_mut();
        let mut rp = std::ptr::null_mut();
        let mut cp = std::ptr::null();
        let exprs = [JitExpressionInput::default(); 4];
        let inputs = [[1.0, 2.0], [3.0, 4.0], [5.0, 6.0], [7.0, 8.0]];
        let mut outputs = [[0.0; 2]; 4];
        let mut ctx = JitBlockContext {
            runtime_nodes_ptr: &mut rt,
            runtime_params_ptr: &mut rp,
            inline_state_ptr: std::ptr::null_mut(),
            compiled_nodes_ptr: &mut cp,
            controls_ptr: std::ptr::null(),
            controls_len: 0,
            expressions_ptr: exprs.as_ptr(),
            input_ptr: inputs.as_ptr(),
            output_ptr: outputs.as_mut_ptr(),
            error_out: std::ptr::null_mut(),
            start_frame: 10,
            frames: 4,
            rate: 48_000.0,
            settled: 1,
        };
        let done = unsafe { compiled.call(&mut ctx) };
        assert_eq!(done, 4);
        assert_eq!(
            outputs,
            [[11.0, -8.0], [14.0, -7.0], [17.0, -6.0], [20.0, -5.0]]
        );
    }
}
