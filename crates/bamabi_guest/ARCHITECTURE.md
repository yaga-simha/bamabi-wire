# `bamabi_guest` Architecture & Implementation Guide

`bamabi_guest` (`v0.3.0`, targeting `ABI 0.3.0` / `0x0003_0000`, Rust `edition = "2024"`, MSRV `1.87`) is the safe (`#![forbid(unsafe_code)]`), **zero-Bevy-dependency** Layer 0 guest SDK for writing native `.cdylib` mods in Rust.

Its sole runtime dependencies are:
- `bamabi_sys` (`#![no_std]`, zero dependencies): frozen C-ABI wire types, 68-slot `BamHostApi`, `ComponentUuid` (SipHash-2-4-128), `FfiResult`, and descriptors.
- `bamabi_guest_sys`: the guest-side `unsafe` boundary housing `GuestInstance`, `Host`, `ReentrantCell`, panic-catching `extern "C"` trampolines, checked `BamHostApi` wrappers, `ChunkColumnDispenser`, `ValidatedIter` / `ValidatedCommands` / `ValidatedSlice*`, and unconditional `wire_types`.
- `bamabi_macros` (optional, behind `feature = "derive"`, `default = []`): `#[derive(AbiPod, GuestComponent, ModComponent)]`.

---

## 1. Core Architectural Invariants

1. **`#![forbid(unsafe_code)]` at Crate Root (`src/lib.rs`):** Zero `unsafe` blocks exist anywhere in `bamabi_guest/src/`. Soundness relies on private/validated wrappers (`GuestApp`, `SystemContext`, `ChunkIter`, `Chunk`, `ChunkSlice`, `ChunkSliceMut`, `GuestCommands`) whose constructors only accept already-validated types from `bamabi_guest_sys` (for example `from_validated`), plus compile-time `AbiPod` / `GuestComponent` / `ModComponent` bounds and sealed query traits.
2. **Per-World Isolation (`export_mod!` + `GuestInstance`):** No process-global mutable `static` state holds mod data. Each Bevy `World` loading a `.cdylib` invokes `bamabi_init`, which heap-allocates an independent `GuestInstance` owned via `*mut BamInstance` and drops it in `bamabi_shutdown`.
3. **Panic Containment Across `extern "C"`:** Every cross-ABI function pointer uses `extern "C"` (`nounwind`). Under Cargo's default `panic = "unwind"`, `bamabi_guest_sys` wraps entry points, systems, `run_if` predicates, observers, migration callbacks, and component destructors in `catch_unwind`, dropping panic payloads on the guest heap and returning `FfiResult::PANIC_CAUGHT` (or `FfiBool::FALSE` for `run_if`).
4. **Fail-Closed Error Latching:** Builder APIs (`QueryBuilder`, `GuestEntityCommands`, `GuestChildSpawner`, `ObserverBuilder`, `TypedObserverBuilder`, `SystemBuilder`) latch `first_error: Option<FfiResult>` without panicking and return the error cleanly when finalized (`.build(...)`, `.finish()`, `.status()`).
5. **Manifest Namespace Qualification:** When `GuestComponent::NAME` is unqualified (contains no `"::"`), `qualify_component_identity` automatically prefixes `"<manifest.name>::<Name>"` and recomputes the `ComponentUuid` (unless overridden), while `guest_uuid_filter` generates a `UuidFilter` (stack-allocated up to 256 bytes, heap-fallback above) matching the qualified UUID with fallback to `T::UUID`.

---

## 2. How Safe Wrappers Delegate to `bamabi_guest_sys` Without `unsafe`

Every raw `*mut` / `*const` FFI handle is encapsulated inside `bamabi_guest_sys`:

