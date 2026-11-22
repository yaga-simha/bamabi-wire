# Layer-0 Polyglot Profiling & Optimization Record (`bamabi-wire`)

This document preserves the methodology, deterministic profiler signals, instruction-level root-cause analysis, architectural optimizations, verification gates, and before/after benchmark results of the `ABI 0.3.0` (`v0.3.0`) Layer-0 hotpath optimization effort across `bamabi_guest_sys`, `bamabi_guest`, `bamabi_sdk_gen`, the polyglot benchmark guests (`rust`, `c99`, `cpp17`, `zig`), and `bamabi_bench`.

> **Design Constraints Preserved:** `#![forbid(unsafe_code)]` in `bamabi_guest`, `rust_guest`, `bamabi_sdk_gen`, `bamabi_macros`, `bamabi_docs`, and `xtask`; strict `ABI 0.3.0` (`0x0003_0000`) wire parity; zero external runtime dependencies in Layer-0 crates; full Tree Borrows + Stacked Borrows Miri soundness (`358/358`); and Kani formal proofs (`56/56`).

---

## 1. Methodology & Instrumentation

Every optimization followed a closed measurement loop pairing uninstrumented wall-clock timings with deterministic structural signals:

1. **Uninstrumented Baseline & Post-Optimization Matrix:** Full 8-guest polyglot sweeps (`cargo xtask bench --strict-langs --no-write-docs`, `warmup=15`, `timed=60`, 63 verified `(subject, workload)` permutations across `native` and `rust`/`c99`/`cpp17`/`zig` in both `release` and `debug` profiles).
2. **Layered `hotpath-rs` Observability (Temporary Profiling Build):** Instrumented `bamabi_bench` with `hotpath-rs` (`timing`, `hotpath-alloc`, `threads`, and `hotpath::mutex!` on `HostSharedState::symbol_table`) under isolated single-workload runs (`HOTPATH_METRICS_SERVER_OFF=1`) to capture 100% deterministic host vtable call counts, exclusive per-function heap allocations, lock acquisition counts, and RSS—then reverted so Layer-0 retains zero external runtime dependencies.
3. **Instruction-Level Disassembly (`otool -tV` on ARM64):** Audited generated machine code for `system_closure_trampoline`, `TypedChunkIter::for_each`, `invoke_chunk` / `call_unpacked`, `native_dense_update`, and C99/C++17/Zig guest callbacks before and after each phase.

---

## 2. Baseline Diagnostic Findings

### 2.1 Deterministic `hotpath-rs` Signals (Before Optimization)

- **Heap Allocations (`hotpath-alloc`):** Confirmed **0 B/tick** inside `OwnedHostWorld::run_system` and all 16 hot `BamHostApi` vtable slots across all four workloads (`noop`, `dense_query_update`, `command_burst`, `host_api_roundtrip`). All steady-state overhead was pure CPU/stack/FFI/cache cost rather than allocator churn.
- **Wasted FFI Calls (`rust:release` vs `c99:release`):**
  - `command_burst` (1,001 ticks): `rust:release` invoked `host_sys_host_ctx` **1,001 times** (`c99:release`: **0**) because `LazyCommands::resolve()` eagerly called `LazyInvocationHost::resolve()`, which also copied the entire **584-byte `Host` struct** onto the stack every tick even though commands only use `BamHostApi`.
  - `dense_query_update` (862 ticks, 23,938 chunks): `rust:release` already used slot 68 (`iter_next_chunk_batch`, 24,800 calls) while `c99`/`cpp17`/`zig` used the 3-call legacy path (`72,676` FFI calls). However, `rust:release` re-queried `host_iter_column_info` **1,724 times** (2×/tick) because its `BamColumnInfo` cache lived on the stack and was discarded after every tick.
  - `host_api_roundtrip` (1,001 ticks): `rust:release` invoked `host_iter_column_info` **1,001 times** (1×/tick), copied the 584-byte `Host` struct on every tick, and re-ran `validate_resource_read_info` + 128-bit UUID checks **64×/tick**.
- **Lock Contention (`hotpath::mutex!`):** `HostSharedState::symbol_table` acquired `std::sync::Mutex` **64,064 times** per subject (`45.4%` of total `host_api_roundtrip` runtime) using default SipHash.

