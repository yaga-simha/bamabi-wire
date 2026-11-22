# `bamabi_sdk_gen` Architecture & Implementation Guide

`bamabi_sdk_gen` (`v0.3.0`, ABI `0x0003_0000`) is the deterministic polyglot SDK and component header generator in Layer 0 ([`bamabi-wire`](../../README.md)). It consumes the Rust wire contract from [`bamabi_sys`](../bamabi_sys/README.md) and produces:

1. **`include/bamabi.h`** — the canonical ISO C99 ABI wire header (`header_gen.rs`).
2. **`bamabi.hpp`** — ISO C++17 RAII + typed view wrapper header (`sdk.rs`).
3. **`bamabi.zig`** — Zig `0.16` `b.addTranslateC` (`"bam_c"`) bindings (`sdk.rs`).
4. **`include/generated/*.h`** — C99/C++17 component declaration headers from `ModComponent::C_DECL` (`component_header.rs`).

---

## 1. Crate Boundaries & Design Invariants

- **Safety (`#![forbid(unsafe_code)]`):** `src/lib.rs` enforces `#![forbid(unsafe_code)]` and `#![warn(missing_docs)]`. The generator performs zero `unsafe` operations; it emits strings and verifies Rust layouts via safe `core::mem::{size_of, align_of, offset_of!}`.
- **Zero Bevy Dependencies & Zero Subprocesses:** `Cargo.toml` depends strictly on `bamabi_sys`. Library code never invokes C/C++/Zig compilers or external processes; compiler tests live strictly in `tests/sdk_compile_tests.rs` and `xtask`.
- **Deterministic Byte-Identical Output:** Generated files contain zero timestamps, host paths, PIDs, or randomized map iterations. Repeated invocations of `render_c99_header()` and `render_component_header()` produce identical byte streams, allowing `--check` CI gates (`header_parity`, `cargo xtask header --check`, `cargo xtask gen-component-headers --check`) to enforce exact byte parity.
- **Single Source of Truth:** All ABI constants, bitflags, `FfiResult` codes, `MAX_*` limits, `SipHash` keys, and `BamHostApi` offsets (`bamabi_sys::offsets`) are imported from `bamabi_sys` rather than duplicated.

---

## 2. Module Map

| File | Public API | Responsibility |
|---|---|---|
| [`src/lib.rs`](src/lib.rs) | Re-exports `render_c99_header`, `write_or_check_header`, `CPP17_HEADER`, `ZIG_SDK`, `export_sdk`, `render_component_header`, `section_for` | Crate root lints (`#![forbid(unsafe_code)]`, `#![warn(missing_docs)]`), incremental learning docs, and doctests |
| [`src/header_gen.rs`](src/header_gen.rs) | `render_c99_header`, `write_or_check_header` (+ `#[cfg(test)] host_api_slot_names`) | Pre-emission Rust layout verification (`verify_host_rust_layouts`) and 6-section ISO C99 header emission |
| [`src/sdk.rs`](src/sdk.rs) | `c99_header`, `CPP17_HEADER`, `ZIG_SDK`, `export_sdk` | Bundles C99 + C++17 + Zig SDK files for `cargo bam export-sdk` (`bamabi-tools`) and polyglot builds |
| [`src/component_header.rs`](src/component_header.rs) | `render_component_header`, `section_for` | Wraps `#[derive(ModComponent)]` `C_DECL` strings into standalone C99/C++17 headers |

---

## 3. How `include/bamabi.h` Is Synthesized (`header_gen.rs`)

### 3.1 Layout Specification Table (`wire_struct_specs`) & Pre-Emission Guard (`verify_host_rust_layouts`)

Before formatting any C99 text, `render_c99_header()` builds `wire_struct_specs() -> Vec<StructLayoutSpec>`, a declarative table covering all **20 ABI `0.3.0` wire structs**. Each `StructLayoutSpec` records:

- `c_name` and `assert_stem` (for `BAM_STATIC_ASSERT` symbol naming).
- Frozen 64-bit layout (`size_64`, `align_64`) and 32-bit layout (`size_32`, `align_32: Align32`).
- Live host compiler values (`rust_host_size`, `rust_host_align`) via `size_of::<T>()` / `align_of::<T>()`.
- Per-field `FieldLayoutSpec` slice pairing `c_name`, `offset_64`, `offset_32`, and `rust_host_offset` (`offset_of!`, or `bamabi_sys::offsets::*` for `BamHostApi`).