| Guest Safe Type (`bamabi_guest`) | Backing Validated Handle (`bamabi_guest_sys`) | What `bamabi_guest_sys` Validates |
|---|---|---|
| [`GuestApp`](src/app/builder.rs) | `&'a mut GuestInstance` + `ValidatedLoadCtx<'a>` | `BamHostApi` SemVer, `struct_size` (`MIN_V0_2_SIZE` / `covers_slot`), non-null pointers |
| [`SystemContext`](src/app/context.rs) | `&'a Host` + `ValidatedSystemRef<'a>` + `[Option<ChunkIter<'a>>; MAX_QUERIES_PER_SYSTEM]` + `resource_query` + `GuestCommands<'a>` | Panic-guarded `ReentrantCell`, query slot bounds, `sys_res_ptr` / `sys_res_mut_ptr` |
| [`RunConditionContext`](src/app/context.rs) | `&'a Host` | Evaluated before per-system `BamSystemCtx` creation; exposes diagnostics, component/symbol resolution |
| [`ChunkIter`](src/iter/chunk_iter.rs) | `ValidatedIter<'a>` | Non-null `*mut BamIter`, active chunk cursor lifetime, `AccessMode` / `BamColumnInfo` checks |
| [`Chunk`](src/iter/chunk.rs) | `&'iter mut ValidatedIter<'ctx>` + `len` | Borrow checker ties slices to `&self` / `&mut self`, preventing overlapping mutable aliases or advancing mid-borrow |
| [`ChunkSlice`](src/iter/slice.rs) / [`ChunkSliceMut`](src/iter/slice.rs) | `ValidatedSlice<'a>` / `ValidatedSliceMut<'a>` | Alignment, element size, `isize::MAX` byte-length cap, ZST `stride == 0` safety, `UuidFilter` |
| [`GuestCommands`](src/commands.rs) | `ValidatedCommands<'a>` | Non-null `*mut BamCommands`, `FfiEntity` non-null checks, stack `<= 16` / heap `> 16` `SpawnEntry` marshalling |

### Zero-`unsafe` Command Re-Wrapping in Observers (`with_guest_commands`)

In [`src/app/observers.rs`](src/app/observers.rs), observer trampolines from `bamabi_guest_sys` pass `&mut ValidatedCommands<'_>`. Because `GuestCommands<'a>` wraps `ValidatedCommands<'a>` by value, `with_guest_commands` creates a temporary `GuestCommands::from_validated(ValidatedCommands::empty(host))`, swaps the inner `ValidatedCommands` in via `core::mem::swap`, executes the user closure, and swaps it back—providing an idiomatic `&mut GuestCommands<'_>` with zero `unsafe` code.

### Slot-Checked ABI `0.3.0` Extension & `0.2.0` Fallback

`bamabi_guest_sys` checks `BamHostApi::covers_slot(offset_of!(...), size_of_val(...))` before calling any ABI `0.3.0` slot (slots 50–67). Missing or truncated slots return `Err(FfiResult::UNSUPPORTED)`, allowing `bamabi_guest` helpers such as `TypedChunkIter::single_chunk` (falling back to `reset` + `next_chunk` counting when `iter_count` is `UNSUPPORTED`) and `GuestCommandsAudioExt::spawn_audio` (falling back to `spawn_empty` + `insert` with despawn-on-error) to degrade gracefully on `0.2.0` hosts.

---

## 3. Module-by-Module Map

### Root Modules (`src/`)