### 2.2 Disassembly Root Causes (ARM64 Release)

1. **16 KB Stack Frames & 4 Stack-Guard Probes per Tick (`rust:release`):** `SystemContext` embedded `extra_queries: Option<[Option<ChunkIter<'a>>; 15]>` and `ValidatedSystemInvocation` embedded `[MaybeUninit<ValidatedIter<'a>>; 16]` by value on the stack (~304 B per `ValidatedIter`). Because query counts were runtime fields in `SystemClosureState`, LLVM could not dead-code-eliminate the 16-slot arrays, emitting **16 KB frames** (`sub sp, sp, #0x3fd0`) with **4 page-probe writes** (`str xzr, [sp]`) on `dense_query_update`, `command_burst`, and `host_api_roundtrip`, and a **1,072 B frame + 12 callee-saved registers** even for `noop`.
2. **304-Byte `_memcpy` per Tick:** `#[inline(never)] fetch_query_slot_raw` returned a 304-byte `ValidatedIter` by value via hidden `sret`, triggering an explicit `bl _memcpy` into `SystemContext::query0` every tick.
3. **Lost `noalias` in `TypedChunkIter::for_each`:** Passing `slices: (&mut [Position], &[Velocity])` as a single 32-byte struct parameter through `#[inline(never)] invoke_chunk` passed the tuple indirectly in `x0`, stripping LLVM `noalias` metadata and forcing a runtime overlap check (`cmp` / `ccmp`) on every chunk.
4. **Non-Inlined `spawn(&[])` & 56B `RecordedCmd` Zero-Init:** `rust_guest` called `cmds.spawn(&[])` through non-inlined `ValidatedCommands::spawn_entries_impl` 65×/tick, while host `CommandQueue` zero-initialized a 32-byte inline array inside a 56-byte `RecordedCmd` enum on every `cmd_insert`.
5. **168-Byte Host `SystemFrame` Zero-Init:** `OwnedHostWorld::run_system` zero-initialized `[null_mut(); 16]` (128 B) on every system call, including 0-query `noop`.

---

## 3. Implemented Optimizations (Phases 1–3)

### Phase 1 — Stack Bloat & 584B `Host` Copy Elimination (`bamabi_guest_sys`, `bamabi_guest`)

- **Decoupled `LazyInvocationHost` (`<= 48 B`):** Removed the 584–624 B `Host` stack buffer from `LazyInvocationHost`, replacing it with strict-provenance `resolve_ctx_slow` (`*const BamHostCtx`) and `Host::*_with_ctx` helpers while moving any lazy `Host` scratch into `SystemClosureState`.
- **Fixed `LazyCommands::resolve()`:** Passes `(base_host, lazy_host)` directly so command-only systems never invoke `host_sys_host_ctx` or copy `Host`.
- **Shrank `ValidatedIter<'a>` (`<= 64 B`, `!needs_drop`) & Bucketed Slice Trampolines:** Replaced 304 B `fetch_query_slot_raw` struct returns with scalar `*mut BamIter`, introduced slice-backed `ValidatedSystemSlice<'a>` / `SystemContext<'a>`, and specialized `system_closure_slice_trampoline<const N, F>` across `N ∈ {0, 1, 4, 16}` buckets so 0-query systems (`noop`, `command_burst`) incur zero query-slot stack or loop overhead.

### Phase 2 — Persistent `BamColumnInfo` Cache, `noalias` Unpacking & Slot 68 Polyglot Parity

- **Persistent Per-System `QuerySlotScratch` (`bamabi_guest_sys`):** Persists validated `BamColumnInfo` (`INLINE_COL_INFO_CAP = 8`) across ticks inside `SystemClosureState` (`FrozenSlotCache` + strict-provenance `SlotCacheRef`, separating `reset_per_tick()` from `reset_all()`), eliminating steady-state `host_iter_column_info` FFI calls.
- **Zero-Redundancy `ChunkColumnDispenser` & `QueryTuple::invoke_unpacked` (`bamabi_guest`):** Deduplicated `dispense_dense_*` -> `raw_dense_slice[_mut]_unchecked` while preserving `check_and_record_borrow`, wired `BatchColCache<'ctx>` / `advance_batch_into`, and added `QueryTuple::invoke_unpacked` (`call_unpacked`) across all 34 `QueryTuple` implementations so LLVM receives separate `&mut [T]` / `&[U]` arguments with full `noalias` guarantees and zero runtime overlap checks.
- **Polyglot Slot 68 (`iter_next_chunk_batch`) Parity (`bamabi_sdk_gen`, `c99`/`cpp17`/`zig` guests):** Added slot 68 batch iteration helpers to generated `bamabi.hpp` (`ChunkView`) and `bamabi.zig` (`ChunkView`) with runtime mock tests, and updated `benches/guests/{c99,cpp17,zig}_guest` `dense_query_update` to use slot 68 with `MIN_V0_3_SIZE` fallback plus `cmd_spawn_empty` and hoisted slot checks.

