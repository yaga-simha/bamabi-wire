# `bamabi_guest_sys`

Low-level, **zero-Bevy-dependency** guest FFI runtime for Rust `.cdylib` mods targeting `bamabi` (`ABI 0.3.0`, `v0.3.0`, Layer 0 in [`bamabi-wire`](../../README.md)).

Isolates all guest-side `unsafe` code (`#![deny(unsafe_op_in_unsafe_fn, clippy::undocumented_unsafe_blocks, clippy::missing_safety_doc)]`) so [`bamabi_guest`](../bamabi_guest/README.md) enforces `#![forbid(unsafe_code)]`:

- **`Host` & `ValidatedLoadCtx`**: Bounded `BamHostApi` copy, `ABI 0.3.0` SemVer verification, checked `covers_slot` + `UNSUPPORTED`-fallback wrappers for all **68/68** host slots (27 core ABI `0.2` slots + 41 extended/ABI-`0.3.0` slots), `"bam.v0.*"` custom symbol lookup, and `VTableHeader` checks.
- **`GuestInstance`**: Per-World heap state owner with deterministic LIFO closure teardown, `RuntimeAllocTracker`, init rollback, and hot-reload `save_state` / `restore_state` trampolines.
- **`panic` & `ReentrantCell`**: `extern "C"` panic guards (`catch_guest_result`, `catch_guest_bool`, `catch_guest_unit`) paired with lock-free `UnsafeCell` + `AtomicBool` re-entrancy guards so unwind payloads are dropped strictly on the guest heap.
- **Validated Iterators, Commands & Migrations**: Layout-, alignment-, UUID-, and length-checked wrappers for dense columns, sparse rows, zero-alloc `512 B` `align(16)` `RawMsgStackScratch` message/removed batch & drain streaming (`DrainOptions`, `read_messages_into*`, `drain_messages_into_vec_checked`, `drain_messages_count_checked`), `iter_count`, `iter_up_cascade`, `par_for_each_chunk_readonly`, `GuestRes` change ticks, deferred commands & checkpoints (`cmd_checkpoint` / `cmd_rollback`, `cmd_insert_relation` / `cmd_remove_relation`), panic-guarded `migrate_component_column` (`BamRowMigrateFn`), dynamic component drop/hook trampolines, and observers.
- **Unconditional Bevy `0.20` Wire Projections (`wire_types`)**: Zero-Bevy `#[repr(C)]` / `#[repr(transparent)]` `AbiPod` wire mirrors with compile-time zero-padding checks and `FIELD_SCHEMA_HASH` digests matching `bamabi_bridges` `ModComponent` types (including all 194 named Bevy `0.20` `FfiKeyCode` indices `0..=193`, `FfiUiRect` / elliptical `FfiBorderRadius` helpers, picking, camera/light, audio, UI/Feathers, `ModStateSlab`, and 64-byte `"BAMT"` `SystemTelemetryDigest64`).

Mod authors should depend on [`bamabi_guest`](../bamabi_guest/README.md) rather than calling this `-sys` crate directly.

## Documentation

- **Crate Architecture & Internals:** [`ARCHITECTURE.md`](ARCHITECTURE.md)
- **Test & Formal Verification Catalog:** [`TESTS.md`](TESTS.md)
- **Workspace Docs:** [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md) | [`../../DEVELOPMENT.md`](../../DEVELOPMENT.md) | [`../../TESTING.md`](../../TESTING.md)

## Setup & Building

Requires Rust `>= 1.87` (Edition `2024`). Standalone usage:

```toml
[dependencies]
bamabi_guest_sys = { git = "https://github.com/yaga-simha/bamabi-wire.git", branch = "main" }
```

Build and verify zero Bevy dependencies from the workspace root:

```bash
cargo check -p bamabi_guest_sys --all-targets --all-features
cargo build -p bamabi_guest_sys --all-features
cargo tree -p bamabi_guest_sys | sed 's/ (.*//' | grep -v bamabi | grep -i bevy   # MUST print nothing
```

## Testing & Formal Verification

```bash
cargo fmt -p bamabi_guest_sys -- --check
cargo clippy -p bamabi_guest_sys --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_guest_sys --no-deps --all-features
cargo test -p bamabi_guest_sys --all-features
cargo test --doc -p bamabi_guest_sys --all-features

# Miri dual-model gate (Tree Borrows + Stacked Borrows, strict provenance)
MIRIFLAGS="-Zmiri-tree-borrows -Zmiri-strict-provenance" cargo +nightly miri test -p bamabi_guest_sys
MIRIFLAGS="-Zmiri-strict-provenance" cargo +nightly miri test -p bamabi_guest_sys --test soundness_regressions

# Kani formal verification (PO-G1, PO-G3..PO-G10; see TESTS.md for the CI two-step memory split)
cargo kani -p bamabi_guest_sys
```

## Benchmarks

As a Layer 0 FFI boundary crate, `bamabi_guest_sys` has **no Criterion benchmarks**; end-to-end ECS dispatch and guest mod benchmarks live in `bamabi-showcase`. For zero-allocation runtime profiling across the FFI boundary, `wire_types::lifecycle` provides the fixed 64-byte `"BAMT"` [`SystemTelemetryDigest64`](src/wire_types/lifecycle.rs) wire frame.

## License

Dual-licensed under `MIT OR Apache-2.0` ([`LICENSE-MIT`](../../LICENSE-MIT) and [`LICENSE-APACHE`](../../LICENSE-APACHE)).
