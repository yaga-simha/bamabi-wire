# `bamabi_guest` Test & Verification Guide

`bamabi_guest` proves safety (`#![forbid(unsafe_code)]`), ABI `0.2.0` / `0.3.0` wire compatibility, Bevy `0.20` feature parity, and Miri/Tree-Borrows soundness across **unit tests**, **7 integration test suites** backed by mock host worlds, and **compile-fail / compile-pass doctests** under both `--no-default-features` and `--all-features`.

---

## Running the Test Suites

```bash
# Default (macro-free, --no-default-features)
cargo test -p bamabi_guest --no-default-features

# All features (enables `derive` / `bamabi_macros`)
cargo test -p bamabi_guest --all-features

# Doctests (compile-pass & compile-fail soundness proofs)
cargo test --doc -p bamabi_guest --no-default-features
cargo test --doc -p bamabi_guest --all-features

# Miri Tree Borrows + Strict Provenance
MIRIFLAGS="-Zmiri-tree-borrows -Zmiri-strict-provenance" cargo +nightly miri test -p bamabi_guest
```

---

## 1. Mock Host Harnesses

Because `bamabi-wire` is Layer 0 (zero Bevy dependencies), integration tests run against deterministic in-process C-ABI mock hosts that emulate a real `bamabi` host without `bevy_ecs`:

| Harness | Location | Scope |
|---|---|---|
| `MockHostWorld` / `TestHost` | [`tests/common/mod.rs`](tests/common/mod.rs) | Shared ABI `0.2.0` mock world implementing `BamHostApi` slots for component registration/resolution, chunk iteration, dense & sparse-set columns, untracked mutation + `mark_changed_range`, singleton resources, POD messages, random `sys_entity_get*`, parallel chunk dispatch, and `component_layout_info`. |
| `MockV03World` / `V03TestHost` | [`tests/v03_guest_sdk_tests.rs`](tests/v03_guest_sdk_tests.rs) | Full 68-slot ABI `0.3.0` mock world implementing diagnostics (`log_diagnostic`, `report_guest_fault`), UUID resolution, schema/field layout info, typed relationships with cycle/degree enforcement, symbol interning, named objects, `iter_count`, `iter_up_cascade`, `par_for_each_chunk_readonly`, `register_system_handle`, `configure_schedule_set`, `mount_system_in_schedule`, `cmd_checkpoint` / `cmd_rollback`, and `migrate_component_column`. |
| `MockHost` / `MockSysCtxState` | [`tests/bevy_020_parity_tests.rs`](tests/bevy_020_parity_tests.rs) | Full-featured ABI `0.3.0` mock world emulating `StdTypesBridgePlugin` (including `DisabledBridge` denying raw `cmd_insert`), `ModUiBridgePlugin` string/wire VTables, `ModStateSlab`, POD and witness-backed non-POD message streams, `Children` / `RelationshipTarget` adjacency lists, `RemovedComponents` cursors, and observers. |

---

## 2. Integration Test Suites (`crates/bamabi_guest/tests/`)

### [`tests/guest_sdk_integration_tests.rs`](tests/guest_sdk_integration_tests.rs)

Verifies core `ABI 0.2.0` guest runtime invariants, typed queries, panic containment, and lifecycle hooks (`#![deny(unsafe_code)]` at file level, with localized `#[allow(unsafe_code)]` only on raw trampoline invocations):

