# Testing, Formal Verification & Benchmarking

`bamabi-wire` uses a multi-tier verification pyramid driven locally and in [`.github/workflows/ci.yml`](.github/workflows/ci.yml) / [`.github/workflows/nightly.yml`](.github/workflows/nightly.yml) via `cargo xtask`.

## 1. Quick Commands

```bash
# Standard CI gate (fmt, header parity checks, lint, Rust tests, C99 + ASan/UBSan)
cargo xtask ci

# Full gate (adds audit, dual-model Miri, and Kani formal verification)
cargo xtask ci --full

# Fast iteration on tests only
cargo xtask test
```

## 2. Verification Pyramid

### Tier 1: Formatting, Isolation & Static Lints (`cargo xtask fmt`, `cargo xtask lint`)

- `cargo xtask fmt`: runs `cargo fmt --all -- --check` (`--fix` to format).
- `cargo xtask lint`:
  1. Verifies `#![forbid(unsafe_code)]` is present in `crates/bamabi_guest/src/lib.rs`, `crates/bamabi_macros/src/lib.rs`, `crates/bamabi_sdk_gen/src/lib.rs`, and `xtask/src/main.rs`.
  2. Verifies zero non-`bamabi` Bevy crates in `cargo tree --workspace`.
  3. Runs `cargo clippy --workspace --all-targets -- -D warnings` (both default features and `--all-features`), enforcing workspace lints in [`Cargo.toml`](Cargo.toml) (`unsafe_op_in_unsafe_fn = "deny"`, `improper_ctypes_definitions = "deny"`, `undocumented_unsafe_blocks = "warn"`, `cast_ptr_alignment = "warn"`, etc.) and [`clippy.toml`](clippy.toml).
  4. Runs `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` (default features and `--all-features`).

### Tier 2: Rust Unit, Integration & Doc Tests (`cargo xtask test`)

Running `cargo xtask test` with no filter flags executes:
1. `cargo test --workspace --no-default-features`
2. `cargo test --workspace --all-features`
3. `cargo test --doc --all-features`

Targeted filters:
- `cargo xtask test --no-default-features` / `--all-features` / `--doc`
- `cargo test -p xtask` — runs the 6 `xtask` CLI parser and generated-component-header parity unit tests (see [`xtask/TESTS.md`](xtask/TESTS.md)).

### Tier 3: `trybuild` UI Compile-Fail & Positive Controls (`bamabi_macros`)

[`crates/bamabi_macros/tests/compile_fail_tests.rs`](crates/bamabi_macros/tests/compile_fail_tests.rs) drives 34 `trybuild` fixtures under `crates/bamabi_macros/tests/ui/*.rs` (2 positive controls + 32 compile-fail diagnostics) proving that `#[derive(AbiPod)]`, `#[derive(ModComponent)]`, and `#[derive(GuestComponent)]` reject `bool`, `u128`, `usize`, `Drop` types, `#[repr(Rust)]`, `#[repr(C, packed)]`, implicit interior/trailing padding, malformed UUIDs, spoofed `String`/`Vec` types, and invalid `sparse_set` + `summary_tick` combinations at compile time.

### Tier 4: Property-Based Tests & Nightly `-Zrandomize-layout`

Three dedicated `proptest` suites validate algebraic, hashing, aliasing, and witness round-trip invariants:
- `crates/bamabi_sys/tests/proptest_wire.rs` (`siphash24_128`, `ConstSipHasher128`, `ComponentUuid::try_parse_hex`)
- `crates/bamabi_guest_sys/tests/proptest_guest_sys.rs` (`ChunkColumnDispenser`, `ValidatedIter::read_messages_batch_checked`, `SparseRows`, `SparseRowsMut`)
- `crates/bamabi_macros/tests/proptest_witness.rs` (`export_fields` -> `import_field` schema-evolution witness round-trips)

Run directly or scale case counts:

```bash
cargo xtask test --proptest --cases 256
```

Under nightly Rust ([`.github/workflows/nightly.yml`](.github/workflows/nightly.yml)), `cargo xtask nightly --randomize-layout` (default `--cases 2048`) runs `bamabi_sys`, `bamabi_guest_sys`, `bamabi_guest` (`--all-features`), `bamabi_macros` (`--lib --test mod_component_vtable`), and `bamabi_sdk_gen` with `RUSTFLAGS="-Zrandomize-layout"` to prove no wire type relies on `#[repr(Rust)]` field ordering.

