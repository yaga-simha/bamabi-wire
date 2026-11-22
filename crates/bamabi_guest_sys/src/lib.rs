//! Low-level, **zero-Bevy-dependency** guest FFI internals for `bamabi` Rust modules (`ABI 0.3.0`).
//!
//! Mod authors should normally depend on `bamabi_guest` (`#![forbid(unsafe_code)]`) rather than
//! calling this `-sys` crate directly. Read this documentation incrementally from Step 1 to Step 6
//! to understand how every guest-side `unsafe` boundary is isolated and verified.
//!
//! ## Workspace & Crate Documentation
//!
//! - **This Crate (`bamabi_guest_sys`):**
//!   - [`docs::architecture`] — Guest-side `unsafe` boundary architecture, 73-slot `host_slot!`
//!     negotiation, move-safe `GuestInstance` provenance, `ChunkColumnDispenser`, and Bevy `0.20`
//!     `wire_types`.
//!   - [`docs::tests`] — Integration tests, soundness regressions (`S1`–`S13`), `proptest` fuzzing,
//!     and Kani formal proofs (`PO-G1`, `PO-G3`..`PO-G9`).
//! - **Workspace-Wide Guides:**
//!   - [`bamabi_docs`](../bamabi_docs/index.html) — Workspace landing hub across all crates
//!     (`bamabi_sys` -> `bamabi_macros` -> `bamabi_sdk_gen` -> `bamabi_guest_sys` -> `bamabi_guest`
//!     -> `xtask`).
//!   - [`architecture`](../bamabi_docs/architecture/index.html) · [`development`](../bamabi_docs/development/index.html) · [`testing`](../bamabi_docs/testing/index.html)
//! - **Sibling Crates:**
//!   [`bamabi_sys`](../bamabi_sys/index.html) ·
//!   [`bamabi_macros`](../bamabi_macros/index.html) ·
//!   [`bamabi_sdk_gen`](../bamabi_sdk_gen/index.html) ·
//!   [`bamabi_guest`](../bamabi_guest/index.html) ·
//!   [`xtask`](../xtask/index.html)
//!
//! ---
//!
//! ## Step 1: Safety Boundary Role — Isolating All Guest `unsafe`
//!
//! `bamabi_guest_sys` is the **only** crate in the guest SDK stack allowed to perform raw pointer
//! casts, dereference host-supplied C-ABI tables, or define `extern "C"` trampolines. That strict
//! separation lets `bamabi_guest` enforce `#![forbid(unsafe_code)]` while remaining zero-copy on hot
//! ECS paths:
//!
//! - **Zero Bevy Dependencies:** Depends solely on `bamabi_sys` and `std` (MSRV `1.87`, Edition
//!   `2024`). No `bevy_*` crate is ever linked into guest `.cdylib` mods.
//! - **Enforced `unsafe` Hygiene:** This crate compiles with `#![warn(missing_docs)]`,
//!   `#![deny(unsafe_op_in_unsafe_fn, clippy::undocumented_unsafe_blocks, clippy::missing_safety_doc)]`,
//!   and workspace `improper_ctypes_definitions = "deny"`. Every `unsafe fn` documents its `# Safety`
//!   contract, and every `unsafe` block carries an explicit `// SAFETY:` justification.
//! - **Fail-Closed Validation:** Host pointers, struct sizes, alignments, component UUIDs,
//!   storage flags, and byte-length bounds are checked *before* forming Rust slices or references.
//!
//! ---
//!
//! ## Step 2: Loading & Host Table (`Host`, `ValidatedLoadCtx`, 73 Slots & `"bam.v0.*"`)
//!
//! When a host loads a guest `.cdylib` and invokes `bamabi_init`, it passes a `*const BamHostApi`
//! function table and `*mut BamLoadCtx`:
//!
//! 1. **Bounded Table Copy & ABI Handshake:** [`Host::from_load_ctx`] (or [`Host::from_raw`] when
//!    paired with an active [`BamHostCtx`](../bamabi_sys/struct.BamHostCtx.html)) verifies non-nullness and
//!    alignment, performs a bounded prefix copy via `read_descriptor::<BamHostApi>` up to
//!    `min(struct_size, size_of::<BamHostApi>())`, checks [`ABI_VERSION`](../bamabi_sys/constant.ABI_VERSION.html)
//!    SemVer compatibility, and verifies all 27 mandatory core ABI `0.2` slots
//!    (`BamHostApi::MIN_V0_2_SIZE` = `224` B on 64-bit, `116` B on 32-bit).
//! 2. **73-Slot `covers_slot` + `UNSUPPORTED` Fallback:** The full ABI `0.3.0`
//!    [`BamHostApi`](../bamabi_sys/host_api/struct.BamHostApi.html) contains **73** slots
//!    (`MIN_V0_3_SIZE` = `552` B / `280` B for the first 68 slots; full `size_of` = `592` B on
//!    64-bit, `300` B on 32-bit). All **46** optional/extended slots beyond the 27 core slots
//!    (23 extended `0.2` slots + 23 `0.3.0` slots) are resolved through `host_slot!`, which checks
//!    `api.covers_slot(offset_of!(..), size_of_val(..))` and `Option::is_some()`, returning
//!    [`FfiResult::UNSUPPORTED`] cleanly when running against an older or truncated host.
//! 3. **Custom Symbols & VTables:** [`Host::resolve_custom_symbol`] and [`Host::resolve_custom_fn`]
//!    enforce the [`CUSTOM_SYMBOL_PREFIX`] (`"bam.v0."`) namespace. [`Host::resolve_vtable`],
//!    [`Host::resolve_mapped_wire_vtable`], and [`Host::resolve_mapped_string_vtable`] validate
//!    the leading [`VTableHeader`](../bamabi_sys/struct.VTableHeader.html) (`struct_size`, `abi_version`, and
//!    `ComponentUuid`) before forming `&V`.
//! 4. **Init-Time Registration:** [`ValidatedLoadCtx`] wraps the active `*mut BamLoadCtx` during
//!    `bamabi_init` for registering POD/dynamic components, relationships, schedule sets, systems,
//!    observers, and schema migrations.
//!
//! ---
//!
//! ## Step 3: Per-World Ownership, Panic Isolation & Hot-Reload
//!
//! A single `.cdylib` may be loaded into multiple Bevy `World`s concurrently without cross-world
//! interference:
//!
//! - **Per-World [`GuestInstance`]:** [`init_guest_instance`] allocates a heap-owned
//!   [`GuestInstance`] with **zero** process-global mutable state. All closures, owned descriptor
//!   buffers, `CString` names, per-instance `"bam.v0.*"` symbols, and shutdown hooks live inside
//!   that instance. If `build_fn` fails or panics, all staged allocations roll back cleanly on the
//!   guest heap and `*instance_out` remains `null`.
//! - **Move-Safe Provenance & Deterministic LIFO Teardown:** Internal heap anchors
//!   (`OwnedPinnedHost`, `OwnedRuntimeAllocs`, `OwnedManifestBytes`) use `Box::into_raw` so moving
//!   `GuestInstance` does not invalidate `*const Host` or internal pointers held by trampolines
//!   and [`Host`] under Miri Tree/Stacked Borrows. Runtime observer allocations created after init
//!   are tracked in a mutex-guarded `RuntimeAllocTracker` and drained in LIFO order during
//!   [`shutdown_guest_instance`] before init-time allocations.
//! - **Lock-Free Re-entrancy Guard ([`ReentrantCell`]):** Wraps `FnMut` system, `run_if`, and
//!   observer closures in `UnsafeCell<T>` + `AtomicBool` (`Acquire`/`Release` + RAII drop guard)
//!   so hot per-frame dispatch avoids OS mutex overhead while rejecting nested re-entry with
//!   [`FfiResult::ACCESS_CONFLICT`] (or [`FfiBool::FALSE`] for `run_if`) *before* forming `&mut`.
//! - **Panic Containment ([`mod@panic`]):** [`catch_guest_result`], [`catch_guest_bool`], and
//!   [`catch_guest_unit`] wrap every `extern "C"` boundary (`catch_unwind`), dropping any unwind
//!   payload strictly on the guest heap so panics never unwind across `extern "C"` or cross
//!   allocators.
//! - **Hot-Reload State Transfer:** [`GuestInstance::set_save_state_hook`] and
//!   [`GuestInstance::set_restore_state_hook`] pair with [`guest_save_state_trampoline`] and
//!   [`guest_restore_state_trampoline`] (which guards `len > isize::MAX` before forming `&[u8]`).
//!
//! ---
//!
//! ## Step 4: Validated Data Plane (`iter`, `commands`, `component`, `trampoline`, `wire_types`)
//!
//! - **[`iter`] — Validated Chunk, Sparse-Row & Resource Views:** [`ValidatedIter`] introspects
//!   [`BamColumnInfo`](../bamabi_sys/struct.BamColumnInfo.html) (`mode`, `size`, `align`, `uuid`, storage flags)
//!   to vend [`ValidatedSlice`] / [`ValidatedSliceMut`], indirect [`SparseRows`] /
//!   [`SparseRowsMut`], untracked writes paired with [`ValidatedIter::mark_changed_range`],
//!   non-destructive [`ValidatedIter::count`], [`ValidatedIter::up_cascade_checked`], work-stealing
//!   [`ValidatedIter::par_for_each_chunk`] / [`ValidatedIter::par_for_each_chunk_readonly`], POD and
//!   witness-encoded non-POD message batches, `RemovedComponents` drains, and opaque
//!   [`ValidatedWireVTable`] / [`ValidatedStringVTable`] row/resource access. [`ChunkColumnDispenser`]
//!   tracks component handles and byte intervals across up to `MAX_TERMS_PER_QUERY` terms so safe
//!   tuple queries in `bamabi_guest` never alias mutably.
//! - **[`commands`] — Deferred Entity & Relationship Commands:** [`ValidatedCommands`] wraps
//!   `*mut BamCommands` for `spawn_empty`, `spawn` / `spawn_persistent` ([`SpawnEntry`]), columnar
//!   `spawn_batch_columns` ([`SpawnBatchColumn`]), `despawn`, `despawn_all`, `insert`,
//!   `insert_if_new`, `insert_default`, `remove`, `trigger` / `trigger_ex`, runtime `observe*`,
//!   `spawn_scene`, ABI `0.3.0` `checkpoint` / `rollback`, and `insert_relation` / `remove_relation`,
//!   verifying safe entries against `Host::component_layout_info`.
//! - **[`component`] — Component Registration & Schema Migration:** [`PodComponentHooks`],
//!   [`register_pod_component`], [`register_pod_component_with_hooks`],
//!   [`register_dynamic_component_with_drop`] (`generic_drop_trampoline`),
//!   [`register_component_with_layout`], [`register_relation`], [`resolve_component`], and
//!   panic-guarded `BamRowMigrateFn` migration helpers ([`migrate_component_column_pod`],
//!   [`migrate_mod_component_column`], [`migrate_pod_component_column`]).
//! - **[`trampoline`] — System Frame & Dynamic Move Verification:** [`ValidatedSystemInvocation`]
//!   owns up to `MAX_QUERIES_PER_SYSTEM` query slots plus an affine, non-`Copy`
//!   [`ValidatedSystemRef`] token (enforcing single mutable access on handle-based resource lookups).
//!   Also provides [`SystemRegistrationConfig`], [`PreparedRunIf`], `wrap_system_callback!`,
//!   `wrap_run_condition!`, and same-mod [`ModComponent`](../bamabi_sys/trait.ModComponent.html) VTable
//!   provenance verification (`verify_same_mod_dynamic_component`, `insert_moved`, `spawn_moved`).
//! - **[`wire_types`] — Unconditional Bevy `0.20` POD Projections:** Zero-Bevy `#[repr(C)]` /
//!   `#[repr(transparent)]` `AbiPod` structs with compile-time zero-padding assertions and
//!   `FIELD_SCHEMA_HASH` constants matching `bamabi_bridges`, organized across `std_types`
//!   (including all 194 named `FfiKeyCode` indices `0..=193`), `lifecycle` (`ModStateSlab` +
//!   64-byte `"BAMT"` `SystemTelemetryDigest64`), `picking`, `camera_light`, `audio`, `ui`, and
//!   `render`.
//!
//! ---
//!
//! ## Step 5: Module Map & Reading Order
//!
//! | Order | Module | Responsibility |
//! |---|---|---|
//! | 1 | [`mod@panic`] | `catch_guest_result` / `catch_guest_bool` / `catch_guest_unit` guest-heap unwind barriers |
//! | 2 | [`host`] | Bounded `Host` API copy, `host_slot!` guards, `"bam.v0.*"` / `VTableHeader` checks, [`ValidatedLoadCtx`] |
//! | 3 | [`instance`] | Per-World [`GuestInstance`], LIFO teardown, `RuntimeAllocTracker`, init rollback, state transfer |
//! | 4 | [`trampoline`] | [`ReentrantCell`], [`ValidatedSystemInvocation`], [`ValidatedSystemRef`], system/observer trampolines |
//! | 5 | [`component`] | POD/drop component & relation registration, [`PodComponentHooks`], [`BamRowMigrateFn`](../bamabi_sys/type.BamRowMigrateFn.html) migrations |
//! | 6 | [`iter`] | [`ValidatedIter`], [`ValidatedSlice`]/[`ValidatedSliceMut`], [`SparseRows`]/[`SparseRowsMut`], [`ChunkColumnDispenser`] |
//! | 7 | [`commands`] | [`ValidatedCommands`], [`SpawnEntry`], [`SpawnBatchColumn`], checkpoints, relations, triggers |
//! | 8 | [`wire_types`] | Unconditional Bevy `0.20` `AbiPod` mirrors (`std_types`, `lifecycle`, `picking`, `camera_light`, `audio`, `ui`, `render`) |
//!
//! For deep implementation internals and the full test/proof catalog, see [`docs::architecture`]
//! (`crates/bamabi_guest_sys/ARCHITECTURE.md`) and [`docs::tests`]
//! (`crates/bamabi_guest_sys/TESTS.md`).
//!
//! ---
//!
//! ## Step 6: Setup, Build, Test, Miri & Kani Verification
//!
//! Run all checks standalone inside `bamabi-wire`:
//!
//! ```text
//! cargo fmt -p bamabi_guest_sys -- --check
//! cargo clippy -p bamabi_guest_sys --all-targets --all-features -- -D warnings
//! RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_guest_sys --no-deps --all-features
//! cargo test -p bamabi_guest_sys --all-features
//! cargo test --doc -p bamabi_guest_sys --all-features
//! MIRIFLAGS="-Zmiri-tree-borrows -Zmiri-strict-provenance" cargo +nightly miri test -p bamabi_guest_sys
//! MIRIFLAGS="-Zmiri-strict-provenance" cargo +nightly miri test -p bamabi_guest_sys --test soundness_regressions
//! cargo kani -p bamabi_guest_sys
//! ```
//!
//! ### Minimal Doctest Example (`ReentrantCell`, `catch_guest_result` & `wire_types`)
//!
//! ```rust
//! use bamabi_guest_sys::{
//!     FfiResult, ReentrantCell, catch_guest_result,
//!     wire_types::FfiTransform,
//! };
//!
//! // Panic containment converts guest panics into `FfiResult::PANIC_CAUGHT`
//! // while dropping the panic payload on the guest heap.
//! let status = catch_guest_result(|| {
//!     if false {
//!         panic!("never crosses extern \"C\"");
//!     }
//!     FfiResult::OK
//! });
//! assert_eq!(status, FfiResult::OK);
//!
//! // `ReentrantCell` rejects overlapping re-entrant mutable access cleanly.
//! let cell = ReentrantCell::new(FfiTransform::from_xyz(1.0, 2.0, 3.0));
//! let outer = cell.try_with_mut(|tf| {
//!     assert!(cell.try_with_mut(|_| ()).is_none());
//!     tf.translation[0] += 4.0;
//!     tf.translation
//! });
//! assert_eq!(outer, Some([5.0, 2.0, 3.0]));
//! ```