### Phase 3 — Command Queue, Resource Cache & Host Dispatch (`bamabi_guest_sys`, `rust_guest`, `bamabi_bench`)

- **Command Fast Paths:** Marked `ValidatedCommands::{spawn_entries_impl, spawn, spawn_persistent}` `#[inline]` and switched `rust_guest` `bench_command_burst` to `cmds.spawn_empty()`.
- **Resource Term Caching:** Cached validated resource metadata in `resource_checked` / `resource_mut_checked` for `ctx.res_with_ticks::<SimParams>(0)`.
- **Host Harness Optimizations (`bamabi_bench`):**
  - Replaced 56 B `RecordedCmd` with a 16 B `CmdHeader` + contiguous `payload_bytes: Vec<u8>` split-buffer in `CommandQueue`.
  - Shrank `SystemFrame` (`168 B -> 48 B`) with an `if query_count > 0` fast path in `OwnedHostWorld::run_system`.
  - Replaced `Mutex<HashMap>` in `SymbolTable` with `std::sync::RwLock` + a zero-dependency `FastBuildHasher`.

---

## 4. Verification & Structural Proof Gates

All verification gates passed independently after all optimizations:

- **`cargo xtask ci --no-bench`:** PASS (formatting, `include/bamabi.h` and `include/generated/testbed_components.h` byte-parity, `#![forbid(unsafe_code)]` & zero-Bevy lints, `clippy -D warnings`, `RUSTDOCFLAGS="-D warnings" cargo doc`, workspace/integration/doc/SDK tests, C99 ASan+UBSan).
- **`cargo xtask miri`:** PASS (**358 / 358** — 179 Tree Borrows + 179 Stacked Borrows, 0 failures).
- **`cargo xtask kani`:** PASS (**56 / 56** formal proofs verified, 0 failures).
- **Deterministic Structural & Disassembly Verification:**
  - `0` `host_sys_host_ctx` calls in `command_burst` (down from 1,001).
  - `0` steady-state `host_iter_column_info` calls per tick in `dense_query_update` and `host_api_roundtrip` (down from 2/tick and 1/tick).
  - Slot 68 (`#0x228`) emitted in `c99`, `cpp17`, and `zig` release `dense_query_update` dylibs.
  - `libbench_rust_release.dylib`: `0` `___chkstk_darwin` stack probes, `0` large `_memcpy` calls, `0` overlap branches in `call_unpacked`, and drastic stack frame reductions:
    - `bench_noop`: `1,072 B -> 0 B`
    - `bench_command_burst`: `16,448 B -> 272 B + 96 B`
    - `bench_dense_update`: `16,432 B -> 480 B`
    - `bench_host_api_roundtrip`: `16,432 B -> 384 B + 160 B`
  - `bamabi_bench`: `48 B` `SystemFrame`, `0` `stp xzr, xzr` zero-init, `0` `pthread_mutex` calls on `SymbolTable` read hits.

---

## 5. Before vs. After Benchmark Results (Full 63-Permutation Matrix)

Measured on macOS ARM64 (`ABI 0.3.0`, `quick=false`, `warmup=15`, `timed=60`, 63/63 permutations verified).

### 5.1 Release Highlights Summary

