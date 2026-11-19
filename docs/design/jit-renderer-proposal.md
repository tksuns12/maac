# JIT renderer for compiled instrument graphs: proposal

**Status:** revised draft proposal (2026-11-19, updated after Phase 0 profiling and architecture review).

## Why

In the [song work budget proposal](song-budget-proposal.md), full `maac build`
times on an Apple M1 Pro measured 49.1 s for "Lantern Street" (2:09) and 68.4 s
for "Afterlight" (2:44). Phase 0 profiling (`CARGO_PROFILE_RELEASE_DEBUG=true`
with macOS `sample` at 1 ms) established the actual CPU breakdown:

| Component | Afterlight (66.3 s user) | Lantern Street (48.0 s -> 43.7 s user) |
| --- | ---: | ---: |
| `GraphState::render_block` + `process_graph_node` (self) | **34.8%** | **39.6%** |
| `num_bigint` / `num_rational` (`expression_inputs`) | **4.4%+** | **14.1%+** (reduced in Phase 0) |
| `memcmp` (`settled_controls`) + `malloc`/`free`/`memmove` | **13.1%** (reduced in Phase 0) | **5.9%** (reduced in Phase 0) |
| `Modal::sample` | **3.2%** | **9.5%** |
| `Oscillator::sample` + `Adsr::value` | **4.4%** | **3.7%** |
| Transcendentals (`sincos`, `exp`, `expm1`, `tan`, `log1p`) | **4.0%** | **2.4%** |

Phase 0 eliminated `memcmp` on `settled_controls` (via `controls_generation`),
reused per-block scratch `Vec`s in `InstrumentRuntime::fill`, and pre-compiled
`ExpressionRuntime` segments to `i128`/constant fast paths with exact IEEE-754
rounding, cutting "Lantern Street" user CPU time from 47.97 s to 43.70 s with
bit-identical WAV output.

The single largest remaining bottleneck (35–40% of total CPU) is the
**instrument graph interpreter** in `src/voice.rs`:

1. **Enum dispatch per sample:** `process_graph_node` matches
   `(&compiled.processor, state)` for every node on every frame, preventing
   inlining across connected nodes.
2. **Repeated parameter setup & validation:** When `has_sample_modulations` is
   set, `render_block` copies `base_params`, applies controls, loops over
   `modulations`, and calls `validate_params` on every frame.
3. **Scratch memory round-trips:** Node outputs are written to and summed from
   `scratch[node_index * stride + offset]` through `sum_incoming` on every
   frame, rather than staying in CPU registers.

## Fixed Design Decisions

1. **Scope — Instrument `voice` and `shared` `CompiledGraph` block kernel:**
   - Target `CompiledGraph` inside `InstrumentRuntime` (`src/voice.rs`).
   - **Out of scope for v1:** `EffectRuntime` and `synth.delay/1`.
     `synth.delay/1` is `Placement::Effect` only (`processor.rs`) and never
     appears in instrument graphs, and `DspEngine::process_node` always calls
     `EffectRuntime::process` for 1 frame at a time (`dsp.rs`), so effect
     graphs cannot benefit from a multi-frame block kernel until `DspEngine`
     gains block-based bus evaluation.
   - Keep `DspEngine` (`src/dsp.rs`) event scheduling, automation evaluation,
     top-level bus mixing, `production_*` processors, and `PlanLimits` in Rust.
2. **Backend — Cranelift (`cranelift-jit`):**
   - Pure Rust (`cranelift-codegen`, `cranelift-jit`, `cranelift-module`,
     `cranelift-native`), supporting `aarch64` (Apple Silicon / Linux ARM64)
     and `x86_64` with sub-10 ms compile times for `MAX_GRAPH_NODES` graphs.
   - Gated behind an **opt-in** Cargo feature (`jit`, **default off**), keeping
     the Rust interpreter as the default path, fallback, and reference oracle.
3. **Determinism & Compatibility — Bit-identical to the Rust interpreter:**
   - On the same machine and build, JIT output must match the Rust interpreter
     **bit-for-bit** in `binary64`, and must report the **exact same
     `RenderError` at the exact same frame**.

## Preserving Bit-Identical `binary64` Semantics

To guarantee zero ULP drift between the JIT and the Rust interpreter:

1. **Disable FMA contraction and fast-math:**
   - Configure Cranelift ISA flags with FMA disabled and preserve strict
     IEEE-754 operation order (`a * b + c` remains `fmul` followed by `fadd`
     with matching associativity, e.g. `((-2.0 * PI * cutoff) / rate).exp()`).
2. **Call Rust/libm `extern "C"` helpers for transcendental functions:**
   - `sin`, `cos`, `tan`, `tanh`, `exp`, `pow`, `log2`, `rem_euclid` call thin
     `extern "C"` shims wrapping Rust's `f64` methods, guaranteeing identical
     rounding to the interpreter.