| File | Responsibilities |
|---|---|
| [`src/lib.rs`](src/lib.rs) | Enforces `#![forbid(unsafe_code)]` and `#![warn(missing_docs)]`; provides the 8-step onboarding guide; re-exports `bamabi_sys`, `SpawnEntry`, `#[derive(AbiPod, GuestComponent, ModComponent)]` (under `feature = "derive"`), and all top-level types/bridges; hosts unit smoke tests (`derive_smoke`, `invalid_slices_return_none_safely`). |
| [`src/macros.rs`](src/macros.rs) | Defines `export_mod!` (4 forms: `plugin: Type`, `name:version:fn`, `fn`, and `manifest:init:shutdown`) generating `#[unsafe(no_mangle)]` `bamabi_manifest`, `bamabi_init`, `bamabi_shutdown`, and `bamabi_resolve_symbol`; defines `guest_log!` with a zero-allocation fast path via `core::fmt::Arguments::as_str`. |
| [`src/prelude.rs`](src/prelude.rs) | Curated wildcard import (`use bamabi_guest::prelude::*;`) covering traits, builders, bridges, ABI constants, macros, and optional derives. |
| [`src/raw.rs`](src/raw.rs) | Re-exports `bamabi_sys::*` and low-level `bamabi_guest_sys` items (`GuestInstance`, `Host`, `Validated*`, `ChunkColumnDispenser`, `init_guest_instance`, `shutdown_guest_instance`, `register_*`, `migrate_*`) plus `*RawExt` pointer inspection traits (`GuestAppRawExt`, `SystemContextRawExt`, `ChunkIterRawExt`, `GuestCommandsRawExt`, `ChunkSliceRawExt`, `ChunkSliceMutRawExt`). |
| [`src/lifecycle.rs`](src/lifecycle.rs) | Defines `GuestPlugin` (`NAME`, `VERSION`, `FLAGS`, `MANIFEST`, `build`, `save_state`, `restore_state`, `on_shutdown`), `build_guest_plugin` (wrapping `P` in `Arc<ReentrantCell<P>>` per `GuestInstance`), and `build_guest_fn`. |
| [`src/component.rs`](src/component.rs) | Defines `GuestComponent` (with `FfiChildOf` and `FfiDisabled` built-in impls), `GuestComponentHooks<T>` (5 lifecycle hooks, `enable_clone`, `required_components`, `schema_fingerprint`, `fields`, `required_init_descs`, `from_mod_component`), `guest_component_uuid`, `qualify_component_identity`, `resolve_guest_component`, and `guest_uuid_filter`. |
| [`src/commands.rs`](src/commands.rs) | Defines `GuestCommands<'a>` (`spawn_empty`, `spawn`, `despawn`, `insert`, `insert_default`, `remove`, typed `*_component*` helpers, `trigger` / `trigger_entity` / `trigger_handle`, `spawn_batch` / `spawn_batch_into`, `despawn_all`, `insert_if_new`, `insert_relation` / `insert_relation_with_payload` / `remove_relation`, `checkpoint`, `rollback`, `begin_transaction`, `transaction`) and RAII `GuestTransaction<'a>`. |
| [`src/commands_lifecycle.rs`](src/commands_lifecycle.rs) | Extends `GuestApp`, `GuestCommands`, `QueryBuilder`, `Chunk`, and `ChunkIter` for persistent spawns (`spawn_empty_persistent`, `spawn_persistent`) and non-POD `ModComponent` registration, dynamic moves (`insert_dynamic`, `spawn_dynamic`, `spawn_with_dynamic`), dynamic query terms (`*_dynamic`), and same-mod VTable-verified `dynamic_component*` chunk slices. |
| [`src/entity_commands.rs`](src/entity_commands.rs) | Defines fluent `GuestEntityCommands<'cmd, 'a>` (entity-scoped `.insert*`, `.remove*`, `.insert_relation*`, `.set_parent`/`.remove_parent`/`.add_child`/`.with_children`, `.disable`/`.enable`, `.despawn_on_*`, `.observe`/`.observe_mut`, `.trigger`/`.trigger_propagated`, `.despawn`) and `GuestChildSpawner<'cmd, 'a>`, plus `GuestCommands::{entity, spawn_entity*, add_children, disable_entity, enable_entity}`. Note: `disable_entity` tries `insert_default` first so bridged immutable `Disabled` succeeds and falls back to raw ZST `insert` only on `PERMISSION_DENIED`. |

### Application, Scheduling & Contexts (`src/app/`)