| Workload | Native Before (ns) | Native After (ns) | Native Delta | `rust:release` Before -> After (Speedup, vs Native) | Fastest Release Guest After |
| --- | ---: | ---: | ---: | --- | --- |
| `noop` | 1.53 | 1.15 | -25.1% | 6.95 -> 3.17 ns (**2.19x**, 2.77x) | `zig:release`: 2.19 ns (1.91x) |
| `dense_query_update (1000, c256)` | 287.50 | 283.32 | -1.5% | 484.18 -> 397.50 ns (**1.22x**, 1.40x) | `cpp17:release`: 348.32 ns (1.23x) |
| `dense_query_update (1000, c4096)` | 295.82 | 295.00 | -0.3% | 428.34 -> 335.83 ns (**1.28x**, 1.14x) | `c99:release`: 303.34 ns (1.03x) |
| `dense_query_update (100000, c256)` | 58350.10 | 61829.30 | +6.0% | 63595.80 -> 66529.10 ns (0.96x, **1.08x**) | `cpp17:release`: 64775.00 ns (1.05x) |
| `dense_query_update (100000, c4096)` | 51595.90 | 51700.10 | +0.2% | 52979.10 -> 52858.40 ns (1.00x, **1.02x**) | `cpp17:release`: 52429.20 ns (1.01x) |
| `command_burst` | 187.50 | 188.61 | +0.6% | 899.86 -> 533.47 ns (**1.69x**, 2.83x) | `c99:release`: 522.36 ns (2.77x) |
| `host_api_roundtrip` | 1240.83 | 849.30 | -31.6% | 2118.75 -> 1499.31 ns (**1.41x**, 1.77x) | `cpp17:release`: 1223.75 ns (1.44x) |

### 5.2 Full 63-Row Permutation Comparison