`verify_host_rust_layouts(&specs)` then validates:

1. **Primitive newtype invariants:** `FfiBool` (1 B), 32-bit handles/enums (`BamComponent`, `BamSystem`, `BamSystemSet`, `BamSymbol`, `BamFieldKind`, `FfiResult`, `AccessMode`, `ScheduleId`, `BamEventKind`, `FfiAssetLoadState` = 4 B), 64-bit handles (`FfiEntity`, `FfiChildOf` = 8 B), and `FfiOption<u32>` (8 B, `is_some` at 0, `value` at 4).
2. **All 20 wire structs on the active host (`usize::BITS` = 64 or 32):** Checks `rust_host_size`, `rust_host_align`, and every field's `rust_host_offset`. Any mismatch aborts header generation with an explicit diagnostic `Err(String)`.
3. **73-slot `BamHostApi` count check:** Counts non-header fields in `BamHostApi` (excluding `struct_size` and `abi_version`) and asserts equality with `bamabi_sys::BAM_HOST_API_SLOT_COUNT` (`73`).

### 3.2 All 20 Wire Structs Covered

| C99 Struct (`c_name`) | Rust Type (`bamabi_sys`) | 64-bit Size / Align | 32-bit Size / Align | Extensibility Floor (`MIN_V0_2` / `MIN_V0_3`) |
|---|---|---|---|---|
| `BamComponentUuid` | `ComponentUuid` | 16 / 1 | 16 / `Fixed(1)` | — |
| `BamStrView` | `StrView` | 16 / 8 | 8 / `Fixed(4)` | — |
| `BamManifest` | `BamManifest` | 48 / 8 | 32 / `Fixed(4)` | 32 / 24 (`v0.2`) |
| `BamAssetHandle` | `FfiAssetHandle` | 32 / 4 | 32 / `Fixed(4)` | — |
| `BamTermDesc` | `BamTermDesc` | 12 / 4 | 12 / `Fixed(4)` | — |
| `BamQueryDesc` | `BamQueryDesc` | 32 / 8 | 24 / `Fixed(4)` | 24 / 16 (`v0.2`) |
| `BamSystemDesc` | `BamSystemDesc` | 192 / 8 | 104 / `Fixed(4)` | 104 / 60 (`v0.2`) |
| `BamObserverDesc` | `BamObserverDesc` | 88 / 8 | 64 / `Fixed(4)` | 48 / 36 (`v0.2`) |
| `BamTriggerDesc` | `BamTriggerDesc` | 64 / 8 | 48 / `Fixed(4)` | 64 / 48 (`v0.2`) |
| `BamChangeTicks` | `BamChangeTicks` | 16 / 4 | 16 / `Fixed(4)` | — |
| `BamFieldDesc` | `BamFieldDesc` | 64 / 8 | 36 / `Fixed(4)` | 64 / 36 (`v0.3`) |
| `BamRequiredComponent` | `BamRequiredComponent` | 64 / 8 | 48 / `Fixed(4)` | 64 / 48 (`v0.3`) |
| `BamRelationDesc` | `BamRelationDesc` | 56 / 8 | 44 / `Fixed(4)` | 56 / 44 (`v0.3`) |
| `BamLayoutInfo` | `BamLayoutInfo` | 64 / 8 | 48 / `U64` | 64 / 48 (`v0.3`) |
| `BamScheduleSetDesc` | `BamScheduleSetDesc` | 48 / 8 | 36 / `Fixed(4)` | 48 / 36 (`v0.3`) |
| `BamComponentDesc` | `ComponentDesc` | 176 / 8 | 104 / `U64` | 64 / 44 (`v0.2`), 176 / 104 (`v0.3`) |
| `BamColumnInfo` | `BamColumnInfo` | 48 / 8 | 40 / `Fixed(4)` | — |
| `BamVTableHeader` | `VTableHeader` | 40 / 8 | 32 / `Fixed(4)` | 40 / 32 (`v0.2`) |
| `BamMappedStringVTable` | `MappedStringVTable` | 56 / 8 | 40 / `Fixed(4)` | — |
| `BamHostApi` | `BamHostApi` | 592 / 8 | 300 / `Fixed(4)` | 224 / 116 (`v0.2`), 552 / 280 (`v0.3` floor) |

### 3.3 Six Ordered C99 Sections