| File | Responsibilities |
|---|---|
| [`src/app/mod.rs`](src/app/mod.rs) | Re-exports `builder`, `conditions`, `context`, `hierarchy`, `observers`, `schedules`, and `v03` public APIs. |
| [`src/app/builder.rs`](src/app/builder.rs) | Defines `IntoSystemResult` (sealed for `FfiResult`, `()`, `Result<(), FfiResult>`), `GuestComponentOptions<T>`, `GuestApp<'a>` (component registration, observer registration, hot-reload hooks, `alloc_user_data`, `on_shutdown`, `.system(name)`), `QueryBuilder<'b>` (generated typed + handle terms, `or_group`, `allow/with/without/has_disabled`, `term_named`), and `SystemBuilder<'b, 'a>` (`query`, `query_typed`, `resource_*`, `message_*`, `removed*`, `schedule`, `after`/`before`/`in_set`/`after_weak`/`before_weak`/`ambiguous_with*`, `main_thread`, chainable AND-composed `run_if`/`run_if_fn`, `build`, `build_with_local`, `register_handle`, `register_handle_with_local`). |
| [`src/app/context.rs`](src/app/context.rs) | Defines `RunConditionContext<'a>`, `GuestRes<'a, R>`, `GuestResMut<'a, R>` (with `BamChangeTicks` `is_added`/`is_changed`), and `SystemContext<'a>` (`iter`, `query`/`query_mut`, `queries_2_mut`, `queries_3_mut`, `iter_and_commands`, `queries_2_and_commands`, `queries_3_and_commands`, `resource*`, `res_by_handle`, `res_with_ticks`, `par_chunks`, `single_query*`, `query_typed`, `queries_2_typed`, `queries_3_typed`, `get`/`get_mut`/`get_pod`/`get_dynamic`/`get_many`, POD & non-POD `read_messages`/`send_message`). |
| [`src/app/schedules.rs`](src/app/schedules.rs) | Defines `BevySchedules` mapping all 16 Bevy `0.20` schedules onto ABI `0.3.0` built-in IDs `0..=5` (`Startup`, `PreUpdate`, `FixedUpdate`, `Update`, `PostUpdate`, `Last`) plus 10 deterministic offsets from `custom_base >= ScheduleId::CUSTOM_BASE` (`1024`). |
| [`src/app/conditions.rs`](src/app/conditions.rs) | Defines composable Bevy-parity run conditions: `run_once`, `not`, `and`, `or`, `xor`, `every_n_ticks`, `component_registered`, `component_registered_named`, `symbol_exists`. |
| [`src/app/hierarchy.rs`](src/app/hierarchy.rs) | Adds `SystemContext::{children, relationship_targets, iter_descendants, iter_descendants_of}` (cycle-safe BFS), `QueryBuilder::read_children`, `GuestCommands::{add_child, remove_parent, spawn_child}`, and `GuestApp::register_relationship_pair` (`__bam_rel_bind::<rel>::<target>`). |
| [`src/app/observers.rs`](src/app/observers.rs) | Adds `GuestApp::{add_observer, observe_lifecycle, observe_add/insert/discard/remove/despawn, observe_ready*, observe_event*, observe_mut*}`, runtime `GuestCommands` observers & propagated triggers (`trigger_propagated*`, `trigger_ready*`), and fluent `ObserverBuilder` + `TypedObserverBuilder<E>` with `.with_target`, `.with_propagate`, `.with_relationship*`, and chainable `.run_if` / `.run_if_fn`. |
| [`src/app/v03.rs`](src/app/v03.rs) | Implements ABI `0.3.0` (slots 50–67) safe builders and context helpers: `RelationDescBuilder` (`exclusive`, `symmetric`, `acyclic`, `cascade_despawn`, `cleanup_remove`, `max_degree`, `with_payload`), `ScheduleSetBuilder` (`sequence`/`chain`, `sequence_weak`, `parallel`, `before*`/`after*`), `GuestApp::{register_relation, configure_schedule_set, mount_system, migrate_component_column, migrate_pod_component_column}`, `verify_component_layout*`, host forwarders via `impl_v03_host_forwarders!`, and `SystemContext::{par_chunks_readonly, par_try_chunks_readonly}`. |

### Iteration & Typed Queries (`src/iter/`)

