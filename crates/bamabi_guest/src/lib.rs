#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! # `bamabi_guest`: Safe Idiomatic Rust Guest SDK (`ABI 0.3.0`)
//!
//! Safe (`#![forbid(unsafe_code)]`), **zero-Bevy-dependency** Rust SDK for authoring native
//! `.cdylib` mods targeting `bamabi` (`ABI 0.3.0`, `v0.3.0`, Layer 0 of `bamabi-wire`).
//!
//! Follow the eight incremental steps below to go from the core mental model to a tested,
//! hot-reloadable native mod.
//!
//! ## Workspace & Crate Documentation
//!
//! - **Crate guides:** [`docs::architecture`] (`crates/bamabi_guest/ARCHITECTURE.md`) · [`docs::tests`] (`crates/bamabi_guest/TESTS.md`)
//! - **Workspace guides:** [`bamabi_docs`](../bamabi_docs/index.html) ([`architecture`](../bamabi_docs/architecture/index.html), [`development`](../bamabi_docs/development/index.html), [`testing`](../bamabi_docs/testing/index.html))
//! - **Sibling crates:** [`bamabi_sys`](../bamabi_sys/index.html) · [`bamabi_macros`](../bamabi_macros/index.html) · [`bamabi_sdk_gen`](../bamabi_sdk_gen/index.html) · [`bamabi_guest_sys`](../bamabi_guest_sys/index.html) · [`xtask`](../xtask/index.html)
//!
//! ---
//!
//! ## Step 1: Mental Model — Safe `#![forbid(unsafe_code)]` `.cdylib` SDK & Per-World Isolation
//!
//! - **Zero `unsafe` in Guest Code:** This crate enforces `#![forbid(unsafe_code)]` at the crate
//!   root. Every raw FFI pointer check, [`BamHostApi`](raw::BamHostApi) slot bounds check,
//!   [`VTableHeader`] verification, and `extern "C"` panic trampoline is isolated inside
//!   `bamabi_guest_sys` over the [`bamabi_sys`] `ABI 0.3.0` (`0x0003_0000`) wire contract.
//! - **Zero Bevy Dependencies:** `bamabi_guest` depends only on `bamabi_sys`, `bamabi_guest_sys`,
//!   and optionally `bamabi_macros` (`feature = "derive"`, `default = []`). Guest mods compile
//!   independently of Bevy and across mismatched Rust compiler toolchains.
//! - **Per-World Isolation:** [`export_mod!`] exports `bamabi_manifest`, `bamabi_init`,
//!   `bamabi_shutdown`, and `bamabi_resolve_symbol` backed by a heap-allocated per-World
//!   [`GuestInstance`](raw::GuestInstance) with **zero** process-global mutable state. Loading the
//!   same `.cdylib` into multiple Bevy `World`s simultaneously keeps every instance isolated.
//! - **Panic & Unwind Policy (`panic = "unwind"` vs. `panic = "abort"`):** Every function pointer
//!   across `bamabi` uses `extern "C"` (`nounwind`), never `extern "C-unwind"`. Unwinding across
//!   `extern "C"` is Undefined Behavior, and the host never frees guest-allocated panic payloads:
//!   - **`panic = "unwind"` *(Cargo default — recommended)*:** Guest trampolines (`bamabi_init`,
//!     `bamabi_shutdown`, system callbacks, `run_if` predicates, observers, migration callbacks,
//!     and component destructors) catch panics inside the guest, drop the payload on the guest
//!     heap, release their lock-free re-entrancy guard, and return [`FfiResult::PANIC_CAUGHT`]
//!     (or [`FfiBool::FALSE`] for `run_if`). The host discards uncommitted commands from that tick
//!     and applies its `ModErrorPolicy` without crashing the game.
//!   - **`panic = "abort"`:** `std::panic::catch_unwind` compiles away and any guest panic aborts
//!     the OS process immediately. Use only for trusted first-party mods needing smaller binaries.
//!
//! ---
//!
//! ## Step 2: Hello World Mod — `GuestPlugin`, `GuestApp`, `export_mod!`, `prelude`
//!
//! Implement [`GuestPlugin`] on a `Default + Send + 'static` struct, register systems and
//! components inside [`GuestPlugin::build`] on [`GuestApp`], and export the C entry points with
//! [`export_mod!`]. Importing [`prelude`] (`use bamabi_guest::prelude::*;`) brings in the core
//! traits, builders, bridges, and macros.
//!
//! ```no_run
//! use bamabi_guest::prelude::*;
//! use core::ffi::CStr;
//!
//! #[derive(Default)]
//! pub struct HelloMod;
//!
//! impl GuestPlugin for HelloMod {
//!     const NAME: &'static CStr = c"hello_mod";
//!     const VERSION: &'static CStr = c"0.3.0";
//!
//!     fn build(&mut self, app: &mut GuestApp<'_>) -> FfiResult {
//!         app.system(c"hello_tick")
//!             .schedule(ScheduleId::UPDATE)
//!             .build(|ctx| {
//!                 let _ = guest_log!(ctx, info, "hello from hello_mod");
//!             })
//!     }
//! }
//!
//! export_mod!(plugin: HelloMod);
//! ```
//!
//! [`export_mod!`] also supports explicit CStr manifests (`export_mod!(name: c"...", version: c"...", build_fn)`),
//! Cargo package metadata (`export_mod!(build_fn)`), and raw C-ABI hooks (`export_mod!(manifest: ..., init: ..., shutdown: ...)`).
//! Implement [`GuestPlugin::save_state`] and [`GuestPlugin::restore_state`] (or
//! [`GuestApp::on_save_state`] / [`GuestApp::on_restore_state`]) for hot-reload state transfer.
//!
//! ---
//!
//! ## Step 3: Defining Components & Systems
//!
//! Cross-language Plain Old Data components implement [`AbiPod`] and [`GuestComponent`] (either
//! via `#[derive(AbiPod, GuestComponent)]` when `feature = "derive"` is enabled, via
//! `#[mod_component(..., guest)]`, or manually on `#[repr(C)]` types). Unqualified names such as
//! `c"Health"` are automatically prefixed with `"<manifest.name>::Health"` when registered via
//! [`GuestApp::register_component`] or [`GuestApp::register_component_with_options`]
//! ([`GuestComponentOptions`] adds `sparse_set`, `immutable`, `summary_tick`, `clone`, lifecycle
//! hooks, and `require::<R>()`). Non-POD Rust components implement [`ModComponent`] and register
//! via [`GuestApp::register_dynamic_component`].
//!
//! Systems are built with [`GuestApp::system`]:
//! - **Typed Queries (`1..=16` terms):** [`SystemBuilder::query_typed`] accepts any [`QueryTuple`]
//!   over `&T`, `&mut T`, `Option<&T>`, `Option<&mut T>`, [`With<T>`], [`Without<T>`],
//!   [`Changed<T>`], [`Added<T>`], [`Allow<T>`], [`Has<T>`], [`Or<(T0, T1, ...)>`] (2..=8
//!   branches, plus [`Or2`] / [`Or3`] aliases), and [`FfiEntity`]. Up to [`MAX_QUERIES_PER_SYSTEM`]
//!   (8) queries per system are supported (`ctx.single_query`, `ctx.query_typed`,
//!   `ctx.queries_2_typed`, `ctx.queries_3_typed`).
//! - **Dynamic Queries & `RemovedComponents`:** [`SystemBuilder::query`] + [`QueryBuilder`]
//!   support handle-based, dynamic, and entity-disabling terms (`allow_disabled`, `with_disabled`,
//!   `without_disabled`, `has_disabled`), plus removal streams via [`QueryBuilder::removed`] /
//!   [`SystemBuilder::removed`] and [`SystemContext::read_removed`].
//! - **Resources, Messages & Local State:** [`SystemBuilder::resource_read`] /
//!   [`SystemBuilder::resource_write`] pair with [`SystemContext::res_with_ticks`] /
//!   [`SystemContext::res_mut_with_ticks`] returning [`GuestRes`] / [`GuestResMut`] (with
//!   [`BamChangeTicks::is_added`] / [`BamChangeTicks::is_changed`]); buffered streams use
//!   `message_read` / `message_write` (`read_messages`, `send_message`, plus non-POD witness
//!   streams); per-system `Local<L>` state uses [`SystemBuilder::build_with_local`].
//!
//! ```text
//! #[repr(C)]
//! #[derive(Clone, Copy, Debug, Default, PartialEq, AbiPod, GuestComponent)]
//! #[guest_component(name = "my_mod::Health", summary_tick)]
//! pub struct Health {
//!     pub current: f32,
//!     pub max: f32,
//! }
//! ```
//!
//! ```no_run
//! use bamabi_guest::prelude::*;
//!
//! fn register_systems(app: &mut GuestApp<'_>) -> FfiResult {
//!     app.system(c"inspect_hierarchy")
//!         .query_typed::<(
//!             FfiEntity,
//!             &FfiChildOf,
//!             Or<(With<FfiChildOf>, Without<FfiDisabled>)>,
//!         )>()
//!         .removed::<FfiChildOf>()
//!         .build_with_local(0u64, |ctx, tick_count| {
//!             *tick_count += 1;
//!             let _removed = ctx.read_removed::<FfiChildOf>(0);
//!             let _total = ctx.single_query::<(
//!                 FfiEntity,
//!                 &FfiChildOf,
//!                 Or<(With<FfiChildOf>, Without<FfiDisabled>)>,
//!             )>()
//!             .count();
//!         })
//! }
//! ```
//!
//! ---
//!
//! ## Step 4: Iterating & Mutating Data
//!
//! Choose the abstraction level that matches your system:
//!
//! - **Typed Ergonomic Iteration ([`TypedChunkIter`]):** Borrow via
//!   [`SystemContext::single_query`] (or `query_typed` / `queries_2_typed` / `queries_3_typed`) and
//!   iterate with [`TypedChunkIter::for_each`], [`TypedChunkIter::single_chunk`] (returning
//!   [`QuerySingleError`]), [`TypedChunkIter::is_empty`], [`TypedChunkIter::count`] (non-destructive),
//!   or [`TypedChunkIter::up_cascade`] (ancestor lookup).
//! - **Low-Level Chunk & Sparse Views ([`ChunkIter`], [`Chunk`]):** Iterate chunks via
//!   [`ChunkIter::next_chunk_view`]; borrow dense columns via [`Chunk::component`] /
//!   [`Chunk::component_mut`] or split disjoint borrows via [`Chunk::component_read_write`] /
//!   [`Chunk::component_write_2`] / [`Chunk::entities_and_component_mut`]. For `SparseSet`
//!   components, use [`Chunk::component_rows`] / [`Chunk::component_rows_mut`] ([`SparseRows`] /
//!   [`SparseRowsMut`]).
//! - **Fine-Grained Change Marking:** Borrow [`Chunk::component_mut_untracked`] and explicitly mark
//!   modified rows with [`Chunk::mark_changed`] or [`Chunk::mark_changed_range`] so downstream
//!   `Changed<T>` queries do not wake on untouched rows.
//! - **Parallel Chunk Processing:** Dispatch chunks across the host `ComputeTaskPool` via
//!   [`TypedChunkIter::par_for_each`] / [`SystemContext::par_chunks`], or enforce read-only access at
//!   compile time and across FFI via [`TypedChunkIter::par_for_each_chunk_readonly`] (checked with
//!   [`QueryTuple::IS_READ_ONLY`]) and [`SystemContext::par_chunks_readonly`].
//! - **Random Entity Access:** Use [`SystemContext::get`], [`SystemContext::get_mut`],
//!   [`SystemContext::get_many`], or [`SystemContext::get_dynamic`] /
//!   [`SystemContext::get_dynamic_mut`].
//!
//! ---
//!
//! ## Step 5: Commands, Transactions, Hierarchy, Observers & Schedules
//!
//! - **Deferred Commands ([`GuestCommands`]):** Queue `spawn_empty`, `spawn`, `spawn_persistent`,
//!   `spawn_batch` (SoA columns), `despawn`, `despawn_all`, `insert`, `insert_if_new`,
//!   `insert_default`, `remove`, and `trigger` / `trigger_ex`.
//! - **RAII Transactions ([`GuestTransaction`]):** Wrap multi-step command sequences in
//!   [`GuestCommands::transaction`] or [`GuestCommands::begin_transaction`]. Any early `?` error or
//!   drop without [`GuestTransaction::commit`] rolls back all commands queued since the checkpoint.
//! - **Fluent Entity & Hierarchy Builders ([`GuestEntityCommands`], [`GuestChildSpawner`]):** Use
//!   `commands.entity(e)` or `commands.spawn_entity()` to chain `.insert_component(...)`,
//!   `.with_children(...)`, `.disable()` / `.enable()`, `.observe(...)`, and `.trigger(...)`.
//!   Read hierarchy children via [`SystemContext::children`] and cycle-safe
//!   [`SystemContext::iter_descendants`] (paired with [`QueryBuilder::read_children`]).
//! - **Typed Relationships:** Register ABI `0.3.0` relationships via [`RelationDescBuilder`] +
//!   [`GuestApp::register_relation`] (or dynamic hook pairs via
//!   [`GuestApp::register_relationship_pair`]) and mutate edges with
//!   [`GuestCommands::insert_relation`] / [`GuestCommands::remove_relation`].
//! - **Observers:** Register global, entity-scoped, lifecycle (`Add`/`Insert`/`Discard`/`Remove`/
//!   `Despawn`), `Ready`, mutable/stoppable, and upward-propagating observers via
//!   [`GuestApp::observe`], [`GuestApp::observe_event`] ([`TypedObserverBuilder`]),
//!   [`GuestApp::observe_ready_with`] ([`ObserverBuilder`]), or runtime [`GuestCommands`] observers.
//! - **Schedules, Ordering & Run Conditions:** Target built-in [`ScheduleId`]s or all 16 Bevy
//!   `0.20` schedules via [`BevySchedules`]; define sets via [`ScheduleSetBuilder`] +
//!   [`GuestApp::configure_schedule_set`], decouple system handles via
//!   [`SystemBuilder::register_handle`] + [`GuestApp::mount_system`], order with `.in_set()` /
//!   `.before()` / `.after()` / `.after_weak()` / `.ambiguous_with*()` / `.main_thread()`, and gate
//!   execution with chainable AND-composed `.run_if(...)` / `.run_if_fn(...)` using
//!   [`conditions`] (`run_once`, `not`, `and`, `or`, `xor`, `every_n_ticks`, `component_registered`,
//!   `component_registered_named`, `symbol_exists`).
//!
//! ---
//!
//! ## Step 6: Bevy `0.20` Subsystem Bridges (`bridges`)
//!
//! All [`bridges`] submodules are available unconditionally (even with `--no-default-features`) and
//! resolve via [`GuestBridgesHostExt`] (`.std_bridges()`, `.ui_handles()`, `.state_bridge()`,
//! etc.), [`GuestAssetHostExt`] (`.assets()`), and domain extension traits:
//!
//! - **Built-in `bamabi_bridges` / `bamabi_reflect` ABI parity:**
//!   - [`bridges::std_types`]: `FfiTransform`, `FfiGlobalTransform`, `FfiVisibility`, `Name`,
//!     `Disabled` (`GuestDisabledBridge`), `FfiColor`, `Time<Virtual|Fixed|Real>`,
//!     `ButtonInput<KeyCode|MouseButton>` (194 `FfiKeyCode` indices), mouse motion/scroll, and
//!     gamepad state/rumble.
//!   - [`bridges::picking`]: Flattened `FfiPointerPress`/`Release`/`Move`/`Click`/`Over`/`Out`/
//!     `Drag` observers and `FfiEntityCursor`.
//!   - [`bridges::camera_light`]: `FfiCamera2d`, `FfiCamera3d`, `FfiPointLight`, `FfiSpotLight`,
//!     `FfiDirectionalLight`, `FfiPanOrbitCamera`, `FfiSolariLighting`.
//!   - [`bridges::audio`]: `FfiAudioPlaybackSettings`, `FfiAudioPlayerSpec`,
//!     `FfiSpatialAudioSinkControl`, `AudioSource`, atomic `spawn_audio`.
//!   - [`bridges::ui`]: `FfiVal` (incl. `Em`/`Rem`), `FfiUiNode`, `FfiUiTextStyle`, Feathers
//!     widgets, `Activate` / `FfiValueChange` observers, `GuestUiHandles`, string/wire bridges.
//!   - [`bridges::asset`]: Isolated `mod://<ns>/...` and `mod://overlay/...` paths (`mod_asset_path`,
//!     `mod_overlay_path`), [`GuestAssetServer`], [`GuestAssetHandle`], [`TypedAssetHandle`].
//!   - [`bridges::state`]: Primary slot-0 [`GuestStateBridge`], unbounded `ModStateSlab`
//!     ([`ModStateEntry`], [`ModStateTransitionRequest`], [`ModDespawnOnExit`], [`ModDespawnOnEnter`]),
//!     and deterministic `OnEnter` / `OnExit` / `OnTransition` schedules.
//!   - [`bridges::reflect`]: `bamabi/*` / `bam/*` / `world.*` BRP constants and 64-byte `"BAMT"`
//!     [`SystemTelemetryDigest64`].
//!   - [`bridges::bsn`]: [`GuestScene`] (`cmd_spawn_scene`) and `Ready` integration.
//! - **Host-opt-in conventions:** [`bridges::render`] (`Sprite`, `StandardMaterial`, `FfiMesh2d`,
//!   `FfiMesh3d`, WESL material specs), [`bridges::window`] (`Window`, `WindowResized`,
//!   `WindowCloseRequested`, `FfiAppExit`), and [`bridges::gizmos`] (`FfiGizmo*`).
//!
//! ---
//!
//! ## Step 7: Module Map & Companion Documentation
//!
//! | Module | Purpose |
//! |---|---|
//! | [`prelude`] | Single-import wildcard (`use bamabi_guest::prelude::*;`) |
//! | [`lifecycle`] | [`GuestPlugin`], [`build_guest_plugin`], [`build_guest_fn`] |
//! | [`macros`] | [`export_mod!`] (4 forms) and [`guest_log!`] |
//! | [`component`] | [`GuestComponent`], [`GuestComponentHooks`], [`guest_component_uuid`], manifest prefixing |
//! | [`app`] | [`GuestApp`], [`SystemBuilder`], [`QueryBuilder`], [`SystemContext`], [`BevySchedules`], [`conditions`], [`observers`](app::observers), [`hierarchy`](app::hierarchy), [`v03`](app::v03) |
//! | [`iter`] | [`QueryTuple`], [`TypedChunkIter`], [`ChunkIter`], [`Chunk`], [`ChunkSlice`], [`ChunkSliceMut`], [`SparseRows`], [`RemovedComponents`](iter::filters) |
//! | [`commands`] / [`commands_lifecycle`] | [`GuestCommands`], [`GuestTransaction`], persistent spawns, non-POD [`ModComponent`] commands |
//! | [`entity_commands`] | [`GuestEntityCommands`], [`GuestChildSpawner`], `.disable()` / `.enable()` |
//! | [`bridges`] | Unconditional Bevy `0.20` subsystem wrappers (`std_types`, `ui`, `state`, `asset`, `picking`, `camera_light`, `audio`, `bsn`, `reflect`, `render`, `window`, `gizmos`) |
//! | [`raw`] | Re-exports [`bamabi_sys`] and `bamabi_guest_sys` raw escape hatches & `*RawExt` pointer traits |
//!
//! For deep architectural explanations, see [`docs::architecture`]
//! (`crates/bamabi_guest/ARCHITECTURE.md`). For a complete catalog of unit, integration, parity,
//! stress, Miri, and compile-fail tests, see [`docs::tests`] (`crates/bamabi_guest/TESTS.md`). For
//! workspace-wide guides across all crates, see [`bamabi_docs`](../bamabi_docs/index.html).
//!
//! ---
//!
//! ## Step 8: Setup, Build, Run `.cdylib`, Test & Benchmark
//!
//! ### Cargo Setup (`Cargo.toml`)
//!
//! ```toml
//! [lib]
//! crate-type = ["cdylib"]
//!
//! [dependencies]
//! bamabi_guest = { git = "https://github.com/yaga-simha/bamabi-wire.git", branch = "main", features = ["derive"] }
//! ```
//!
//! ### Build & Run `.cdylib`
//!
//! ```bash
//! cargo build --release -p <your_mod_crate>   # produces target/release/lib<name>.{so,dylib,dll}
//! ```
//!
//! Load the compiled `.cdylib` in a `bamabi` host (or via `cargo-bam` in `bamabi-tools` /
//! runnable examples in Layer 4 `bamabi-showcase`).
//!
//! ### Test & Verify `bamabi_guest`
//!
//! ```bash
//! cargo fmt -p bamabi_guest -- --check
//! cargo clippy -p bamabi_guest --all-targets -- -D warnings
//! cargo clippy -p bamabi_guest --all-targets --all-features -- -D warnings
//! cargo test -p bamabi_guest --no-default-features
//! cargo test -p bamabi_guest --all-features
//! cargo test --doc -p bamabi_guest --no-default-features
//! cargo test --doc -p bamabi_guest --all-features
//! RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_guest --no-deps --all-features
//! MIRIFLAGS="-Zmiri-tree-borrows -Zmiri-strict-provenance" cargo +nightly miri test -p bamabi_guest
//! ```
//!
//! ### Stress Testing & Benchmarks
//!
//! `bamabi-wire` is Layer 0 and contains no Criterion `[[bench]]` targets; runtime ECS/FFI
//! benchmarks live in Layer 4 (`bamabi-showcase`, `docs/BENCHMARKS.md`). Within `bamabi_guest`,
//! command-queue fast/heap paths and padded-payload soundness are stress-tested in
//! `crates/bamabi_guest/tests/command_queue_stress.rs`, and per-system host telemetry frames are
//! modeled via [`SystemTelemetryDigest64`] (`"BAMT"`).