#![warn(missing_docs)]
#![deny(
    unsafe_op_in_unsafe_fn,
    clippy::undocumented_unsafe_blocks,
    clippy::missing_safety_doc
)]

/// Safely resolves an optional extended [`bamabi_sys::BamHostApi`] slot, returning
/// `Some(fn_ptr)` only when `api.covers_slot(offset_of!(BamHostApi, $slot), size_of_val(&api.$slot))`
/// and the slot is populated.
macro_rules! host_slot {
    ($api:expr, $slot:ident) => {{
        let __api: &::bamabi_sys::BamHostApi = $api;
        if __api.covers_slot(
            ::core::mem::offset_of!(::bamabi_sys::BamHostApi, $slot),
            ::core::mem::size_of_val(&__api.$slot),
        ) {
            __api.$slot
        } else {
            ::core::option::Option::None
        }
    }};
}

pub mod commands;
pub mod component;
#[cfg(doc)]
/// Architecture and verification guides for `bamabi_guest_sys`.
pub mod docs {
    pub mod architecture {
        #![doc = include_str!("../ARCHITECTURE.md")]
    }

    pub mod tests {
        #![doc = include_str!("../TESTS.md")]
    }
}
pub mod host;
pub mod instance;
pub mod iter;
pub mod panic;
pub mod trampoline;
pub mod wire_types;