| File | Responsibilities |
|---|---|
| [`src/iter/mod.rs`](src/iter/mod.rs) | Re-exports `SparseRows`, `SparseRowsIter`, `SparseRowsMut`, `Chunk`, `ChunkIter`, `ChunkSlice`, `ChunkSliceMut`, `QueryTerm`, `QueryTuple`, `TypedChunkIter`, `QuerySingleError`, `With`, `Without`, `Changed`, `Added`, `Allow`, `Has`, `Or`, `Or2`, `Or3`, `OrFilterTerm`, `OrFilterTuple`. |
| [`src/iter/typed_query.rs`](src/iter/typed_query.rs) | Implements sealed 1..=16-arity `QueryTuple` and 2..=8-branch `Or<(T0..T7)>` (`Or2`, `Or3`), `TypedChunkIter`, `single_chunk` (with ABI `0.2.0` multi-chunk fallback), `up_cascade*`, `for_each`, `par_for_each`, and `par_for_each_chunk_readonly`. |
| [`src/iter/filters.rs`](src/iter/filters.rs) | Implements `Chunk::{has, has_pod, has_untyped}`, `ChunkIter::{read_removed*, drain_removed}`, and `SystemContext::{read_removed*, query_read_removed*, read_removed_raw*}` (with `validate_removed_term` checking `REMOVED_READ`, non-resource/non-message, layout, and UUID). |
| [`src/iter/chunk_iter.rs`](src/iter/chunk_iter.rs) | Defines `ChunkIter<'a>` (`next_chunk`, `next_chunk_view`, `reset`, `count`, `is_empty`, `up_cascade*`, `column_info`, `entities`, `readonly_slice`, `mutable_slice*`, `mark_changed*`, `rows*`, `resource*`, `par_for_each*`, `par_for_each_chunk_readonly`, `typed`, POD & non-POD `read_*messages` / `send_*message`). |
| [`src/iter/chunk.rs`](src/iter/chunk.rs) | Defines `Chunk<'iter, 'ctx>` enforcing Rust aliasing via `&self` vs `&mut self` (`column*`, `component*`, `column_mut_untracked`, `mark_changed*`, split-borrow `read_write` / `component_read_write` / `write_2` / `component_write_2` / `entities_and_*`, `rows*`, `typed::<Q>()`). |
| [`src/iter/slice.rs`](src/iter/slice.rs) | Defines `ChunkSlice<'a>` and `ChunkSliceMut<'a>` delegating bounds/alignment/UUID validation to `ValidatedSlice*`. |

### Subsystem Bridges (`src/bridges/`)

All 12 bridge modules are unconditional (available under `--no-default-features`) and wrap `bamabi_guest_sys::wire_types` + VTable bridges:

| File | Role | Host Convention |
|---|---|---|
| [`src/bridges/mod.rs`](src/bridges/mod.rs) | Re-exports all bridge symbols and defines `GuestBridgesHostExt` for `Host`, `GuestApp`, and `SystemContext`. | Both |
| [`src/bridges/std_types.rs`](src/bridges/std_types.rs) | `GuestNameBridge`, `GuestWireBridge<W>`, `GuestStdHandles`, `GuestStdTypes`, `GuestStdQueryBuilderExt`, `GuestStdCommandsExt`, `GuestStdSystemBuilderExt`, `GuestGamepadRumbleExt`, 194-key `FfiKeyCode`, `Transform`/`GlobalTransform`/`Visibility`/`Disabled`/`Time<*>`/`ButtonInput<*>`/`Mouse*`/`Gamepad*`. | Built-in (`StdTypesBridgePlugin`, `ModInputExtBridgePlugin`; `InheritedVisibility`/`ViewVisibility` host-opt-in) |
| [`src/bridges/asset.rs`](src/bridges/asset.rs) | Validated `mod://<ns>/<rel>` (`mod_asset_path`) and `mod://overlay/<rel>` (`mod_overlay_path`, `OVERLAY_NAMESPACE`), `GuestAssetHandle`, `TypedAssetHandle<A>`, `GuestAssetServer`, `GuestAssetHostExt`, and `FfiAssetEvent`. | Built-in (`AssetLoad` slots; `FfiAssetEvent` host-opt-in) |
| [`src/bridges/audio.rs`](src/bridges/audio.rs) | `FfiAudioPlaybackSettings`, `FfiAudioPlayerSpec`, `FfiSpatialAudioSinkControl`, `resolve_audio_source_type`, `GuestAudioQueryBuilderExt`, `GuestCommandsAudioExt` (atomic `spawn_audio` with `0.2.0` orphan-free fallback). | Built-in (`ModAudioBridgePlugin`) |
| [`src/bridges/bsn.rs`](src/bridges/bsn.rs) | `GuestScene`, `resolve_scene_patch_type`, `GuestCommandsBsnExt` (`cmd_spawn_scene`). | Built-in (`ModBsnPlugin`) |
| [`src/bridges/camera_light.rs`](src/bridges/camera_light.rs) | `FfiCamera2d`, `FfiCamera3d`, `FfiPointLight`, `FfiSpotLight`, `FfiDirectionalLight`, `FfiPanOrbitCamera`, `FfiSolariLighting`, `GuestCameraLightQueryBuilderExt`, `GuestCommandsCameraLightExt`. | Built-in (`ModCameraLightBridgePlugin`) |
| [`src/bridges/picking.rs`](src/bridges/picking.rs) | Flattened `FfiPointerPress/Release/Move/Click/Over/Out/Drag`, `FfiEntityCursor`, `GuestAppPickingExt`, `GuestPickingQueryBuilderExt`, `GuestCommandsPickingExt`. | Built-in (`ModPickingBridgePlugin`) |
| [`src/bridges/ui.rs`](src/bridges/ui.rs) | `GuestMappedStringBridge` (`Text`, `TextSpan`), `GuestWireBridge` aliases (`BackgroundColor`, `BorderColor`, `RemSize`, `EmSize`, `ThemeContext`), `GuestUiHandles` (optional `theme_context` failing closed on `TYPE_MISMATCH`), `GuestAppUiExt` (`observe_value_change`, `observe_activate`), `GuestUiQueryBuilderExt`, `GuestCommandsUiExt` (`insert_default_*`, `trigger_activate*`), Feathers/text POD types. | Built-in (`ModUiBridgePlugin`) |
| [`src/bridges/state.rs`](src/bridges/state.rs) | Slot-0 `GuestStateBridge` + unbounded `ModStateSlab` (`ModStateHandle`, `ModStateEntry`, `ModStateTransitionRequest`, `ModDespawnOnExit`, `ModDespawnOnEnter`), `state_set_name`, `on_enter_schedule` / `on_exit_schedule` / `on_transition_schedule`, `GuestStateHandles`, `GuestStateSystemBuilderExt`, `GuestStateSlabContextExt`, `GuestCommandsStateExt`, `GuestStateQueryBuilderExt`. | Built-in (`ModStatePlugin`, `ModStateSlab`) |
| [`src/bridges/reflect.rs`](src/bridges/reflect.rs) | `BRP_METHOD_*` (`bamabi/*`, `bam/*`, `world.*`, `registry.schema`, `rpc.discover`), `resolve_reflected_handle`, `resolve_settings_handle`, and 64-byte `"BAMT"` `SystemTelemetryDigest64` (`fnv1a_64_hash_system`). | Built-in (`bamabi_reflect`) |
| [`src/bridges/gizmos.rs`](src/bridges/gizmos.rs) | `FfiGizmoLine/Ray/Circle/Sphere/Rect/Cuboid/Arrow` and `GuestCommandsGizmosExt`. | Host-opt-in |
| [`src/bridges/render.rs`](src/bridges/render.rs) | `GuestSpriteBridge`, `GuestStandardMaterialBridge`, `FfiSprite`, `FfiStandardMaterial`, `FfiWeslMaterialSpec`, `FfiSpriteMaterialSpec`, `FfiMesh2d`, `FfiMesh3d`, `GuestRenderQueryBuilderExt`, `GuestCommandsRenderExt`. | Host-opt-in |
| [`src/bridges/window.rs`](src/bridges/window.rs) | `GuestWindowBridge`, `FfiWindow`, `FfiWindowMode`, `FfiWindowResized`, `FfiWindowCloseRequested`, `FfiAppExit`, `GuestWindowQueryBuilderExt`, `GuestAppExitExt`. | Host-opt-in |

---

## 4. Deep Dive: How `QueryTuple` Seals & Validates 1..=16 Terms

In [`src/iter/typed_query.rs`](src/iter/typed_query.rs):