| Test | Why It Exists & What It Proves |
|---|---|
| `reentrant_trampoline_conflict` | Proves lock-free `ReentrantCell` returns `FfiResult::ACCESS_CONFLICT` on re-entrant system invocation and `FfiBool::FALSE` on re-entrant `run_if`, and that an RAII guard clears active state after a panicking system (`PANIC_CAUGHT`) so subsequent ticks still execute. |
| `column_uuid_mismatch_returns_none` | Proves `Chunk` and `SystemContext::component_resource*` reject layout-identical imposters (`ImposterPos` vs `Pos`), wrong element sizes/alignments, mutable borrows of `READ`-only columns, and mutable borrows of `BAM_COMPONENT_FLAG_IMMUTABLE` components. |
| `unified_system_builder_compiles_queries_resources_ordering_and_flags` | Proves `SystemBuilder` compiles dense/filter/tick/optional query terms, system-level resources, schedule selection, `after`/`before` ordering, `main_thread`, and `run_if_fn` into a valid `BamSystemDesc`, and latches `CAPACITY_EXCEEDED` (> 8 queries) and `NOT_FOUND` cleanly. |
| `sparse_rows_and_untracked_change_marking_work_end_to_end` | Proves `SparseSet` columns reject contiguous dense casts (`column`/`column_mut` return `None`) while `component_rows`/`component_rows_mut` succeed, and proves `component_mut_untracked` + `mark_changed` leaves whole-chunk change flags unset while rejecting out-of-range `mark_changed_range`. |
| `host_handshake_custom_symbols_and_vtable_validation` | Proves `init_guest_instance` rejects under-sized `BamHostApi` (`struct_size < MIN_V0_2_SIZE`) with `INVALID_VERSION` and nulls `*instance_out`, retrieves `last_error`, resolves `"bam.v0.*"` symbols, and rejects unprefixed (`INVALID_ARGUMENT`) or missing (`NOT_FOUND`) symbols. |
| `derive_mod_component_vtable_header_roundtrip` *(derive)* | Proves `#[derive(ModComponent)]` exports a valid `VTableHeader`-prefixed VTable whose slots round-trip via `Host::resolve_vtable` and fail with `TYPE_MISMATCH` on wrong UUID. |
| `typed_tuple_queries_and_par_chunks_without_magic_indices` | Proves 8-arity `QueryTuple` (`FfiEntity`, `&mut`, `&`, `Option<&mut>`, `Changed`, `Added`, `With`, `Without`) dispenses slices without magic indices, rejects wrong UUIDs with `TYPE_MISMATCH`, runs `par_for_each`, and contains worker panics inside `par_chunks` as `PANIC_CAUGHT`. |
| `state_messages_random_get_and_hooks` | Proves `GuestPlugin::save_state`/`restore_state` byte round-trips across instances, `GuestComponentHooks::{on_add, on_remove}` trampolines, independent multi-reader message cursors, and filter/access/UUID-checked `SystemContext::{get, get_mut}`. |
| `guest_sdk_child_of_ticks_and_in_set_smoke` | Proves `FfiChildOf` name/UUID parity and `GuestRes` / `GuestResMut` `is_added()` / `is_changed()` tick logic and deref mutation. |

### [`tests/command_queue_stress.rs`](tests/command_queue_stress.rs)

| Test | Why It Exists & What It Proves |
|---|---|
| `handle_commands_and_batched_spawn_fast_and_heap_paths` | Stress-tests `GuestCommands` (`#![deny(unsafe_code)]`) across `spawn_empty`, `PaddedPayload` (3 bytes of uninitialized alignment padding—proving Miri uninit-padding soundness), ZST `MarkerZst`, `insert_default` (including `PERMISSION_DENIED` when host lacks `Default`), `remove`, typed `*_component*` helpers, `spawn(&[])`, the `<= 16` stack fast path, the `> 16` heap fallback (`20` components), `despawn`, and invalid-handle / `FfiEntity::NULL` error rejections. |

### [`tests/derive_and_run_if_tests.rs`](tests/derive_and_run_if_tests.rs)

| Test | Why It Exists & What It Proves |
|---|---|
| `system_builder_chained_run_if_composes_and_drops_cleanly` | Proves chaining multiple `.run_if` / `.run_if_fn` predicates on `SystemBuilder` composes with short-circuit logical AND without overwriting earlier closures, and proves all captured predicates drop cleanly on `shutdown_guest_instance` with zero leaks. |
| `observer_builder_run_if_chains_controls_propagation_and_catches_panics` | Proves `ObserverBuilder` chained `.run_if` / `.run_if_fn` short-circuits, leaves `*propagate == TRUE` untouched when a `build_stoppable` observer is gated off while clearing it when active, and contains predicate panics as `PANIC_CAUGHT`. |
| `derive_guest_component_tests::uuid_parity_flags_and_host_registration_match_mod_component` *(derive)* | Proves bit-for-bit `NAME`, `UUID`, and `FLAGS` parity between `#[derive(GuestComponent)]`, `#[mod_component(..., guest)]`, `#[derive(ModComponent, GuestComponent)]`, and host `#[derive(ModComponent)]` across named, tuple, unit, nested, `no_schema_hash`, explicit-UUID, and manifest-auto-prefixed components. |