`render_c99_header()` emits six sections wrapped in `#ifndef BAMABI_H` / `#define BAMABI_H` and `extern "C"` guards:

1. **Compiler Portability, Export & Static Layout Assertion Macros:**
   - `BAM_EXPORT`: `__declspec(dllexport)` on Windows/Cygwin, `__attribute__((visibility("default")))` on GCC/Clang, and `extern "C"` prefix under `__cplusplus`.
   - `BAM_UNUSED`: `__attribute__((unused))` on GCC/Clang for `static inline` helpers.
   - `BAM_RESTRICT`: `__restrict` on `_MSC_VER`, `restrict` in C99+ (`!defined(__cplusplus) && __STDC_VERSION__ >= 199901L`), and empty otherwise (since ISO C++ has no standard `restrict` keyword).
   - `BAM_ALIGNOF(type)`: uses C++11 `alignof`, C11 `_Alignof`, GCC/Clang `__extension__ __alignof__`, MSVC `__alignof`, or an ISO C99 `offsetof(struct { char _bam_pad; type _bam_val; }, _bam_val)` fallback.
   - `BAM_STATIC_ASSERT(cond, name)`: uses C++11 `static_assert`, C11 `_Static_assert`, GCC/Clang `__extension__ _Static_assert`, or a `typedef char bam_static_assert_##name[(cond) ? 1 : -1]` fallback.
2. **ABI SemVer Version, Architectural Limits & Domain Constants:**
   - `BAM_ABI_VERSION` (`0x00030000u`), `BAM_PACK_ABI_VERSION`, `BAM_MAX_TERMS_PER_QUERY`, `BAM_MAX_QUERIES_PER_SYSTEM` (`16u`), `BAM_HOST_API_SLOT_COUNT` (`73u`), `BAM_SIPHASH_KEY_0`/`1`, `BAM_MAX_CHANGE_AGE`, and canonical names `BAM_CHILD_OF_NAME`, `BAM_CHILDREN_NAME`, `BAM_DISABLED_NAME`.
3. **Primitive Newtypes, Handles, Status Codes, Modes & Flags:**
   - `BamBool`, `BamComponent`, `BamSystem`, `BamSystemSet`, `BamSymbol`, `BamEntity`, `BamResult` (18 status codes including `BAM_RESULT_CYCLE_DETECTED`), `BamAccessMode`, `BamEventKind`, `BamFieldKind`, `BAM_LOG_*`, `BAM_ASSET_HANDLE_KIND_*`, `BamAssetLoadState`, `BamScheduleId`, and all `BAM_*_FLAGS` bitmasks.
4. **Opaque Context Handles, Wire Descriptors & Function Pointer Typedefs:**
   - Opaque structs (`BamHostCtx`, `BamLoadCtx`, `BamSystemCtx`, `BamIter`, `BamCommands`, `BamInstance`), wire descriptors (`BamStrView`, `BamByteSlice`, `BamComponentUuid`, `BAM_OPTION_STRUCT`, `BamAssetHandle`, `BamChildOf`, `BamChangeTicks`, `BamManifest`, `BamTermDesc`, `BamQueryDesc`, `BamSystemDesc`, `BamObserverDesc`, `BamTriggerDesc`, `BamFieldDesc`, `BamRequiredComponent`, `BamRelationDesc`, `BamLayoutInfo`, `BamScheduleSetDesc`, `BamComponentDesc`, `BamColumnInfo`, `BamVTableHeader`, `BAM_MAPPED_WIRE_VTABLE_STRUCT`, `BamMappedStringVTable`), guest entry-point typedefs (`BamManifestFn`, `BamInitFn`, `BamShutdownFn`), all 73 slot typedefs, `struct BamHostApi`, `BAM_*_MIN_V0_*_SIZE` macros, and all 73 `BAM_HOST_API_OFFSET_*` macros (`offsetof(BamHostApi, <slot>)`).
   - Under C++14+ (`__cplusplus >= 201402L` or `_MSVC_LANG >= 201402L`), extensible tail fields on `BamManifest`, `BamSystemDesc`, `BamObserverDesc`, and `BamComponentDesc` receive safe default member initializers (`= nullptr` / `= 0`).