1. **Private Sealing Traits (`mod private`):** `TermSealed`, `TupleSealed`, `OrFilterSealed`, and `OrFilterTupleSealed` prevent external crates from implementing `QueryTerm`, `QueryTuple`, `OrFilterTerm`, or `OrFilterTuple`—guaranteeing every term maps 1-to-1 to a valid `AccessMode` and `ChunkColumnDispenser` consumption order.
2. **Supported Terms (`QueryTerm`):**
   - `FfiEntity` (`&[FfiEntity]`, pushes 0 `BamTermDesc`s)
   - `&T` (`&[T]`, `READ`), `&mut T` (`&mut [T]`, `WRITE`, `IS_READ_ONLY = false`)
   - `Option<&T>` (`Option<&[T]>`, `OPTIONAL_READ`), `Option<&mut T>` (`Option<&mut [T]>`, `OPTIONAL_WRITE`, `IS_READ_ONLY = false`)
   - `With<T>` (`()`, `FILTER_ONLY`), `Without<T>` (`()`, `WITHOUT`), `Allow<T>` (`()`, `ALLOW`)
   - `Changed<T>` (`&[T]`, `CHANGED`), `Added<T>` (`&[T]`, `ADDED`)
   - `Has<T>` (`bool`, `HAS`)
   - `Or<(T0, ...)>` (`()`, 2..=8 `OrFilterTerm` branches over `With`/`Without`/`Changed`/`Added`; `Or2`/`Or3` aliases)
3. **Compile-Time `IS_READ_ONLY` Propagation:** `QueryTuple::IS_READ_ONLY` is the logical AND of every term's `IS_READ_ONLY`. `TypedChunkIter::par_for_each_chunk_readonly` rejects any tuple with `IS_READ_ONLY == false` with `FfiResult::PERMISSION_DENIED` before crossing FFI.
4. **Disjunctive `or_group` Allocation:** `Or<T>::push_desc` calls `next_or_group_id(out)` (`max(existing or_group) + 1`, capped at `255` via `u8::checked_add`) and stamps every branch's `BamTermDesc::or_group`.
5. **Runtime Sequential Validation:** At registration, `QueryTuple::collect_descs` resolves every `GuestComponent` and enforces `MAX_TERMS_PER_QUERY` (16). Per chunk, `ChunkColumnDispenser` walks `BamTermDesc`s in exact declaration order, checking `BamColumnInfo` (`size`, `align`, `mode`, storage flags, and manifest-aware `guest_uuid_filter::<T>`) and checking write-alias disjointness across columns.

---

## 5. Deep Dive: How `GuestTransaction` RAII Rollback Works

In [`src/commands.rs`](src/commands.rs):

1. `GuestCommands::begin_transaction(&mut self)` constructs a reborrowed `GuestCommands` via `commands.inner.reborrow()` and calls `cmd_checkpoint` (ABI `0.3.0` slot 63) to capture an opaque `checkpoint: usize` token.
2. `GuestTransaction<'a>` stores `{ commands: GuestCommands<'a>, checkpoint: usize, settled: bool }` and implements `Deref<Target = GuestCommands<'a>>` and `DerefMut`, so every standard command method (`spawn`, `insert`, `entity`, `insert_relation`, etc.) queues directly inside the transaction scope.
3. **Explicit Commit:** `tx.commit()` sets `self.settled = true` and consumes `tx`.
4. **Explicit Rollback:** `tx.rollback()` sets `self.settled = true` and calls `cmd_rollback(checkpoint)` (slot 64), returning the host's `FfiResult`.
5. **RAII Drop Auto-Rollback:** If `tx` is dropped with `settled == false` (e.g., via `?` early return or panic unwind inside `catch_unwind`), `Drop::drop` sets `self.settled = true` and invokes `cmd_rollback(self.checkpoint)`.
6. `GuestCommands::transaction(|tx| ...)` wraps `begin_transaction`, commits on `Ok(val)`, and rolls back on `Err(err)`.

---

## 6. How to Add a New Bridge or Builder Method

### Adding a New Subsystem Bridge Module
1. **Define wire structs in `bamabi_guest_sys::wire_types`** (`#[repr(C)]`, `AbiPod`, field schema hashes matching `bamabi_bridges` / Bevy `0.20`).
2. **Create `crates/bamabi_guest/src/bridges/<name>.rs`** (with `#![forbid(unsafe_code)]` inherited from `lib.rs`):
   - Re-export the `Ffi*` wire types and constants from `bamabi_guest_sys::wire_types`.
   - Implement `GuestComponent` for each direct POD component/event/message (`const NAME: &'static CStr = c"..."; const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;`).
   - For opaque host Bevy types exposed via VTable, alias `GuestWireBridge<FfiType>` or wrap `GuestMappedStringBridge` and provide `resolve_*` helpers calling `GuestWireBridge::resolve` with canonical + alias `"bam.v0.<Name>.vtable"` symbols.
   - Define `Guest<Name>QueryBuilderExt`, `GuestCommands<Name>Ext`, or `GuestApp<Name>Ext` extension traits delegating to `QueryBuilder::term_named`, `GuestCommands::insert_component`, `GuestApp::observe_event`, etc.