#[cfg(kani)]
mod kani_bounds_proofs;
#[cfg(kani)]
mod kani_proofs;

pub use bamabi_sys::{BamSystemCtx, FfiBool, FfiResult};

pub use commands::{SpawnBatchColumn, SpawnEntry, TypedComponent, ValidatedCommands};
pub use component::{
    PodComponentHooks, generic_drop_trampoline, migrate_component_column_pod,
    migrate_component_column_raw, migrate_mod_component_column, migrate_pod_component_column,
    pod_clone_trampoline, register_component_raw, register_component_with_layout,
    register_dynamic_component_with_drop, register_pod_component,
    register_pod_component_with_hooks, register_relation, register_relation_raw, resolve_component,
};
pub use host::{
    CUSTOM_SYMBOL_PREFIX, Host, ValidatedLoadCtx, ValidatedStringVTable, ValidatedWireVTable,
};
pub use instance::{
    GuestInstance, guest_restore_state_trampoline, guest_save_state_trampoline,
    init_guest_instance, qualify_mod_component_identity, resolve_guest_instance_symbol,
    resolve_mod_component, shutdown_guest_instance,
};
pub use iter::{
    BatchColCache, ChunkColumnDispenser, DrainOptions, QuerySlotScratch, SparseRows,
    SparseRowsIter, SparseRowsMut, UuidFilter, ValidatedIter, ValidatedSlice, ValidatedSliceMut,
};
pub use panic::{catch_guest_bool, catch_guest_result, catch_guest_unit};
pub use trampoline::{
    LazyCommands, PreparedRunIf, ReentrantCell, SystemRegistrationConfig,
    ValidatedSystemInvocation, ValidatedSystemRef, ValidatedSystemSlice, allocate_run_if_ctx_query,
    allocate_run_if_ctx_trampoline, allocate_run_if_trampoline, allocate_system_slice_trampoline,
    allocate_system_trampoline, configure_schedule_set, configure_schedule_set_raw,
    iter_dynamic_slice, iter_dynamic_slice_mut, iter_entities_and_dynamic_slice_mut,
    mount_system_in_schedule, register_lifecycle_observer_closure, register_observer_closure,
    register_observer_closure_with_options, register_observer_mut_closure, register_system_closure,
    register_system_handle_closure, register_system_handle_slice_closure,
    register_system_slice_closure, verify_same_mod_dynamic_component,
};