5. **ISO C99 Inline Helpers (`bam_*`):**
   - Version helpers (`bam_pack_abi_version`, `bam_version_major`/`minor`/`patch`, `bam_version_is_compatible`).
   - Diagnostics & views (`bam_result_name`, `bam_str_view_len`, `bam_str_view`, `bam_entity_is_null`/`valid`, `bam_component_is_valid`, `bam_asset_load_state_is_*`, `bam_event_kind_is_lifecycle`, `bam_schedule_is_valid`, `bam_field_kind_is_valid`).
   - Descriptor builders (`bam_term_desc`, `bam_term_desc_or`, `bam_asset_handle_*`, `bam_desc_component_with_layout`, `bam_observer_desc`, `bam_trigger_desc`).
   - Host API prefix safety (`bam_host_api_raw_struct_size`, `bam_host_api_covers_slot`, `bam_host_api_copy_bounded`).
   - Change-tick wraparound arithmetic (`bam_ticks_is_newer`, `bam_ticks_is_added`, `bam_ticks_is_changed`).
   - Pure C99 `SipHash-2-4-128` (`BamSipHasher128`, `bam_siphash128_*`, `bam_uuid_from_parts`, `bam_uuid_from_canonical_name`, `bam_disabled_uuid`, `bam_uuid_equals`), emitting byte-for-byte identical 16-byte digests to `bamabi_sys::ComponentUuid::from_parts`.
6. **Auto-Generated Layout Assertions:**
   - Unconditional primitive `sizeof` / `BAM_ALIGNOF` checks.
   - `#if UINTPTR_MAX == 0xffffffffffffffffULL` (64-bit) and `#elif UINTPTR_MAX == 0xffffffffUL` (32-bit) blocks checking `sizeof`, `BAM_ALIGNOF`, `offsetof` for every field of all 20 structs (including all 73 `BamHostApi` slots), and all `BAM_*_MIN_V0_*_SIZE` bounds.

### 3.4 `BamHostApi` 73-Slot Layout & 32/64-Bit Parity (`Align32`)

`BamHostApi` begins with `uint32_t struct_size` (offset `0`) and `uint32_t abi_version` (offset `4`), followed by **73 function-pointer slots**:

- **Slots 0..=26 (27 slots, ABI 0.2 baseline):** `get_proc_address` through `cmd_remove`. Tail ends at `BAM_HOST_API_MIN_V0_2_SIZE` (`224` B on 64-bit, `116` B on 32-bit).
- **Slots 27..=49 (23 slots, ABI 0.2.x extensions):** `par_for_each_chunk` through `cmd_spawn_scene`.
- **Slots 50..=72 (23 slots, ABI 0.3.0 extensions):** `log_diagnostic` through `migrate_component_column` (ending at `BAM_HOST_API_MIN_V0_3_SIZE`, `552` B / `280` B), plus tail slots `iter_next_chunk_batch`, `iter_column_ticks`, `cmd_clone_entity`, `cmd_insert_resource`, `cmd_remove_resource` (`592` B / `300` B).

For 32-bit targets, `Align32` handles a critical cross-platform subtlety: `i686-unknown-linux-gnu` aligns `uint64_t` to **4 bytes**, whereas 32-bit ARM and `wasm32` align `uint64_t` to **8 bytes**. Structs containing `uint64_t` fields (`BamLayoutInfo` and `BamComponentDesc`) therefore specify `Align32::U64`, causing `header_gen.rs` to verify against `align_of::<u64>()` in Rust and emit `BAM_ALIGNOF(uint64_t)` in C99 rather than hardcoding `4` or `8`.

### 3.5 `write_or_check_header`

`write_or_check_header(root, check_only)` backs `cargo xtask header` and `tests/header_parity.rs`:

- In `check_only = true` mode, reads `<root>/include/bamabi.h`, normalizes `\r\n` to `\n`, and compares byte-for-byte against `render_c99_header()`.
- In write mode (`check_only = false`), creates `<root>/include` if missing and writes the file only when on-disk contents differ, avoiding unnecessary mtime churn for downstream C/C++ build systems.

---

## 4. Polyglot SDK Bundle (`sdk.rs`)

`sdk::export_sdk(out_dir)` writes three synchronized files to `out_dir`: `bamabi.h`, `bamabi.hpp`, and `bamabi.zig`.

### 4.1 C++17 Header (`CPP17_HEADER` -> `bamabi.hpp`)