3. **Wire exports:** Add `pub mod <name>;` and `pub use <name>::{...};` in [`src/bridges/mod.rs`](src/bridges/mod.rs), [`src/lib.rs`](src/lib.rs), and [`src/prelude.rs`](src/prelude.rs). Add any convenience resolver to `GuestBridgesHostExt`.
4. **Verify parity:** Add layout/UUID/default parity assertions in [`tests/bevy_020_parity_tests.rs`](tests/bevy_020_parity_tests.rs) (including `--no-default-features` and `--all-features`).

### Adding a New Builder Method on `SystemBuilder` / `QueryBuilder` / `GuestEntityCommands`
1. **Latch errors, never panic:** On `QueryBuilder`, `SystemBuilder`, `GuestEntityCommands`, `ObserverBuilder`, or `TypedObserverBuilder`, record the first non-`OK` `FfiResult` in `first_error` instead of returning `Result` mid-chain or calling `.unwrap()`.
2. **Preserve `#[inline]` delegation:** Forward through `bamabi_guest_sys` validated methods rather than adding raw pointer logic in `bamabi_guest`.
3. **Document & test:** Add `///` doc comments (satisfying `#![warn(missing_docs)]` and `RUSTDOCFLAGS="-D warnings"`) and cover both happy-path and fail-closed error-latching cases in `tests/`.

---

## 7. Setup, Building a `.cdylib` Mod, Running, Testing & Benchmarking

### Setup & Building a `.cdylib` Mod

In your guest mod crate's `Cargo.toml`:
```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
bamabi_guest = { git = "https://github.com/yaga-simha/bamabi-wire.git", branch = "main", features = ["derive"] }
```

Build the native dynamic library:
```bash
cargo build --release -p <your_mod_crate>
# Output: target/release/lib<mod_name>.dylib (macOS), .so (Linux), or <mod_name>.dll (Windows)
```

For minimal / macro-free guests across constrained environments, omit `features = ["derive"]` (`default = []`) and implement `AbiPod` + `GuestComponent` manually (or use built-in bridge types).

### Loading & Running in a Host

`export_mod!` exports the four ABI `0.3.0` entry points expected by a `bamabi` host (`bamabi-core` / `bamabi` / `cargo-bam` in `bamabi-tools`):
- `bamabi_manifest() -> *const BamManifest`
- `bamabi_init(api, load_ctx, instance_out) -> FfiResult`
- `bamabi_shutdown(instance)`
- `bamabi_resolve_symbol(instance, name) -> *const c_void` (for `"bam.v0.*"` symbols)

End-to-end runnable host + `.cdylib` mod examples live in Layer 4 (`bamabi-showcase`).

### Testing & Verification

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

### Stress Testing & Benchmarking Notes

- **`bamabi-wire` has no Criterion `[[bench]]` targets:** Because `bamabi-wire` is Layer 0 (zero-Bevy wire contract and guest SDK), runtime ECS/FFI throughput benchmarks live in Layer 4 (`bamabi-showcase`, `docs/BENCHMARKS.md`).
- **Command-Queue Stress Test:** [`tests/command_queue_stress.rs`](tests/command_queue_stress.rs) exercises the `<= 16`-entry stack fast path vs. the `> 16`-entry heap fallback (`20` components) in batched `GuestCommands::spawn`, ZSTs, and intentionally uninitialized `#[repr(C)]` padding bytes (`PaddedPayload`) under both standard tests and Miri.
- **Per-System Telemetry Frames (`"BAMT"`):** [`src/bridges/reflect.rs`](src/bridges/reflect.rs) exports `SystemTelemetryDigest64` (64-byte little-endian `"BAMT"` frame carrying `p50_us`, `p99_us`, `matched_entities`, `matched_tables`, `skipped_summary_tick_tables`, `chunks`, `commands_queued`, and `TELEMETRY_FLAG_*` bits) so guest mods and LSP/diagnostic tools can inspect host-emitted system performance digests without Bevy dependencies.