3. **Exact error precedence via Frame-0 Peeling:**
   - The interpreter validates non-modulated parameters (`!settled && !per_sample`)
     **at each node's position in topological order** when `limit > 0`. Hoisting
     all non-modulated validations before frame 0 would invert error precedence
     if an earlier per-sample node fails at frame 0.
   - **Solution:** Peel frame `offset = 0` (when `frames > 0`) and run each
     node's parameter setup + validation in exact topological order, then run
     frames `1..frames` in the tight register-resident inner loop without
     re-validating non-modulated nodes.
4. **Full `RenderError` parity via `*mut Option<RenderError>`:**
   - Runtime errors embed formatted values (SVF `cutoff` and `rate/2`, Sample
     `frequency`/`pitch_hz`/`velocity`, oscillator `frequency_hz`, etc.). The
     JIT context passes a Rust-owned `*mut Option<RenderError>` slot filled by
     cold `extern "C"` error helpers, preserving exact error types and messages.
5. **Panic safety:**
   - Unwinding across `extern "C"` / JIT frames aborts the process. All JIT
     helper functions must be panic-free (checked arithmetic, bounds checked
     before indexing) or wrapped in `std::panic::catch_unwind`.

## Execution Model & Memory Layout

### 1. Frame-outer loop with register-resident signals

In the JIT-compiled block function, **one block call loops frames on the outer
loop and topological node order on the inner loop**:

```text
// Frame 0 (peeled when !settled):
//   Evaluate nodes in topological order; validate non-modulated nodes at
//   their topological position; on any error, fill *error_out and return 0.

for offset in 1..frames:
    // Evaluate each node in topological order:
    // - Intermediate audio signals stay in Cranelift SSA registers.
    // - On any validation/nonfinite error: fill *error_out, unpack committed
    //   state for completed frames, and return `offset`.
return frames
```

Benefits:
- **Zero `scratch` traffic** between nodes in a voice graph; audio signals stay
  in registers.
- **Exact first-error semantics:** returning immediately at the failing frame
  matches frame-by-frame evaluation. Note that the interpreter advances earlier
  nodes past an error frame while JIT stops at the failing frame; this is safe
  under MaaC's invariant that a `GraphState` is not reused after returning an
  error (covered by an explicit test).

### 2. `ProcessorState` as Source of Truth (Pack / Unpack State Buffer)

`GraphState` is read and mutated by many non-block Rust paths (`preview_required`
with `commit = false`, `release`, `release_modulated`, `restart_envelopes`,
`glide_velocity`, `lift_bows`, `play_line`, `excite_unmodulated`, `finished`,
`amplitude`, `attach_tempo`), and `GraphState: Clone` is required for
`shared_reset` and `reset_state`.

Rather than replacing `ProcessorState` (which would break `Clone` if raw
pointers were stored and require porting every event path):

- **`ProcessorState` remains the single source of truth.**
- Before each JIT block call, a fast **pack** step copies small inline DSP state
  (`Noise(u32)`, `OnePole([f64; 2])`, `Svf([[f64; 2]; 2])`, `Drive([DriveState; 2])`,
  `Wavetable { phase }`, `Oscillator` phase/state, `Velocity`/`Random` values,
  and `RuntimeNode::params` for non-modulated nodes) into a reusable,
  64-bit-aligned `#[repr(C)]` scratch buffer on `InstrumentRuntime`.
- Heavy/heap-backed states (`Bow`, `Modal`, `Pluck`, `Sample`, `Adsr`) stay in
  `ProcessorState` and are passed via a per-call `*mut *mut c_void` side array
  of opaque pointers—never stored inside `Clone`-able buffers.
- After the JIT block returns (whether `frames` or an early error frame),
  **unpack** writes the modified inline state and `params` back to
  `ProcessorState`.
- `preview_required` (`commit = false`) stays on the Rust interpreter path,
  automatically seeing the exact same `ProcessorState`.

### 3. JIT Function Signature & ABI

```rust
#[repr(C)]
pub struct JitExpressionInput {
    pub pitch_hz: f64,
    pub timbre: f64,
    pub pressure: f64,
}

#[repr(C)]
pub struct JitBlockContext {
    pub inline_state_ptr: *mut u8,
    pub opaque_ptrs: *const *mut std::ffi::c_void,
    pub controls_ptr: *const f64,
    pub expressions_ptr: *const JitExpressionInput,
    pub input_ptr: *const [f64; 2],       // null when graph has no audio input
    pub output_ptr: *mut [f64; 2],        // block output buffer [frames]
    pub error_out: *mut Option<RenderError>,
    pub start_frame: u64,
    pub frames: usize,
    pub rate: f64,
    pub settled: u8,
}

// Returns number of completed frames (0..=ctx.frames).
pub type JitRenderBlockFn = unsafe extern "C" fn(*mut JitBlockContext) -> usize;
```

- `settled` is computed in Rust (`self.settled_generation == Some(controls_generation)`)
  and updated in Rust only when `error.is_none() && frames > 0`.
- `VoiceState` expressions use `#[repr(C)] JitExpressionInput` directly in
  `InstrumentRuntime::expressions` to guarantee ABI layout.

### 4. Two-Tier Node Compilation Strategy