### [`tests/v03_guest_sdk_tests.rs`](tests/v03_guest_sdk_tests.rs)

Exercises all 18 ABI `0.3.0` host slots (`50..=67`) plus graceful `0.2.0` degradation and max-arity typed queries:

| Test | Why It Exists & What It Proves |
|---|---|
| `abi_v02_truncated_and_missing_slots_return_unsupported` | Proves every ABI `0.3.0` wrapper cleanly returns `Err(FfiResult::UNSUPPORTED)` against both a truncated-`struct_size` `0.2.0` host and a full-size host with null slots `50..=67`. |
| `diagnostics_guest_log_and_report_fault` | Proves `guest_log!` / `log_diagnostic` (slot 50) and `report_fault` (slot 51) work across `GuestApp`, `SystemContext`, and `RunConditionContext`. |
| `resolve_by_uuid_layout_info_and_verify_layout` | Proves `resolve_component_by_uuid` (slot 52), `component_layout_info` (slot 53), and `verify_component_layout` pass on matching schemas and reject fingerprint, size, or alignment drift with `TYPE_MISMATCH`. |
| `derived_mod_component_schema_fingerprint_and_fields_verified` *(derive)* | Proves `#[derive(ModComponent)]` emits non-zero `SCHEMA_FINGERPRINT` and `BAM_FIELDS` descriptors accepted by host `verify_component_layout`. |
| `relations_builder_commands_and_cycle_detection` | Proves `RelationDescBuilder` + `register_relation` (slot 54), `insert_relation*` (slot 65), and `remove_relation` (slot 66) enforce exclusivity, `max_degree`, payload edges, and `CYCLE_DETECTED` on acyclic graphs. |
| `symbol_interning_and_named_object_binding` | Proves `intern_symbol` (slot 55) deduplicates identical symbols and `bind_named_object` (slot 56) rejects invalid arguments while binding entities across `GuestApp` and `SystemContext`. |
| `iter_count_up_cascade_and_par_readonly` | Proves `iter_count` (slot 57) is non-destructive, `up_cascade` (slot 58) resolves ancestor components and detects cycles, and `par_chunks_readonly` / `TypedChunkIter::par_for_each_chunk_readonly` (slot 59) succeed on read-only queries while rejecting writable queries with `PERMISSION_DENIED`. |
| `decoupled_scheduling_and_schedule_sets` | Proves `SystemBuilder::register_handle` (slot 60), `ScheduleSetBuilder` + `configure_schedule_set` (slot 61), and `GuestApp::mount_system` (slot 62) preserve execution-mode flags and before/after set ordering. |
| `command_transactions_commit_rollback_drop_and_nested` | Proves `GuestCommands::transaction` / `begin_transaction` (`cmd_checkpoint` slot 63 / `cmd_rollback` slot 64) handle explicit commit, explicit rollback, RAII `Drop` auto-rollback on error, and nested transactions. |
| `component_column_migration_success_error_panic_and_zst` | Proves `migrate_component_column` (slot 67) migrates `OldStats -> NewStats`, aborts cleanly on callback `Err`, contains panics as `PANIC_CAUGHT`, and handles ZST `OldTag -> NewTag` migrations. |
| `sixteen_term_query_tuple_and_eight_branch_or` | Proves compile-time and descriptor-generation support for maximum-arity 16-term `QueryTuple`s and 2..=8-branch `Or<(T0..T7)>` (plus `Or2`/`Or3`) disjunctive filter groups. |

### [`tests/bevy_020_parity_tests.rs`](tests/bevy_020_parity_tests.rs)

Verifies full Bevy `0.20` SDK parity and fail-closed invariants under both `--no-default-features` and `--all-features`:

| Test | Why It Exists & What It Proves |
|---|---|
| `bevy_schedules_maps_all_16_schedules_without_violating_abi_030_reserved_range` | Proves `BevySchedules::DEFAULT` and `with_custom_base` map all 16 Bevy `0.20` schedules onto `0..=5` and `custom_base + 0..=9` without violating `ScheduleId::CUSTOM_BASE` or overflowing. |
| `isolated_asset_path_validation_rejects_traversal_and_malformed_inputs` | Proves `mod_asset_path` and `mod_overlay_path` reject `"overlay"` namespace collisions, `..` traversal, absolute paths, backslashes, NUL bytes, and scheme injection. |
| `bevy_020_wire_types_math_and_edge_cases_work_unconditionally` | Proves `FfiTransform` math (including collinear/zero `looking_to`/`looking_at` without NaN) and color/UI/audio/camera/picking/telemetry wire types work with `--no-default-features`. |
| `entity_commands_child_spawner_disable_enable_and_gizmos_end_to_end` | Proves `GuestEntityCommands`, `GuestChildSpawner`, `.disable()`/`.enable()` (routing through `cmd_insert_default` for bridged `Disabled`), and `GuestCommandsGizmosExt` end-to-end. |
| `run_conditions_local_state_removed_queries_3_single_chunk_and_descendants` | Proves `conditions::*`, `build_with_local`, `RemovedComponents` cursors, `queries_3_typed`, `TypedChunkIter::single_chunk`, `SystemContext::get_many`, and cycle-safe `iter_descendants`. |
| `typed_observer_builder_picking_events_and_abi_v03_component_options` | Proves `TypedObserverBuilder`, `observe_pointer_click`/`drag`, `conditions::run_once` gating + propagation stopping, and `GuestComponentOptions` (`schema_fingerprint`, `BAM_FIELDS`, `require_handle_with_value`). |
| `state_bridge_mod_state_slab_pod_messages_and_runtime_lifecycle_observers` | Proves `GuestStateBridge::current_optional` (active vs absent), `ModStateSlab` (`ModStateEntry`, `ModStateTransitionRequest`, `despawn_on_*_slab`), batch POD messages, and runtime `observe_add`/`insert`/`discard`/`remove`/`despawn`. |
| `derive_non_pod_dynamic_entity_access_and_non_pod_messages_end_to_end` *(derive)* | Proves `#[derive(ModComponent)]` on `String`-bearing non-POD types works end-to-end across `register_dynamic_component`, `get_dynamic`/`get_dynamic_mut`, and witness-encoded `read_non_pod_messages`/`send_non_pod_message` without leaks. |
| `typed_chunk_iter_single_chunk_v02_fallback_detects_multiple_1_entity_chunks` | Regression test proving `TypedChunkIter::single_chunk` on ABI `0.2.0` hosts (`iter_count = None`) scans all chunks so two 1-entity chunks return `MultipleEntities` instead of silently returning chunk 0. |
| `read_removed_preserves_type_mismatch_when_zero_user_queries` | Regression test proving `SystemContext::read_removed` preserves `TYPE_MISMATCH` when `user_query_count == 0` and term 0 of the resource bundle is not a `REMOVED` stream. |
| `ui_mapped_string_and_wire_bridges_and_disabled_bridge_roundtrip` | Proves `ModUiBridgePlugin` string/wire VTable bridges (`Text`, `TextSpan`, `BackgroundColor`, `BorderColor`, `RemSize`, `EmSize`, `ThemeContext`), `GuestDisabledBridge`, and `observe_value_change`/`observe_activate`. |
| `all_48_host_mod_components_match_derive_uuid_size_and_align` *(derive)* | Exhaustive ABI parity proof asserting bit-identical `(size_of, align_of, CANONICAL_NAME, UUID)` across all 48 `bamabi_bridges` `ModComponent` mirrors (including `TargetEntity` on all 7 `FfiPointer*` events). |
| `ffi_key_code_all_194_indices_and_ui_wire_constructors_match_bridges` | Proves complete 194-key `FfiKeyCode` (`0..=193`) coverage, 4-word `FfiKeyboardInput` bitset operations, and UI wire constructor/`Default` parity. |
| `spawn_audio_resolves_before_spawn_no_orphan_entity_and_v02_fallback` | Regression test proving `GuestCommandsAudioExt::spawn_audio` resolves `FfiAudioPlayerSpec` before spawning (zero orphan entities on error) and handles `0.2.0` fallback with despawn cleanup on insert failure. |
| `ui_handles_text_span_default_inserters_trigger_activate_and_gamepad_rumble_end_to_end` | Proves `GuestUiHandles`, `TextSpan` query terms, `insert_default_*`, `trigger_activate*`, `write_gamepad_rumble`, and `GuestGamepadRumbleExt` (including `ThemeContext` absent vs present and `TYPE_MISMATCH` / `FfiEntity::NULL` validation). |
| `wire_defaults_and_constructors_match_bamabi_bridges_and_bevy_020` | Proves exact `Default` and constructor parity with `bamabi_bridges` / Bevy `0.20` for `FfiRemSize`, `FfiEmSize`, `FfiUiTextStyle`, `FfiEditableTextConfig`, `FfiInlineBox`, `FfiSelectedTab`, `FfiValueChange`, Feathers controls, `FfiSpatialAudioSinkControl`, and `FfiGamepadButton::bit`. |
| `fail_closed_ui_theme_mismatch_unregistered_activate_hierarchy_and_removed_with_user_query` | Regression test proving fail-closed `TYPE_MISMATCH` on spoofed `ThemeContext`, `NOT_FOUND`/`NULL_POINTER` on `trigger_activate*`, `TYPE_MISMATCH` on canonical `Children`/`ChildOf` UUID drift even when aliases exist, and `TYPE_MISMATCH` preservation in `read_removed` alongside a user query. |
| `camera_light_wire_defaults_match_bevy_020_bridges` | Proves every camera, lighting, `PanOrbitCamera`, and `SolariLighting` `Default` matches Bevy `0.20.1` and `bamabi_bridges::camera_light` field-for-field. |