pub use bamabi_sys;

pub mod app;
pub mod bridges;
pub mod commands;
pub mod commands_lifecycle;
pub mod component;
#[cfg(doc)]
/// Architecture and test guides for `bamabi_guest`.
pub mod docs {
    /// Wrapper architecture, sealed `QueryTuple`, `GuestTransaction` rollback, and bridge guide (`crates/bamabi_guest/ARCHITECTURE.md`).
    pub mod architecture {
        #![doc = include_str!("../ARCHITECTURE.md")]
    }

    /// Unit, integration, Bevy `0.20` parity, stress, Miri, and doctest catalog (`crates/bamabi_guest/TESTS.md`).
    pub mod tests {
        #![doc = include_str!("../TESTS.md")]
    }
}
pub mod entity_commands;
pub mod iter;
pub mod lifecycle;
pub mod macros;
pub mod prelude;
pub mod raw;

pub use bamabi_guest_sys::{SpawnEntry, TypedComponent};
pub use bamabi_sys::{
    ABI_VERSION, ABI_VERSION_MAJOR, ABI_VERSION_MINOR, ABI_VERSION_PATCH, AbiPod, AccessMode,
    BAM_COMPONENT_FLAG_IMMUTABLE, BAM_COMPONENT_FLAG_SPARSE_SET, BAM_COMPONENT_FLAG_SUMMARY_TICK,
    BAM_DISABLED_NAME, BAM_LOG_DEBUG, BAM_LOG_ERROR, BAM_LOG_INFO, BAM_LOG_TRACE, BAM_LOG_WARN,
    BAM_OBSERVER_FLAG_PROPAGATE, BAM_RELATION_FLAG_ACYCLIC, BAM_RELATION_FLAG_CASCADE_DELETE,
    BAM_RELATION_FLAG_CASCADE_DESPAWN, BAM_RELATION_FLAG_CLEANUP_REMOVE,
    BAM_RELATION_FLAG_EXCLUSIVE, BAM_RELATION_FLAG_SYMMETRIC, BAM_SCHEDULE_SET_FLAG_AFTER_WEAK,
    BAM_SCHEDULE_SET_FLAG_BEFORE_WEAK, BAM_SCHEDULE_SET_FLAG_PARALLEL,
    BAM_SCHEDULE_SET_FLAG_SEQUENCE, BAM_SCHEDULE_SET_FLAG_SEQUENCE_WEAK,
    BAM_STORAGE_FLAG_IMMUTABLE, BAM_STORAGE_FLAG_MESSAGE, BAM_STORAGE_FLAG_RESOURCE,
    BAM_STORAGE_FLAG_SPARSE_SET, BAM_STORAGE_FLAG_SUMMARY_TICK, BAM_SYSTEM_FLAG_AMBIGUOUS_WITH_ALL,
    BAM_SYSTEM_FLAG_MAIN_THREAD, BAM_TRIGGER_FLAG_PROPAGATE, BamChangeTicks, BamColumnInfo,
    BamComponent, BamEventKind, BamFieldDesc, BamFieldKind, BamLayoutInfo, BamManifest,
    BamObserverDesc, BamObserverFn, BamObserverMutFn, BamRelationDesc, BamRequiredComponent,
    BamRowMigrateFn, BamScheduleSetDesc, BamSymbol, BamSystem, BamSystemSet, BamTermDesc,
    BamTriggerDesc, ComponentDesc, ComponentUuid, FfiAssetHandle, FfiAssetLoadState, FfiBool,
    FfiChildOf, FfiDisabled, FfiEntity, FfiOption, FfiResult, FfiSlice, MAX_QUERIES_PER_SYSTEM,
    MAX_TERMS_PER_QUERY, ModComponent, RawInsertable, ScheduleId, StrView, SymbolEntry,
    VTableHeader,
};

