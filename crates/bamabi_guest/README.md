# `bamabi_guest`

Safe (`#![forbid(unsafe_code)]`), **zero-Bevy-dependency** Rust SDK for authoring native `.cdylib` mods targeting `bamabi` (`ABI 0.3.0`, `v0.3.0`, Layer 0 in [`bamabi-wire`](../../README.md)).

## Documentation

- **Crate Architecture & Implementation:** [`ARCHITECTURE.md`](ARCHITECTURE.md) — module map, `bamabi_guest_sys` delegation, `QueryTuple` sealing, `GuestTransaction` RAII rollback, and guide to adding new bridges or builder methods.
- **Tests & Verification Catalog:** [`TESTS.md`](TESTS.md) — full breakdown of unit, integration, ABI `0.3.0`, Bevy `0.20` parity, stress, Miri, and `compile_fail` doctest proofs.
- **Workspace Guides:** [`../../ARCHITECTURE.md`](../../ARCHITECTURE.md) · [`../../DEVELOPMENT.md`](../../DEVELOPMENT.md) · [`../../TESTING.md`](../../TESTING.md)

## Features

- **`default = []`**: Core safe SDK with zero proc-macro dependencies (`MSRV 1.87`, `edition = "2024"`).
- **`derive`**: Re-exports `#[derive(AbiPod, GuestComponent, ModComponent)]` from [`bamabi_macros`](../bamabi_macros/README.md).

## Setup

Add `bamabi_guest` to a `cdylib` crate's `Cargo.toml`:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
bamabi_guest = { git = "https://github.com/yaga-simha/bamabi-wire.git", branch = "main", features = ["derive"] }
```

## Quickstart (Zero-Boilerplate Derive & Typed Queries)

```rust,no_run
use bamabi_guest::prelude::*;
use core::ffi::CStr;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, AbiPod, GuestComponent)]
#[guest_component(name = "example::Position")]
pub struct Position {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Default)]
pub struct MyGuestMod;

impl GuestPlugin for MyGuestMod {
    const NAME: &'static CStr = c"my_guest_mod";
    const VERSION: &'static CStr = c"0.3.0";

    fn build(&mut self, app: &mut GuestApp<'_>) -> FfiResult {
        app.system(c"nudge_positions")
            .query_typed::<&mut Position>()
            .schedule(ScheduleId::UPDATE)
            .build(|ctx| {
                ctx.single_query::<&mut Position>().for_each(|positions| {
                    for pos in positions {
                        pos.x += 1.0;
                    }
                })
            })
    }
}

export_mod!(plugin: MyGuestMod);
```

## Building & Running a `.cdylib` Mod

```bash
# Build your guest mod crate into a native shared library (.dylib / .so / .dll)
cargo build --release -p <your_mod_crate>