1. **Inline Cranelift IR (Hot path, no call overhead):**
   - Routing & gain: `Gain`, `Mix`, `Width`, `sum_incoming`
   - Expressions & sources: `Timbre`, `Pressure`, `Velocity`, `Random`, `Key`,
     `Noise` (xorshift32), `Pan`/`Balance` (with `cos`/`sin` shim)
   - Filters & nonlinearities: `OnePole`, `HighPass`, `Svf` step, `Drive`
     (`tanh` shim)
   - Modulations & validation: `apply_controls`, `sample_modulations`, range
     checks (`CompiledBounds`), `is_finite` checks
2. **Direct `extern "C"` Helper Calls (Complex/table-backed, no enum match):**
   - `Oscillator`, `Lfo`, `Adsr` (kept closed-form as a function of `frame` in
     Phase 2 as well, because `fill()` calls `finished`/`amplitude` outside the
     block), `Wavetable`, `Sample` (lazy layer selection on `commit = true`
     only), `Pluck`, `Modal`, `Bow`.
   - The JIT emits a direct call to the concrete helper function using the
     per-call `opaque_ptrs[node_index]`, eliminating `ProcessorCode` enum
     matching.

### 5. Lifetime, W^X, Build Policy, and Security

- **Ownership & `Drop`:** Each `CompiledInstrument` holds an
  `Option<Arc<JitModule>>` (dropped before the graph, `Send + Sync` so
  `freeze`'s `thread::scope` remains valid) whose `Drop` calls
  `JITModule::free_memory` safely.
- **W^X & Platform Fallback:** On Apple Silicon (`MAP_JIT` / hardened runtime
  `allow-jit` entitlement) or Linux SELinux `execmem` / PaX hosts, `JITModule`
  creation may be denied. Creation failures must be caught without panicking and
  fall back cleanly to the Rust interpreter, with a diagnostic/report field
  indicating which backend ran.
- **Dependency & Offline Build Policy:** `CONTRIBUTING.md` requires
  `cargo fetch --locked` and `--offline`. Even as an optional `jit` feature,
  Cranelift crates (`cranelift-codegen`, `cranelift-jit`, `regalloc2`, etc.)
  enter `Cargo.lock`. CI must test `--features jit` on `aarch64-darwin`,
  `x86_64-linux`, and `aarch64-linux`.
- **Security:** Per `SECURITY.md`, untrusted plans are in scope. Because a JIT
  codegen bug can turn into arbitrary code execution, the `jit` feature stays
  **disabled by default** until differential fuzzing (covering `-0.0`, `NaN`,
  subnormals, extreme bounds, and `Sample` zone errors) passes.

## Phased Implementation Plan

### Phase 0: Profiling & Pure-Rust Low-Risk Wins (Completed)
- Profiled "Afterlight" and "Lantern Street" with debug-symbol release builds.
- Replaced `settled_controls` `memcmp` with `controls_generation`, reused `fill`
  scratch `Vec`s, and pre-compiled `ExpressionRuntime` segments to eliminate
  per-frame `BigInt::gcd` allocations.
- **Next measurement before Phase 1:** Measure the distribution of
  `frames == 1` vs `frames > 1` calls to `InstrumentRuntime::fill` across all
  5 trial songs (`fixed_parameters` hit rate). If automated instruments cause
  frequent `frames == 1` calls, evaluate lowering `DspEngine` automation
  blocks before or alongside Phase 1.

### Phase 1: Infrastructure, Pack/Unpack State, and Helper-Backed JIT
- Add optional `jit` Cargo feature (default off) with Cranelift.
- Implement `src/jit/` with `JitCompiler`, `JitBlockContext`, pack/unpack
  state layout, Frame-0 peeling, `*mut Option<RenderError>` channel, and
  `extern "C"` node helpers.
- Verify **100% bit-identical WAV output and error parity** against the
  interpreter across the full test suite and all 5 showcase songs.

### Phase 2: Inline Hot-Path DSP & Register Promotion
- Inline `Noise`, `Gain`, `Mix`, `Width`, `Timbre`, `Pressure`, `Velocity`,
  `Random`, `OnePole`, `HighPass`, `Svf` step, `Oscillator`, and closed-form
  `Adsr` in Cranelift IR (keeping `extern "C"` shims for transcendentals).
- Benchmark `maac build` on "Lantern Street" and "Afterlight" (expected target:
  3–5× speedup on the `render_block` portion, translating to ~1.25–1.5×
  additional whole-song speedup on top of Phase 0).

## Verification & Acceptance Criteria

1. **Bit-identical parity:** Differential test runner executes all integration
   tests and the 5 trial songs ("Homeward", "Afterlight", "Lantern Street",
   "Morning Ferry", "Late Window") with JIT enabled vs. disabled; SHA-256 of
   rendered WAV bytes must match identically.
2. **Error parity:** All `RenderError` tests fail with the exact same error
   code, message, and frame index; add a test asserting `GraphState` is not
   reused after an error.
3. **Differential fuzzing:** Fuzz extreme inputs (`-0.0`, `NaN`, subnormals,
   zero-length / `frames == 1` blocks, `Sample` out-of-range zones).
4. **Fallback parity:** W^X denial or `--no-default-features` / disabled JIT
   uses the Rust interpreter with zero behavioral difference.