### [`tests/slice_and_pointer_edge_tests.rs`](tests/slice_and_pointer_edge_tests.rs)

| Test | Why It Exists & What It Proves |
|---|---|
| `slice_alignment_overflow_zst_and_stride_edge_cases` | Proves `ChunkSlice` / `ChunkSliceMut` accept aligned valid slices, reject mismatched strides and unaligned pointers, reject byte lengths `> isize::MAX` without integer wrap, and handle non-empty ZST (`stride = 0`) slices without pointer arithmetic UB. |
| `raw_dynamic_component_with_drop_contains_panics_and_runs_destructor` | Proves `register_dynamic_component_with_drop` invokes custom `Drop` glue on normal destruction and catches panicking destructors before unwinding across `extern "C"`. |

### [`tests/split_borrow_and_lifecycle_tests.rs`](tests/split_borrow_and_lifecycle_tests.rs)

| Test | Why It Exists & What It Proves |
|---|---|
| `export_mod_manifest_and_multi_world_instance_isolation` | Exercises a real `export_mod!(plugin: IsolatedPlugin)` loaded into two `MockHostWorld` instances simultaneously, proving manifest metadata, auto-prefixed component names, independent per-world ticks, and that shutting down World A leaves World B ticking cleanly. |
| `split_borrows_and_multi_query_nested_reset_enforce_disjointness` | Proves `queries_2_and_commands` and `Chunk::{component_write_2, entities_and_component_read_write}` reject duplicate column indices or duplicate component handles, while permitting disjoint simultaneous borrows and nested `reset()` N × M loops. |
| `unqualified_guest_component_sparse_resource_and_cross_mod_rejection` | Proves unqualified `GuestComponent` names auto-prefix with `<manifest_name>::` across sparse rows and resources while rejecting layout-identical columns owned by another mod (`other_mod::LocalCounter`). |

---

## 3. Inline Unit Tests (`crates/bamabi_guest/src/`)