#[cfg(feature = "derive")]
pub use bamabi_macros::{AbiPod, GuestComponent, ModComponent};

pub use app::{
    BevySchedules, EcsRunCondition, GuestApp, GuestComponentOptions, GuestRes, GuestResMut,
    IntoSystemResult, ObserverBuilder, ParamSetBuilder, QueryBuilder, RelationDescBuilder,
    RunConditionContext, RunConditionSystemContext, ScheduleSetBuilder, SystemBuilder,
    SystemContext, TypedObserverBuilder, conditions,
};
pub use bridges::{
    ACTIVATE_EVENT_UUID, APPLY_EAR_POSITIONS, APPLY_EMITTER_POS, APPLY_SPEED, APPLY_VOLUME,
    AUDIO_SOURCE_ASSET_ALIAS, BRP_METHOD_BAM_COMPONENT_SCHEMAS, BRP_METHOD_BAM_HOT_RELOAD,
    BRP_METHOD_BAM_LIST_MODS, BRP_METHOD_BAM_SYSTEM_METRICS, BRP_METHOD_GET_COMPONENTS,
    BRP_METHOD_GET_RESOURCES, BRP_METHOD_INSERT_COMPONENTS, BRP_METHOD_LIST_COMPONENTS,
    BRP_METHOD_MUTATE_COMPONENTS, BRP_METHOD_MUTATE_RESOURCES, BRP_METHOD_REGISTRY_SCHEMA,
    BRP_METHOD_RPC_DISCOVER, BRP_METHOD_WORLD_DESPAWN_ENTITY, BRP_METHOD_WORLD_GET_COMPONENTS,
    BRP_METHOD_WORLD_GET_RESOURCES, BRP_METHOD_WORLD_INSERT_COMPONENTS,
    BRP_METHOD_WORLD_INSERT_RESOURCES, BRP_METHOD_WORLD_LIST_COMPONENTS,
    BRP_METHOD_WORLD_LIST_RESOURCES, BRP_METHOD_WORLD_MUTATE_COMPONENTS,
    BRP_METHOD_WORLD_MUTATE_RESOURCES, BRP_METHOD_WORLD_QUERY, BRP_METHOD_WORLD_REMOVE_COMPONENTS,
    BRP_METHOD_WORLD_REMOVE_RESOURCES, BRP_METHOD_WORLD_SPAWN_ENTITY, CANONICAL_AUDIO_SOURCE_NAME,
    CLEAR_COLOR_CUSTOM, CLEAR_COLOR_DEFAULT, CLEAR_COLOR_NONE, DEPTH_LOAD_OP_CLEAR,
    DEPTH_LOAD_OP_LOAD, FfiAppExit, FfiAssetEvent, FfiAssetEventKind, FfiAudioPlaybackSettings,
    FfiAudioPlayerSpec, FfiBorderColor, FfiBorderRadius, FfiCamera2d, FfiCamera3d, FfiColor,
    FfiCornerRadius, FfiDirectionalLight, FfiEditableTextConfig, FfiEmSize, FfiEntityCursor,
    FfiFeathersColorInput, FfiFeathersDropdown, FfiFeathersListRow, FfiFeathersListView,
    FfiFeathersNumberInput, FfiFeathersScrollableList, FfiFeathersSelect, FfiFontSize,
    FfiGamepadButton, FfiGamepadRumble, FfiGamepadState, FfiGizmoArrow, FfiGizmoCircle,
    FfiGizmoCuboid, FfiGizmoLine, FfiGizmoRay, FfiGizmoRect, FfiGizmoSphere, FfiGlobalTransform,
    FfiInheritedVisibility, FfiInlineBox, FfiInlineImage, FfiKeyCode, FfiKeyboardInput, FfiMesh2d,
    FfiMesh3d, FfiMouseButton, FfiMouseInput, FfiMouseMotion, FfiMouseScroll, FfiPanOrbitCamera,
    FfiPointLight, FfiPointerButton, FfiPointerClick, FfiPointerDrag, FfiPointerKind,
    FfiPointerMove, FfiPointerOut, FfiPointerOver, FfiPointerPress, FfiPointerRelease, FfiRemSize,
    FfiRgba, FfiSelectedTab, FfiSolariLighting, FfiSpatialAudioSinkControl, FfiSpotLight,
    FfiSprite, FfiSpriteMaterialSpec, FfiStandardMaterial, FfiStateTransitionEvent, FfiTab,
    FfiTabList, FfiThemeContext, FfiTimeFixed, FfiTimeReal, FfiTimeVirtual, FfiTransform,
    FfiUiNode, FfiUiRect, FfiUiTextStyle, FfiVal, FfiValueChange, FfiViewVisibility, FfiVisibility,
    FfiWeslMaterialSpec, FfiWindow, FfiWindowCloseRequested, FfiWindowMode, FfiWindowResized,
    GuestAppExitExt, GuestAppPickingExt, GuestAppUiExt, GuestAssetHandle, GuestAssetHostExt,
    GuestAssetServer, GuestAudioQueryBuilderExt, GuestBackgroundColorBridge,
    GuestBorderColorBridge, GuestBridgesHostExt, GuestCameraLightQueryBuilderExt,
    GuestCommandsAudioExt, GuestCommandsBsnExt, GuestCommandsCameraLightExt,
    GuestCommandsGizmosExt, GuestCommandsPickingExt, GuestCommandsRenderExt, GuestCommandsStateExt,
    GuestCommandsUiExt, GuestDisabledBridge, GuestEmSizeBridge, GuestGamepadRumbleExt,
    GuestGlobalTransformBridge, GuestInheritedVisibilityBridge, GuestKeyboardInputBridge,
    GuestMappedStringBridge, GuestMouseInputBridge, GuestNameBridge, GuestPickingQueryBuilderExt,
    GuestRemSizeBridge, GuestRenderQueryBuilderExt, GuestScene, GuestSpriteBridge,
    GuestStandardMaterialBridge, GuestStateBridge, GuestStateHandles, GuestStateQueryBuilderExt,
    GuestStateSlabContextExt, GuestStateSystemBuilderExt, GuestStdCommandsExt, GuestStdHandles,
    GuestStdQueryBuilderExt, GuestStdSystemBuilderExt, GuestStdTypes, GuestTextSpanBridge,
    GuestThemeContextBridge, GuestTimeFixedBridge, GuestTimeRealBridge, GuestTimeVirtualBridge,
    GuestTransformBridge, GuestUiHandles, GuestUiQueryBuilderExt, GuestUiTextBridge,
    GuestViewVisibilityBridge, GuestVisibilityBridge, GuestWindowBridge,
    GuestWindowQueryBuilderExt, GuestWireBridge, MOD_STATE_FLAG_ACTIVE, MOD_STATE_FLAG_COMPUTED,
    MOD_STATE_FLAG_MUTABLE, MOD_STATE_FLAG_SUB_STATE, MUTE_CMD_MUTE, MUTE_CMD_NONE,
    MUTE_CMD_TOGGLE, MUTE_CMD_UNMUTE, ModDespawnOnEnter, ModDespawnOnExit, ModStateEntry,
    ModStateHandle, ModStateTransitionRequest, ORBIT_CONSTRAINT_FIXED, ORBIT_CONSTRAINT_FREE,
    OVERLAY_NAMESPACE, PLAYBACK_MODE_DESPAWN, PLAYBACK_MODE_LOOP, PLAYBACK_MODE_ONCE,
    PLAYBACK_MODE_REMOVE, SINK_CMD_NONE, SINK_CMD_PAUSE, SINK_CMD_PLAY, SINK_CMD_STOP,
    SINK_CMD_TOGGLE_PLAYBACK, STATE_ABSENT_DISCRIMINANT, SYSTEM_TELEMETRY_DIGEST_SIZE,
    SYSTEM_TELEMETRY_DIGEST_VERSION, SYSTEM_TELEMETRY_MAGIC, SystemTelemetryDigest64,
    TELEMETRY_FLAG_BUDGET_BREACHED, TELEMETRY_FLAG_FAULTED, TELEMETRY_FLAG_QUARANTINED,
    TELEMETRY_FLAG_SUMMARY_TICK_ACTIVE, TONEMAPPING_ACES_FITTED, TONEMAPPING_AGX,
    TONEMAPPING_BLENDER_FILMIC, TONEMAPPING_KHRONOS_PBR_NEUTRAL, TONEMAPPING_LINEAR,
    TONEMAPPING_NONE, TONEMAPPING_REINHARD, TONEMAPPING_REINHARD_LUMINANCE,
    TONEMAPPING_SOMEWHAT_BORING_DISPLAY_TRANSFORM, TONEMAPPING_TONY_MC_MAPFACE,
    TRANSMISSION_QUALITY_HIGH, TRANSMISSION_QUALITY_LOW, TRANSMISSION_QUALITY_MEDIUM,
    TRANSMISSION_QUALITY_ULTRA, TelemetryFrameError, TypedAssetHandle, VOLUME_DECIBELS,
    VOLUME_LINEAR, fnv1a_64_hash_system, mod_asset_path, mod_overlay_path, on_enter_schedule,
    on_exit_schedule, on_transition_schedule, resolve_audio_source_type, resolve_reflected_handle,
    resolve_scene_patch_type, resolve_settings_handle, state_set_name,
};
pub use commands::{GuestCommands, GuestTransaction};
pub use component::{GuestComponent, GuestComponentHooks, guest_component_uuid};
pub use entity_commands::{GuestChildSpawner, GuestEntityCommands};
pub use iter::{
    Added, AddedMut, Allow, Changed, ChangedMut, Chunk, ChunkIter, ChunkSlice, ChunkSliceMut,
    CopyableQueryTerm, CopyableQueryTuple, Has, Mut, MutSlice, Or, Or2, Or3, OrFilterTerm,
    OrFilterTuple, PairwiseQueryTerm, PairwiseQueryTuple, QuerySingleError, QueryTerm, QueryTuple,
    Ref, RefSlice, SparseRows, SparseRowsIter, SparseRowsMut, TypedChunkIter, With, WithAdded,
    WithChanged, Without,
};
pub use lifecycle::{GuestPlugin, build_guest_fn, build_guest_plugin};