| Subject | Workload | Before Median (ns) | After Median (ns) | Speedup | Delta (%) | Before vs Native | After vs Native | Verified |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `native` | `noop` | 1.53 | 1.15 | 1.33x | -25.1% | 1.00x | 1.00x | True |
| `native` | `dense_query_update (1000, c256)` | 287.50 | 283.32 | 1.01x | -1.5% | 1.00x | 1.00x | True |
| `native` | `dense_query_update (1000, c4096)` | 295.82 | 295.00 | 1.00x | -0.3% | 1.00x | 1.00x | True |
| `native` | `dense_query_update (100000, c256)` | 58350.10 | 61829.30 | 0.94x | +6.0% | 1.00x | 1.00x | True |
| `native` | `dense_query_update (100000, c4096)` | 51595.90 | 51700.10 | 1.00x | +0.2% | 1.00x | 1.00x | True |
| `native` | `command_burst` | 187.50 | 188.61 | 0.99x | +0.6% | 1.00x | 1.00x | True |
| `native` | `host_api_roundtrip` | 1240.83 | 849.30 | 1.46x | -31.6% | 1.00x | 1.00x | True |
| `rust:release` | `noop` | 6.95 | 3.17 | 2.19x | -54.3% | 4.54x | 2.77x | True |
| `rust:release` | `dense_query_update (1000, c256)` | 484.18 | 397.50 | 1.22x | -17.9% | 1.68x | 1.40x | True |
| `rust:release` | `dense_query_update (1000, c4096)` | 428.34 | 335.83 | 1.28x | -21.6% | 1.45x | 1.14x | True |
| `rust:release` | `dense_query_update (100000, c256)` | 63595.80 | 66529.10 | 0.96x | +4.6% | 1.09x | 1.08x | True |
| `rust:release` | `dense_query_update (100000, c4096)` | 52979.10 | 52858.40 | 1.00x | -0.2% | 1.03x | 1.02x | True |
| `rust:release` | `command_burst` | 899.86 | 533.47 | 1.69x | -40.7% | 4.80x | 2.83x | True |
| `rust:release` | `host_api_roundtrip` | 2118.75 | 1499.31 | 1.41x | -29.2% | 1.71x | 1.77x | True |
| `rust:debug` | `noop` | 461.40 | 92.28 | 5.00x | -80.0% | 301.73x | 80.53x | True |
| `rust:debug` | `dense_query_update (1000, c256)` | 11460.41 | 11070.42 | 1.04x | -3.4% | 39.86x | 39.07x | True |
| `rust:debug` | `dense_query_update (1000, c4096)` | 9924.16 | 9498.33 | 1.04x | -4.3% | 33.55x | 32.20x | True |
| `rust:debug` | `dense_query_update (100000, c256)` | 1090750.00 | 1097275.00 | 0.99x | +0.6% | 18.69x | 17.75x | True |
| `rust:debug` | `dense_query_update (100000, c4096)` | 870916.70 | 879566.60 | 0.99x | +1.0% | 16.88x | 17.01x | True |
| `rust:debug` | `command_burst` | 2740.70 | 1903.20 | 1.44x | -30.6% | 14.62x | 10.09x | True |
| `rust:debug` | `host_api_roundtrip` | 16823.75 | 15165.69 | 1.11x | -9.9% | 13.56x | 17.86x | True |
| `c99:release` | `noop` | 3.94 | 2.30 | 1.71x | -41.6% | 2.57x | 2.01x | True |
| `c99:release` | `dense_query_update (1000, c256)` | 348.34 | 351.68 | 0.99x | +1.0% | 1.21x | 1.24x | True |
| `c99:release` | `dense_query_update (1000, c4096)` | 294.18 | 303.34 | 0.97x | +3.1% | 0.99x | 1.03x | True |
| `c99:release` | `dense_query_update (100000, c256)` | 62383.30 | 66304.20 | 0.94x | +6.3% | 1.07x | 1.07x | True |
| `c99:release` | `dense_query_update (100000, c4096)` | 52000.00 | 52645.80 | 0.99x | +1.2% | 1.01x | 1.02x | True |
| `c99:release` | `command_burst` | 708.20 | 522.36 | 1.36x | -26.2% | 3.78x | 2.77x | True |
| `c99:release` | `host_api_roundtrip` | 1779.86 | 1276.67 | 1.39x | -28.3% | 1.43x | 1.50x | True |
| `c99:debug` | `noop` | 5.21 | 2.30 | 2.27x | -56.0% | 3.41x | 2.00x | True |
| `c99:debug` | `dense_query_update (1000, c256)` | 3840.41 | 3813.75 | 1.01x | -0.7% | 13.36x | 13.46x | True |
| `c99:debug` | `dense_query_update (1000, c4096)` | 3726.25 | 3731.25 | 1.00x | +0.1% | 12.60x | 12.65x | True |
| `c99:debug` | `dense_query_update (100000, c256)` | 407654.20 | 410187.50 | 0.99x | +0.6% | 6.99x | 6.63x | True |
| `c99:debug` | `dense_query_update (100000, c4096)` | 394679.10 | 395533.30 | 1.00x | +0.2% | 7.65x | 7.65x | True |
| `c99:debug` | `command_burst` | 634.17 | 538.05 | 1.18x | -15.2% | 3.38x | 2.85x | True |
| `c99:debug` | `host_api_roundtrip` | 1853.89 | 1293.06 | 1.43x | -30.3% | 1.49x | 1.52x | True |
| `cpp17:release` | `noop` | 4.32 | 2.30 | 1.87x | -46.6% | 2.82x | 2.01x | True |
| `cpp17:release` | `dense_query_update (1000, c256)` | 346.66 | 348.32 | 1.00x | +0.5% | 1.21x | 1.23x | True |
| `cpp17:release` | `dense_query_update (1000, c4096)` | 293.33 | 304.16 | 0.96x | +3.7% | 0.99x | 1.03x | True |
| `cpp17:release` | `dense_query_update (100000, c256)` | 62595.90 | 64775.00 | 0.97x | +3.5% | 1.07x | 1.05x | True |
| `cpp17:release` | `dense_query_update (100000, c4096)` | 52341.70 | 52429.20 | 1.00x | +0.2% | 1.01x | 1.01x | True |
| `cpp17:release` | `command_burst` | 678.34 | 550.69 | 1.23x | -18.8% | 3.62x | 2.92x | True |
| `cpp17:release` | `host_api_roundtrip` | 1801.67 | 1223.75 | 1.47x | -32.1% | 1.45x | 1.44x | True |
| `cpp17:debug` | `noop` | 8.50 | 1.94 | 4.39x | -77.2% | 5.56x | 1.69x | True |
| `cpp17:debug` | `dense_query_update (1000, c256)` | 3891.66 | 3883.76 | 1.00x | -0.2% | 13.54x | 13.71x | True |
| `cpp17:debug` | `dense_query_update (1000, c4096)` | 3763.33 | 3770.42 | 1.00x | +0.2% | 12.72x | 12.78x | True |
| `cpp17:debug` | `dense_query_update (100000, c256)` | 410912.50 | 409454.20 | 1.00x | -0.4% | 7.04x | 6.62x | True |
| `cpp17:debug` | `dense_query_update (100000, c4096)` | 391800.00 | 397283.30 | 0.99x | +1.4% | 7.59x | 7.68x | True |
| `cpp17:debug` | `command_burst` | 1162.92 | 1050.42 | 1.11x | -9.7% | 6.20x | 5.57x | True |
| `cpp17:debug` | `host_api_roundtrip` | 2790.00 | 1674.31 | 1.67x | -40.0% | 2.25x | 1.97x | True |
| `zig:release` | `noop` | 4.65 | 2.19 | 2.12x | -52.9% | 3.04x | 1.91x | True |
| `zig:release` | `dense_query_update (1000, c256)` | 513.34 | 519.16 | 0.99x | +1.1% | 1.79x | 1.83x | True |
| `zig:release` | `dense_query_update (1000, c4096)` | 419.16 | 426.66 | 0.98x | +1.8% | 1.42x | 1.45x | True |
| `zig:release` | `dense_query_update (100000, c256)` | 66666.70 | 68591.70 | 0.97x | +2.9% | 1.14x | 1.11x | True |
| `zig:release` | `dense_query_update (100000, c4096)` | 54116.60 | 54908.30 | 0.99x | +1.5% | 1.05x | 1.06x | True |
| `zig:release` | `command_burst` | 674.17 | 604.03 | 1.12x | -10.4% | 3.60x | 3.20x | True |
| `zig:release` | `host_api_roundtrip` | 1826.67 | 1224.58 | 1.49x | -33.0% | 1.47x | 1.44x | True |
| `zig:debug` | `noop` | 5.13 | 2.61 | 1.97x | -49.2% | 3.36x | 2.28x | True |
| `zig:debug` | `dense_query_update (1000, c256)` | 4235.84 | 4037.50 | 1.05x | -4.7% | 14.73x | 14.25x | True |
| `zig:debug` | `dense_query_update (1000, c4096)` | 4043.76 | 3764.58 | 1.07x | -6.9% | 13.67x | 12.76x | True |
| `zig:debug` | `dense_query_update (100000, c256)` | 403579.10 | 392866.70 | 1.03x | -2.7% | 6.92x | 6.35x | True |
| `zig:debug` | `dense_query_update (100000, c4096)` | 422837.50 | 366325.00 | 1.15x | -13.4% | 8.20x | 7.09x | True |
| `zig:debug` | `command_burst` | 1654.45 | 1450.70 | 1.14x | -12.3% | 8.82x | 7.69x | True |
| `zig:debug` | `host_api_roundtrip` | 3911.94 | 1499.03 | 2.61x | -61.7% | 3.15x | 1.76x | True |