- **Namespace `bam::offsets`:** Programmatically mirrors `BAM_HOST_API_SLOT_COUNT` and every `BAM_HOST_API_OFFSET_*` as `inline constexpr std::size_t` constants, guarded by `static_assert` checks on `STRUCT_SIZE == 0u`, `ABI_VERSION == 4u`, `GET_PROC_ADDRESS == 8u`, and `CMD_REMOVE_RESOURCE + sizeof(BamCmdRemoveResourceFn) == sizeof(BamHostApi)`.
- **Aliasing & Boundary Helpers:**
  - `bam::copy_column_restricted<T>(T* BAM_RESTRICT dst, const T* BAM_RESTRICT src, std::size_t count)` leverages `BAM_RESTRICT` (`__restrict` on MSVC) for non-aliasing SoA column loops.
  - `bam::guard_boundary(fn)` wraps guest callbacks in `try { ... } catch (const std::exception&) { return BAM_RESULT_PANIC_CAUGHT; } catch (...) { return BAM_RESULT_PANIC_CAUGHT; }` so C++ exceptions never unwind across `extern "C"`.
  - `bam::host_api_covers_slot` / `bam::is_ok` wrap the C prefix-coverage and status checks.
- **73/73 Slot View Wrappers:** `HostContextView`, `LoadContextView`, `ChunkView`, `CommandsView`, and `SystemContextView` expose zero-overhead, `noexcept` typed wrappers over all 73 slots, checking `bam_host_api_covers_slot` before calling any slot beyond the `v0.2` floor (`BAM_RESULT_UNSUPPORTED` if absent).
- **RAII `ScopedTransaction`:** Calls `cmds.checkpoint(token_)` in its constructor, marks itself `active_` on `BAM_RESULT_OK`, is non-copyable and move-constructible (clearing `other.active_`), and automatically calls `cmds_.rollback(token_)` in `~ScopedTransaction()` unless `commit()` or `rollback_now()` was called.

### 4.2 Zig `0.16` Module (`ZIG_SDK` -> `bamabi.zig`)

- **`b.addTranslateC` `"bam_c"` Integration:** Uses `pub const c = @import("bam_c");` rather than legacy `@cImport`/`@cInclude`, matching Zig `0.16` build-graph module conventions.
- **Offsets & `comptime` Verification:** Programmatically exposes `offsets.*` (`@offsetOf(c.BamHostApi, ...)` for header fields and all 73 slots) plus top-level `BAM_HOST_API_OFFSET_*` aliases, verified by a `comptime` block asserting `STRUCT_SIZE == 0`, `ABI_VERSION == 4`, `GET_PROC_ADDRESS == 8`, and `CMD_REMOVE_RESOURCE + @sizeOf(usize) == @sizeOf(c.BamHostApi)`.
- **Aliasing (`noalias`):** Defines `pub const BAM_RESTRICT_QUALIFIER = "noalias";` and `copyColumnNoAlias(comptime T: type, noalias dst: [*]T, noalias src: [*]const T, len: usize)`.
- **Lossless 18-Code `BamError`:** Maps every `BamResult` code (`BAM_RESULT_OK` -> void, 17 error codes -> distinct `BamError` variants, plus `OutOfMemory`) via `checkResult(res) BamError!void` and `mapError(err) c.BamResult` (note: `error.OutOfMemory` and `error.InternalError` both map back to `BAM_RESULT_INTERNAL_ERROR`).
- **73/73 Slot Views & `ScopedTransaction`:** `HostContextView`, `LoadContextView`, `ChunkView`, `CommandsView`, and `SystemContextView` wrap all 73 slots using `coversHostApiSlot` and `checkResult`, alongside `ScopedTransaction` for `defer tx.deinit()` auto-rollback.

---

## 5. Component Header Synthesis (`component_header.rs` & `testbed_components.h`)

Shared host/guest ECS components derive `ModComponent` from `bamabi_macros`, which computes a compile-time `C_DECL` string containing the C99 `typedef struct`, `BAM_STATIC_ASSERT` size/alignment/field-offset guards, and VTable metadata when applicable.

1. `section_for::<T: ModComponent>()` returns `(T::CANONICAL_NAME, T::IS_ABI_POD, T::C_DECL)` as a `const fn`.
2. `render_component_header(guard_macro, abi_header_include, sections)` emits:
   - Auto-generated warning banner.
   - `#ifndef <guard_macro>` / `#define <guard_macro>`.
   - `#include "<abi_header_include>"` (e.g., `"../bamabi.h"` for `include/generated/testbed_components.h`).
   - `extern "C"` wrapper around every ``/* Component: `<name>` (IS_ABI_POD = <bool>) */`` block and its `c_decl`.