#[cfg(test)]
mod tests {
    use super::*;
    use bamabi_guest_sys::ValidatedSlice;

    // Verify `guest_component_uuid` is const-evaluatable over `AbiPod` array types unconditionally.
    const _: () = {
        let u = guest_component_uuid::<[f32; 2]>("game::Pos", 0);
        assert!(!u.is_zero());
    };

    #[cfg(feature = "derive")]
    mod derive_smoke {
        use crate::prelude::*;

        /// Sample `#[derive(AbiPod, GuestComponent)]` fixture disabling schema-hash mixing.
        #[repr(C)]
        #[derive(Clone, Copy, Debug, PartialEq, AbiPod, GuestComponent)]
        #[guest_component(name = "demo::DerivedVec2", no_schema_hash)]
        struct DerivedVec2 {
            x: f32,
            y: f32,
        }

        /// Sample `#[derive(ModComponent)]` fixture using `guest` shorthand and `summary_tick` flag.
        #[repr(C)]
        #[derive(Clone, Copy, Debug, PartialEq, ModComponent)]
        #[mod_component(name = "demo::ShorthandVec2", summary_tick, guest)]
        struct ShorthandVec2 {
            x: f32,
            y: f32,
        }

        /// Verifies that `#[derive(GuestComponent)]` and `#[derive(ModComponent)]` with `guest`
        /// produce deterministic `ComponentUuid`s matching `ComponentUuid::from_parts` and preserve
        /// `BAM_COMPONENT_FLAG_SUMMARY_TICK`.
        #[test]
        fn derived_guest_component_uuid_is_deterministic() {
            let expected = ComponentUuid::from_parts(
                "demo::DerivedVec2",
                core::mem::size_of::<DerivedVec2>(),
                core::mem::align_of::<DerivedVec2>(),
                0,
            );
            assert_eq!(<DerivedVec2 as GuestComponent>::UUID, expected);
            assert_eq!(
                <ShorthandVec2 as GuestComponent>::UUID,
                <ShorthandVec2 as ModComponent>::UUID
            );
            assert_eq!(
                <ShorthandVec2 as GuestComponent>::FLAGS,
                BAM_COMPONENT_FLAG_SUMMARY_TICK
            );
        }
    }

    /// Verifies that invalid `ChunkSlice` and `ChunkSliceMut` wrappers report empty length and
    /// safely return `None` from typed slice conversions.
    #[test]
    fn invalid_slices_return_none_safely() {
        let slice = ChunkSlice::from_validated(ValidatedSlice::invalid());
        assert!(slice.is_empty());
        assert_eq!(slice.len(), 0);
        assert!(slice.as_typed_slice::<f32>().is_none());

        let mut mut_slice =
            ChunkSliceMut::from_validated(bamabi_guest_sys::ValidatedSliceMut::invalid());
        assert!(mut_slice.is_empty());
        assert!(mut_slice.as_typed_slice::<f32>().is_none());
        assert!(mut_slice.as_typed_slice_mut::<f32>().is_none());
    }
}