### Tier 5: Polyglot Header Parity & Compiler Verification (`bamabi_sdk_gen` & C99)

- `cargo xtask header --check` & `cargo xtask test --header-parity` (`crates/bamabi_sdk_gen/tests/header_parity.rs`): verify byte-for-byte parity between `bamabi_sdk_gen::render_c99_header()` and [`include/bamabi.h`](include/bamabi.h).
- `cargo xtask gen-component-headers --check` & `cargo test -p xtask`: verify byte-for-byte parity between `ModComponent::C_DECL` and [`include/generated/testbed_components.h`](include/generated/testbed_components.h).
- `crates/bamabi_sdk_gen/tests/sdk_compile_tests.rs`: exports `bamabi.h`, `bamabi.hpp`, and `bamabi.zig` and compiles/runs strict ISO C++17 (`-std=c++17 -Wall -Wextra -Wpedantic -Werror`) and Zig (`b.addTranslateC`) harnesses when `c++` / `zig` are installed.
- `cargo xtask c-test`: runs strict `-std=c99 -Wall -Wextra -Werror -pedantic -fsyntax-only` on `include/bamabi.h` and `include/generated/testbed_components.h`, compiles and runs [`tests/standalone_c_test.c`](tests/standalone_c_test.c), and repeats under `-fsanitize=address,undefined -fno-sanitize-recover=all`.

### Tier 6: Miri Dynamic Undefined-Behavior Verification (`cargo xtask miri`)

Validates pointer provenance, `UnsafeCell` / `ReentrantCell` soundness, padding-byte safety, sparse row pointers, and generated VTable callbacks across `bamabi_sys`, `bamabi_guest_sys`, `bamabi_guest` (`--all-features`), and `bamabi_macros --test mod_component_vtable`:

```bash
cargo xtask miri                    # Both Tree Borrows and Stacked Borrows (default)
cargo xtask miri --tree-borrows     # -Zmiri-tree-borrows -Zmiri-strict-provenance
cargo xtask miri --stacked-borrows  # -Zmiri-strict-provenance
cargo xtask miri --s390x            # Big-endian s390x-unknown-linux-gnu (also in nightly.yml)
```

### Tier 7: Kani Formal Verification (`cargo xtask kani`)

CI pins `kani-version: 0.68.0` and verifies bounded model-checking proof obligations across the two `unsafe`-bearing crates (`--harness-timeout 15m`):

- **`bamabi_sys` (`PO-W1..PO-W8` in [`crates/bamabi_sys/src/kani_proofs.rs`](crates/bamabi_sys/src/kani_proofs.rs)):** ABI version pack/unpack bijection & compatibility laws, raw `BamHostApi` header reads, `covers_slot` overflow safety, `offsets` & `FfiResult` totality, `FfiSlice`/`StrView` soundness, `FfiOption` zeroed `None` round-trips, `BamChangeTicks` wraparound safety, and `ComponentUuid` bijection/hex parsing.
- **`bamabi_guest_sys` (`PO-G1..PO-G9` in [`crates/bamabi_guest_sys/src/kani_proofs.rs`](crates/bamabi_guest_sys/src/kani_proofs.rs) and [`crates/bamabi_guest_sys/src/kani_bounds_proofs.rs`](crates/bamabi_guest_sys/src/kani_bounds_proofs.rs)):** 68-slot `Host` bounds & `ValidatedLoadCtx` guards, dense slice & mutable pointer soundness, `ChunkColumnDispenser` borrow disjointness & capacity limits, `SparseRows`/`SparseRowsMut` validation, `iter_mark_changed_range` & `SystemInvocation` clamping, trampoline null/alignment/ZST/command/observer/par-chunk/hook/witness guards, and hostile-host bounds (`read_messages_batch`, `spawn_batch`, `GuestInstance::init` rollback, `ValidatedSystemRef`, `insert_moved`, `spawn_moved`, `iter_entities` overlap guards).

```bash
cargo xtask kani                                    # All proofs in bamabi_sys + bamabi_guest_sys
cargo xtask kani -p bamabi_sys --harness po_w1      # Filtered local run
```