3. In `bamabi-wire`, [`xtask/src/component_headers.rs`](../../xtask/src/component_headers.rs) defines the 19 canonical testbed `ModComponent` structs and calls `render_component_header("BAMABI_GENERATED_TESTBED_COMPONENTS_H", "../bamabi.h", ...)` to generate [`include/generated/testbed_components.h`](../../include/generated/testbed_components.h).

---

## 6. Updating Headers When `bamabi_sys` Changes

Follow this workflow whenever a wire struct, flag, error code, or `BamHostApi` slot changes in `crates/bamabi_sys`:

1. **Update `wire_struct_specs()` and emitters in `src/header_gen.rs`:**
   - Update affected `StructLayoutSpec` / `FieldLayoutSpec` entries (both 64-bit and 32-bit offsets/sizes/alignments).
   - Add any new C typedefs, constants, `BAM_HOST_API_OFFSET_*` inputs, or inline helpers in `render_c99_header()`.
   - Update `verify_host_rust_layouts()` if primitive newtypes changed.
2. **Update `src/sdk.rs` if `BamHostApi` or `BamResult` changed:**
   - Mirror new slots in `CPP17_HEADER` (`bam::offsets::*` + view methods) and `ZIG_SDK` (`offsets.*` + `BAM_HOST_API_OFFSET_*` + view methods).
   - Update `BamError`, `checkResult`, and `mapError` in `ZIG_SDK` if `FfiResult` gained a new error code.
   - Update `tests/sdk_compile_tests.rs` and unit test assertions in `src/sdk.rs`.
3. **Regenerate checked-in headers & verify:**
   ```bash
   cargo xtask header
   cargo xtask gen-component-headers
   cargo test -p bamabi_sdk_gen --all-features
   cargo xtask header --check
   cargo xtask gen-component-headers --check
   cargo xtask c-test
   ```

---

## 7. Setup, Building, Running, Testing & Benchmarking

### Setup & Dependencies

- Rust toolchain pinned in [`rust-toolchain.toml`](../../rust-toolchain.toml) (`edition = "2024"`, MSRV `1.95` for `bamabi_sdk_gen`).
- Optional polyglot toolchains for integration tests:
  - C99 compiler (`cc` / `clang`) for `cargo xtask c-test` (and `clang` with ASan+UBSan).
  - C++17 compiler (`c++`) for `sdk_compile_tests::exported_cpp17_header_compiles_and_passes_strict_checks`.
  - Zig `0.16.0` (`zig`) for `sdk_compile_tests::exported_zig_sdk_compiles_and_passes_with_translate_c`.

### Building & Running Codegen

```bash
# Build the generator crate
cargo build -p bamabi_sdk_gen --all-features

# Regenerate or parity-check include/bamabi.h
cargo xtask header
cargo xtask header --check

# Regenerate or parity-check include/generated/testbed_components.h
cargo xtask gen-component-headers
cargo xtask gen-component-headers --check
```

Downstream tools and consumers invoke `bamabi_sdk_gen::export_sdk(out_dir)` (exposed via `cargo bam export-sdk` in `bamabi-tools`), then compile against the exported bundle:

```bash
# C99 guest / test consumer
cc -std=c99 -Wall -Wextra -Werror -pedantic -I<sdk_dir> guest.c -c

# C++17 guest / test consumer
c++ -std=c++17 -Wall -Wextra -Wpedantic -Werror -I<sdk_dir> guest.cpp -c

# Zig 0.16 consumer (inside a build.zig using b.addTranslateC on bamabi.h -> "bam_c")
zig build test
```

### Testing

```bash
cargo fmt -p bamabi_sdk_gen -- --check
cargo clippy -p bamabi_sdk_gen --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_sdk_gen --no-deps --all-features
cargo test -p bamabi_sdk_gen --all-features
cargo test --doc -p bamabi_sdk_gen --all-features
cargo xtask c-test
```

See [`TESTS.md`](TESTS.md) for the complete test catalog and invariants proved by each test.

### Benchmarking Notes

Layer 0 (`bamabi-wire`) contains **no Criterion benchmarks**; `bamabi_sdk_gen` performs compile-time/CLI string synthesis and layout validation rather than frame-loop execution. Runtime FFI dispatch, SoA query throughput, and host/guest call benchmarks live in Layer 4 (`bamabi-showcase`).