# Verify bamabi_guest directly under both feature sets
cargo check -p bamabi_guest --no-default-features
cargo check -p bamabi_guest --all-features
```

[`export_mod!`](src/macros.rs) exports `bamabi_manifest`, `bamabi_init`, `bamabi_shutdown`, and `bamabi_resolve_symbol` backed by a heap-allocated per-World `GuestInstance` (zero process-global mutable state). Load the compiled `.cdylib` in any `bamabi` host (`bamabi-core` / `bamabi`), via `cargo-bam` (`bamabi-tools`), or inspect runnable host+guest examples in Layer 4 (`bamabi-showcase`).

## Highlights

- **Zero-Boilerplate Component Derives:** `#[derive(AbiPod, GuestComponent)]` with `#[guest_component(name = "...", sparse_set, immutable, summary_tick)]` eliminates manual `impl GuestComponent` implementations (and `#[mod_component(name = "...", guest)]` offers a single-attribute shorthand when sharing a `ModComponent`).
- **Per-World Isolation (`export_mod!` & `GuestPlugin`):** Allocates an independent `GuestInstance` per Bevy `World` on the guest heap—zero process-global mutable state.
- **Full 18-Slot ABI `0.3.0` Coverage:** Safe wrappers for structured logging (`log_diagnostic`, `guest_log!`), fault reporting (`report_fault`), strict UUID resolution (`resolve_component_by_uuid`), layout/field inspection (`component_layout_info`, `verify_component_layout`), monotonic symbol interning (`intern_symbol`), persistent named objects (`bind_named_object`), non-destructive query counting (`ChunkIter::count`), ancestor cascades (`ChunkIter::up_cascade` / `TypedChunkIter::up_cascade`), zero-arena read-only parallel chunks (`par_for_each_chunk_readonly` / `par_chunks_readonly` with compile-time `Q::IS_READ_ONLY` enforcement), typed relationships (`RelationDescBuilder`, `register_relation`, `insert_relation`, `remove_relation`), decoupled scheduling (`SystemBuilder::register_handle`, `ScheduleSetBuilder`, `configure_schedule_set`, `mount_system`), RAII command transactions (`GuestTransaction` / `commands.transaction(...)`), and live schema migration (`migrate_component_column` / `migrate_pod_component_column`).
- **Unified `GuestApp::system` Builder & Expanded Typed Queries:** Declares up to 8 typed tuple queries (`query_typed` / `ctx.single_query`, `queries_2_*`, `queries_3_*`) with **1..=16 query terms** (`QueryTuple`) and tuple-generic **`Or<(T0..T7)>`** (2..=8 branches, preserving `Or2`/`Or3` aliases), `Allow<T>`, `Has<T>`, Bevy `0.20` entity disabling (`FfiDisabled`, `allow_disabled`, `with_disabled`, `without_disabled`, `has_disabled`), `RemovedComponents` cursors (`.removed::<T>()`), `TypedChunkIter::single_chunk` / `is_empty`, `get_many`, `get_dynamic` / `get_dynamic_mut`, cycle-safe `iter_descendants`, per-system `Local<T>` state (`build_with_local`, `register_handle_with_local`), singleton `GuestRes<R>` / `GuestResMut<R>` with `is_added()` / `is_changed()` ticks, all 16 Bevy `0.20` schedules (`BevySchedules`), `.in_set()` / `.before()` / `.after()` / `.after_weak()` / `.ambiguous_with*()` ordering (including `"host::<name>"`), `.main_thread()`, and composable run conditions (`conditions::{run_once, not, and, or, xor, every_n_ticks, component_registered, symbol_exists}`).
- **Fluent `GuestEntityCommands`, Deferred Commands, Dynamic Components, Messages & Observers:** Fluent `GuestEntityCommands` + `GuestChildSpawner` (`commands.entity(...)`, `spawn_entity`, `.with_children(...)`, `.disable()`, `.enable()`, `.observe(...)`, `.trigger(...)`), `GuestCommands` (`spawn`, `spawn_persistent`, `spawn_batch`, `FfiChildOf`, `add_children`, `disable_entity`, `enable_entity`, `insert`, `insert_if_new`, `insert_default`, `remove`, `despawn`, `despawn_all`, `trigger`, `trigger_ex`, `observe_add`/`insert`/`discard`/`remove`/`despawn`, `begin_transaction`, `insert_relation`, `remove_relation`), `entity_children`, batch & zero-allocation streaming `Messages<M>` (`read_messages_into*`, `drain_messages*`, `DrainOptions`, `Chunk::send_messages`/`send_pod_messages`, and witness-backed non-POD via `SystemContext`, `RunConditionSystemContext`, `ChunkIter`, and `Chunk`), `GuestComponentOptions` (5 hooks, `clone`, `require`, `require_handle_with_value`, `schema_fingerprint`, `BAM_FIELDS`, `from_mod_component`), and global/`EntityEvent`/lifecycle/`Ready`/propagating `ObserverBuilder` + `TypedObserverBuilder<E>` observers with composable `.run_if(...)` run conditions.
- **Unconditional Bevy `0.20` Subsystem Bridges (`bridges`, available even under `--no-default-features`):**
  - **Built-in `bamabi_bridges` / `bamabi_reflect` ABI parity (48 schema-hashed `ModComponent` mirrors + 18 `HostWireBridge`/`HostStringBridge` VTables):** `std_types`, `picking`, `camera_light`, `audio`, `ui`, `asset`, `state`, `reflect`, and `bsn`.
  - **Host-opt-in convention bridges:** `render`, `window`, `gizmos`, `FfiAssetEvent`, `FfiStateTransitionEvent`, and `InheritedVisibility`/`ViewVisibility`.
- **Parallel Chunk Processing & Hot Reload:** Work-stealing `SystemContext::par_chunks` / `par_chunks_readonly` plus optional `GuestPlugin::save_state` / `restore_state` byte-snapshot callbacks.

## Testing & Verification

```bash
cargo fmt -p bamabi_guest -- --check
cargo clippy -p bamabi_guest --all-targets -- -D warnings
cargo clippy -p bamabi_guest --all-targets --all-features -- -D warnings
cargo test -p bamabi_guest --no-default-features
cargo test -p bamabi_guest --all-features
cargo test --doc -p bamabi_guest --no-default-features
cargo test --doc -p bamabi_guest --all-features
RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_guest --no-deps
RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_guest --no-deps --all-features
MIRIFLAGS="-Zmiri-tree-borrows -Zmiri-strict-provenance" cargo +nightly miri test -p bamabi_guest
```

## Benchmarking & Stress Testing

`bamabi-wire` is Layer 0 (zero Bevy dependencies) and contains no Criterion `[[bench]]` targets; runtime ECS/FFI benchmarks live in Layer 4 (`bamabi-showcase`, `docs/BENCHMARKS.md`). Within `bamabi_guest`:

- **Command Queue Stress Test:** `cargo test -p bamabi_guest --test command_queue_stress --all-features` exercises the `<= 16`-component stack fast path, the `> 16`-component heap fallback (`20` components), ZSTs, and padded structs (`PaddedPayload`).
- **Telemetry Frame Wire Type:** [`SystemTelemetryDigest64`](src/bridges/reflect.rs) (`"BAMT"`, 64-byte frame) models per-system `p50_us` / `p99_us` latency, matched entities/tables, `summary_tick` skips, and queued commands.

## License

Dual-licensed under `MIT OR Apache-2.0` ([`LICENSE-MIT`](../../LICENSE-MIT) and [`LICENSE-APACHE`](../../LICENSE-APACHE)).