### Tier 8: Cross-Target, Spelling, License & SemVer Audit (`cargo xtask audit`)

- Cross-target `#![no_std]` checks: `cargo check -p bamabi_sys --target i686-unknown-linux-gnu` and `wasm32-unknown-unknown` (`--skip-targets` to skip).
- Spelling: `typos` using [`.typos.toml`](.typos.toml) (`--skip-typos` to skip).
- Supply-chain & zero-Bevy policy: `cargo deny --all-features check advisories licenses bans sources` using [`deny.toml`](deny.toml).
- Optional SemVer check: `cargo xtask audit --baseline-rev <rev>` runs `cargo semver-checks check-release -p bamabi_sys --baseline-rev <rev>` (matching `[package.metadata.cargo-semver-checks.lints]` in `crates/bamabi_sys/Cargo.toml`).

## 3. Benchmarking, Stress & Telemetry Notes

`bamabi-wire` is the **Layer 0** wire contract and guest SDK repository and contains **no Criterion `[[bench]]` targets** (neither [`Cargo.toml`](Cargo.toml) nor any member crate defines `[[bench]]`). End-to-end runtime ECS, FFI dispatch, and hot-reload benchmarks live downstream in **Layer 4** (`bamabi-showcase`, `docs/BENCHMARKS.md`).

Tooling in `bamabi-wire` that measures stress behavior, telemetry framing, and build performance:

- **Command-queue & batch-spawn stress tests:** [`crates/bamabi_guest/tests/command_queue_stress.rs`](crates/bamabi_guest/tests/command_queue_stress.rs) exercises handle-based `GuestCommands`, uninitialized alignment padding under Miri, ZST components, the `<= 16`-component stack fast path vs. the `> 16`-component heap fallback (20 components) in batched `spawn`, and invalid-handle error rejections:
  ```bash
  cargo test -p bamabi_guest --test command_queue_stress --all-features
  ```
- **Wire telemetry frame (`SystemTelemetryDigest64` / `"BAMT"`):** `bamabi_guest_sys::wire_types` and `bamabi_guest::bridges::reflect` define the 64-byte little-endian `SystemTelemetryDigest64` (`"BAMT"`) wire projection consumed by downstream `bamabi_reflect::ModTelemetryPlugin`, verified in [`crates/bamabi_guest/tests/bevy_020_parity_tests.rs`](crates/bamabi_guest/tests/bevy_020_parity_tests.rs).
- **Compile-time & proc-macro profiling:** Measure workspace build and macro expansion timings locally with Cargo's built-in HTML timing report:
  ```bash
  cargo check --workspace --all-features --timings
  ```
- **Differential & property-test scaling:** Increase `PROPTEST_CASES` to stress-test hashing, column dispensers, and witness serialization:
  ```bash
  cargo xtask test --proptest --cases 4096
  cargo xtask nightly --randomize-layout --cases 4096
  ```

## 4. Per-Crate Test Catalogs

- [`crates/bamabi_sys/TESTS.md`](crates/bamabi_sys/TESTS.md) — layout assertions, ABI constants, `proptest_wire`, and `PO-W1..PO-W8` Kani proofs.
- [`crates/bamabi_macros/TESTS.md`](crates/bamabi_macros/TESTS.md) — macro unit tests, `mod_component_vtable`, 34 `trybuild` UI scenarios, and `proptest_witness`.
- [`crates/bamabi_sdk_gen/TESTS.md`](crates/bamabi_sdk_gen/TESTS.md) — `header_parity`, `sdk_compile_tests` (C++17 & Zig), and unit tests.
- [`crates/bamabi_guest_sys/TESTS.md`](crates/bamabi_guest_sys/TESTS.md) — `host_and_instance_tests`, `iter_tests`, `soundness_regressions`, `proptest_guest_sys`, and `PO-G1..PO-G9` Kani proofs.
- [`crates/bamabi_guest/TESTS.md`](crates/bamabi_guest/TESTS.md) — integration, Bevy `0.20` parity, `command_queue_stress`, slice/pointer edge, split-borrow/lifecycle, derive/`run_if`, and `v0.3` SDK tests.
- [`xtask/TESTS.md`](xtask/TESTS.md) — CLI parsing, flag validation, and checked-in component-header parity tests.