| Module | Test | What It Proves |
|---|---|---|
| [`src/lib.rs`](src/lib.rs) | `const _:` compile-time check + `tests::derive_smoke::derived_guest_component_uuid_is_deterministic` *(derive)* | `guest_component_uuid` is `const`-evaluatable over `AbiPod` arrays; `#[derive(GuestComponent)]` (`no_schema_hash`) and `#[mod_component(..., guest)]` produce deterministic UUIDs and preserve flags. |
| [`src/lib.rs`](src/lib.rs) | `tests::invalid_slices_return_none_safely` | Invalid `ChunkSlice` / `ChunkSliceMut` report `len() == 0`, `is_empty()`, and safely return `None` from typed slice conversions. |
| [`src/app/hierarchy.rs`](src/app/hierarchy.rs) | `tests::hierarchy_guest_sdk_signatures_compile_with_forbid_unsafe` | All hierarchy traversal, child command, and `register_relationship_pair` signatures compile under `#![forbid(unsafe_code)]`. |
| [`src/app/observers.rs`](src/app/observers.rs) | `tests::guest_hooks_and_options_builders_preserve_all_flags_hooks_and_requirements` | `GuestComponentHooks` and `GuestComponentOptions` preserve all 5 lifecycle hooks, `clone`, `summary_tick`, and required handles when merged. |
| [`src/iter/filters.rs`](src/iter/filters.rs) | `tests::filter_query_terms_and_tuples_typecheck` & `derived_filter_query_tuples_typecheck` *(derive)* | `Allow<T>`, `Has<T>`, `Or2`, and `Or3` satisfy `QueryTerm` and compose cleanly inside `QueryTuple`. |
| [`src/bridges/asset.rs`](src/bridges/asset.rs) | `tests::typed_and_untyped_asset_handle_wrappers_preserve_raw_fields` | `TypedAssetHandle<A>` and `GuestAssetHandle` preserve raw `FfiAssetHandle` type, index, and generation across conversions. |
| [`src/bridges/bsn.rs`](src/bridges/bsn.rs) | `tests::guest_scene_handle_uuid_is_deterministic_and_validates` | `FfiAssetHandle::from_uuid` produces deterministic valid BSN scene handles wrapped by `GuestScene`. |
| [`src/bridges/reflect.rs`](src/bridges/reflect.rs) | `tests::brp_method_constants_use_bamabi_namespace` | Custom BRP JSON-RPC method constants use `"bamabi/"` and `"bam/"` prefixes without colliding with `"world.*"`. |
| [`src/bridges/state.rs`](src/bridges/state.rs) | `tests::deterministic_state_schedule_ids_match_host_layout` | `on_enter_schedule`, `on_exit_schedule`, and `on_transition_schedule` match the host's contiguous state-transition `ScheduleId` layout. |
| [`src/bridges/std_types.rs`](src/bridges/std_types.rs) | `tests::ffi_wire_helpers_and_bitsets_work_without_unsafe` | `FfiTransform`, `FfiGlobalTransform`, `FfiVisibility`, `FfiKeyboardInput`, `FfiMouseInput`, and `FfiColor` helpers work under `#![forbid(unsafe_code)]`. |
| [`src/bridges/std_types.rs`](src/bridges/std_types.rs) | `tests::guest_name_bridge_vtable_header_matches_host_uuid` | `MappedStringVTable::header.uuid` matches `ComponentUuid::from_canonical_name("bevy_ecs::name::Name")`. |
| [`src/bridges/std_types.rs`](src/bridges/std_types.rs) | `tests::guest_wire_bridge_validates_mapped_wire_vtable_unconditionally` | Compile-time `assert_abi_pod` proof for all 15 std wire structs under `--no-default-features` plus `MappedWireVTable` size parity. |

---

## 4. Doctest Soundness Proofs (`20` doctests)

`cargo test --doc -p bamabi_guest` compiles and verifies `2` onboarding `no_run` examples in `src/lib.rs`, `2` macro examples in `src/macros.rs`, and **`16` `compile_fail` soundness proofs**:
- **`src/app/builder.rs` (`GuestApp`):** Rejects raw pointer construction (`GuestApp::from_raw`) and non-`AbiPod` registration (`String`).
- **`src/commands.rs` (`GuestCommands`):** Rejects raw pointer construction (`GuestCommands::from_raw`) and non-`AbiPod` insertion (`String`).
- **`src/component.rs` (`GuestComponent`):** Rejects heap-owning types (`String`) and invalid-bit-pattern types (`bool`) via `AbiPod`.
- **`src/iter/chunk.rs` (`Chunk`):** Rejects simultaneous mutable column borrows (aliasing) and non-`AbiPod` (`bool`) borrows.
- **`src/iter/chunk_iter.rs` (`ChunkIter`):** Rejects raw pointer construction (`ChunkIter::from_raw`) and advancing the iterator while a chunk slice borrow is live.
- **`src/iter/slice.rs` (`ChunkSlice`, `ChunkSliceMut`):** Rejects raw pointer slice construction (`ChunkSlice::from_raw_parts`), non-`AbiPod` (`bool`) casts, and overlapping mutable slice borrows.
- **`src/iter/typed_query.rs` (`Or`, `Or2`):** Rejects non-filter data terms (`&FfiChildOf`) and 1-element `Or<(With<...>,)>` tuples at compile time.