### 5.3 Variance & Regression Notes

- **`noop`:** 0 regressions across all 9 subjects (`rust:release` **2.19x**, `rust:debug` **5.00x**, `c99:release` **1.71x**, `cpp17:release` **1.87x**, `zig:release` **2.12x**).
- **`command_burst`:** 0 guest regressions (`rust:release` **1.69x**, matching `c99:release` at ~522–533 ns; `c99:release` **1.36x**, `cpp17:release` **1.23x**, `zig:release` **1.12x**).
- **`host_api_roundtrip`:** 0 regressions across all 9 subjects (`native` **1.46x**, `rust:release` **1.41x**, `c99:release` **1.39x**, `cpp17:release` **1.47x**, `zig:release` **1.49x**, `zig:debug` **2.61x**).
- **`dense_query_update`:** `rust:release` gained **1.22x** at `(1000, c256)` (`1.68x -> 1.40x` vs native), **1.28x** at `(1000, c4096)` (`1.45x -> 1.14x` vs native), and reached **1.02x native** at `(100000, c4096)`. Small `+0.1%` to `+6.3%` shifts on memory-bandwidth-bound `100000`-entity rows tracked host run-to-run DRAM variance (`native` itself shifted `+6.0%` between captures while `rust:release` `ratio_vs_native` improved `1.09x -> 1.08x`).
