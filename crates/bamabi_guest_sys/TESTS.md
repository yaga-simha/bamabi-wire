# `bamabi_guest_sys` Test & Verification Suite

`bamabi_guest_sys` uses a four-layer verification pyramid to ensure zero undefined behavior, zero aliasing violations, zero unwinds across `extern "C"`, and fail-closed validation against adversarial or ABI-older hosts:

1. **Compile-Time Layout & Schema Proofs (`src/wire_types/`):** `const _: ()` assertions in `impl_wire_abi_pod!` and `FfiKeyCode::ALL` verify zero implicit padding, `(size, align)` parity, `!needs_drop`, and `0..194` key-index bijection at compile time.
2. **Integration & Soundness Regression Suites (`tests/`):**
   - `tests/host_and_instance_tests.rs` (**7 tests**) — host table handshake, VTable checks, `GuestInstance` multi-World isolation, LIFO teardown, rollback, runtime observer tracking, and dynamic component symbols.
   - `tests/iter_tests.rs` (**1 test**) — non-POD witness message serialization/reconstruction and marker enforcement.
   - `tests/soundness_regressions.rs` (**11 tests**, covering audit findings **S1–S13**) — adversarial host/pointer/re-entrancy/overlap/hot-reload/Tree-Borrows regressions.
3. **Property-Based Fuzzing (`tests/proptest_guest_sys.rs`, 3 `proptest!` suites):** Randomized borrow sequences, hostile message counts, and malformed sparse-row pointer arrays.
4. **Formal Verification (`src/kani_proofs.rs` & `src/kani_bounds_proofs.rs`, `35` `#[kani::proof]` harnesses across `PO-G1`, `PO-G3`..`PO-G10`):** Symbolic CBMC proofs over unbounded/adversarial pointers, sizes, counts, stack scratch bounds, and overlap offsets.

*(Integration, property, and soundness suites live in `tests/` alongside `tests/common/mod.rs` mock fixtures, while `src/commands.rs` also carries 1 focused unit test for `SpawnBatchColumn::typed` so Miri and `cargo test` verify its zero-FFI bounds directly.)*

---

## 1. Shared Test Harness (`tests/common/mod.rs`)

| Symbol | Purpose |
|---|---|
| `PaddedPod` | `#[repr(C)]` `(tag: u8, val: u32)` struct with 3 bytes of natural alignment padding; verifies under Miri that raw pointer command copies never materialize `&[u8]` over uninitialized padding bytes. |
| `DummyVTable` / `dummy_getter` | Minimal `#[repr(C)]` `VTableHeader`-prefixed VTable fixture for `Host::resolve_vtable` tests. |
| `VALID_VTABLE_UUID`, `VALID_VTABLE`, `TRUNCATED_HEADER`, `WRONG_VERSION_VTABLE` | Static valid, undersized (`MIN_V0_2_SIZE`), and version-mismatched (`0.1.0`) VTable fixtures. |
| `custom_math_fn` | `"bam.v0.math.add10"` mock custom symbol (`x + 10`). |
| `MockWorldCtx` | Mock context struct backing `BamHostCtx`, `BamLoadCtx`, and `BamCommands` pointers. |
| `stub_component_layout_info_padded` | Mock `component_layout_info` reporting `PaddedPod`'s `(size, align, uuid)` for handle `1`. |
| `make_mock_api` & `stub_*` | Zeroed `BamHostApi` populated with all 27 core ABI `0.2` stubs and all 46 extended/ABI-`0.3.0` slots set to `None` for opt-in customization per test. |

---

## 2. Host, Instance & Lifecycle Tests (`tests/host_and_instance_tests.rs`)

| Test Function | Why It Exists | Invariant Proved |
|---|---|---|
| `reentrant_trampoline_conflict` | Re-entrant system or `run_if` calls and panicking closures must not alias `&mut` closure state or leave `ReentrantCell` permanently locked. | `system_closure_trampoline` returns `FfiResult::ACCESS_CONFLICT` on re-entry, `FfiResult::PANIC_CAUGHT` on panic, and `ReleaseGuard` clears `in_use` (`Ordering::Release`) so the next tick returns `OK`; `run_if_closure_trampoline` returns `FfiBool::FALSE` on nested re-entry. |
| `vtable_header_and_custom_symbol_validation_under_miri` | Host-exported `"bam.v0.*"` symbols and VTables cross an untrusted C-ABI boundary. | `last_error()` implements the two-pass `snprintf` sizing protocol; `resolve_custom_symbol` rejects non-`"bam.v0."` names (`INVALID_ARGUMENT`); `resolve_custom_fn` resolves valid `"bam.v0.*"` functions; `resolve_vtable` rejects wrong UUID (`TYPE_MISMATCH`), truncated headers (`INVALID_VERSION`), and `0.1.0` headers (`INVALID_VERSION`) before forming `&V`. |
| `sparse_rows_and_padded_commands_sound_under_miri` | Sparse row iteration and command copies of padded structs are frequent sources of Miri Stacked/Tree Borrows and uninitialized-byte UB. | `SparseRows` / `SparseRowsMut` (`from_raw_parts`, `get`, `Index`, `iter`, `for_each_mut`) and `ValidatedCommands::spawn` / `insert` on `PaddedPod` (with uninitialized padding) run cleanly under Miri. |
| `multi_world_guest_instance_isolation_and_rollback` | Loading the same `.cdylib` into multiple Bevy `World`s or failing midway through `bamabi_init` must not leak or corrupt sibling worlds. | World A and World B hold independent `GuestInstance` allocations; `shutdown_guest_instance(inst_a)` executes World A's LIFO hooks and frees World A's allocations while World B stays intact; `shutdown_guest_instance(null)` is a no-op; failed `init_guest_instance` resets `*instance_out` to `null`, drops staged allocations, and discards unrun shutdown hooks. |
| `runtime_observer_alloc_tracking_and_unsupported_fallbacks` | Runtime observers spawned after init via `ValidatedCommands::observe` must outlive `bamabi_init` and drop on shutdown, while ABI `0.2` hosts lack `0.3` slots. | `asset_load` and `sys_entity_children` fail cleanly with `FfiResult::UNSUPPORTED` when slots are `None`; runtime observer closures tracked in `RuntimeAllocTracker` stay alive across `init_guest_instance` and drop exactly once during `shutdown_guest_instance`. |
| `system_closure_trampoline_refreshes_host_ctx_via_sys_host_ctx` | Multi-world or relocated invocations may pass a per-tick `BamHostCtx` distinct from the init-time pointer. | `system_closure_trampoline` lazily queries `BamHostApi::sys_host_ctx(sys_ctx)` and updates `Host::host_ctx` for both the invocation and `commands_mut().host()`. |
| `system_closure_trampoline_defers_sys_host_ctx_and_sys_commands_until_accessed` | Hot-path systems (`noop`, query-only) must avoid unnecessary per-tick FFI calls while preserving single-take `LazyCommands` exclusivity. | Noop invocations make 0 `sys_host_ctx` and 0 `sys_commands` calls; repeated `inv.host()` calls invoke `sys_host_ctx` at most once per tick (falling back to init-time `host_ctx` on null); `take_lazy_commands()` defers `sys_commands` until `.resolve()` and rejects duplicate extraction. |
| `register_dynamic_mod_component_symbols_and_moved_command_error_drops_cleanly` | Dynamic `ModComponent` registration must export both canonical and manifest-qualified symbols, and failed `insert_moved` / `spawn_moved` calls must not leak guest heap memory. | `register_dynamic_mod_component` registers `"bam.v0.<Name>.vtable"`, `"bam.v0.<mod>::<Name>.vtable"`, and `init_default`; calling `init_default` constructs `T::default()` in place; failed `insert_moved` / `spawn_moved` on an empty command buffer invoke `Drop` on the unmoved value. |

---

## 3. Non-POD Message Witness Tests (`tests/iter_tests.rs`)

| Test Function | Why It Exists | Invariant Proved |
|---|---|---|
| `validated_iter_non_pod_message_read_and_send_encode_witness_packets` | Non-POD messages (`String`, `Vec<u32>`) cannot be bitwise-copied across ABI boundaries and must not spoof pointer-sized POD messages. | `read_non_pod_messages_checked` reconstructs `MockNonPodMsg` via host `export_fields` + guest `import_field` (`forward_field`); `send_non_pod_message_checked` serializes a length-prefixed witness packet (`field_count == 2`, `append_field`); a pointer-sized type lacking `"bam.v0.<M>.non_pod_message"` (`MockPodWithoutMarker`) fails closed with `FfiResult::TYPE_MISMATCH`. |

---

## 4. Property-Based Fuzz Tests (`tests/proptest_guest_sys.rs`)

| Property Test | Fuzz Inputs | Why It Exists & Invariant Proved |
|---|---|---|
| `chunk_column_dispenser_random_sequences_never_alias_mutably` | `chunk_len in 0..=8`, 1..=40 random `BorrowOp`s (`Read`, `Write`, `OptionalRead`, `OptionalWrite`, `Entities`) across 12 component IDs, byte offsets `0..=128`, element kinds (`U8`, `U32`, `U64`, `Zst`), and null optional flags. | Proves `ChunkColumnDispenser` matches an independent interval-and-component oracle on every step, rejects overlapping mutable borrows and `> MAX_TERMS_PER_QUERY` capacity overflows (`ACCESS_CONFLICT` / `CAPACITY_EXCEEDED`), and guarantees all simultaneously held `&mut [T]` / `&[T]` / `&[FfiEntity]` slices remain strictly disjoint and writable under Miri. |
| `read_messages_batch_checked_clamps_hostile_read_count_and_guards_metadata` | `max_count in 0..=64`, hostile `read_count` (including `usize::MAX`), ZST & `MsgPod` layouts, and corrupted `BamColumnInfo` (`mode`, `storage_flags`, `size`, `align`, `uuid`, host error codes). | Proves `read_messages_batch_checked` rejects metadata mismatches with `TYPE_MISMATCH` before calling `msg_read_batch`, handles `max_count == 0`, propagates host errors, and clamps `read_count` to `max_count` so `scratch.set_len(actual)` never exposes uninitialized `MaybeUninit<M>` slots. |
| `sparse_rows_and_mut_hostile_pointer_array_fuzzing` | Outer pointer array states (`Valid`, `Null`, `Misaligned`, `OverflowLen`), `allow_null_rows`, 0..=24 `RowPtrPlan`s (`Null`, `Misaligned`, `Valid` with potential duplicates), and `ZstPod` vs `u64`. | Proves `SparseRows::from_raw_parts` and `SparseRowsMut::from_raw_parts` reject null/misaligned/`> isize::MAX` outer arrays, null rows when `!allow_null_rows`, and misaligned non-ZST rows before dereferencing, while valid duplicate row pointers remain sound because `SparseRowsMut` lends at most one `&mut T` at a time. |

---

## 5. Soundness Regression Tests (`tests/soundness_regressions.rs`, S1–S13)

| Test Function | Audit Finding(s) | Invariant Proved |
|---|---|---|
| `s1_and_s3_resource_handle_validation_fails_closed` | **S1, S3** | Handle-based resource lookups (`res_ptr_by_handle_checked`, `res_mut_ptr_by_handle_checked`, `read/write_mapped_wire_resource_by_handle`) fail closed (`None` / `UNSUPPORTED`) when `component_layout_info` is absent (ABI `0.2`), and reject oversized reads (`[u8; 4096]` vs `u32`), misaligned types (`[u8; 4]`), wrong UUIDs, immutable handles on `_mut`, and mismatched wire VTable handles (`TYPE_MISMATCH`). |
| `s2_validated_system_ref_and_slots_cannot_be_duplicated` | **S2** | `ValidatedSystemRef` is non-`Copy`/non-`Clone` and `take_system_ref()` succeeds at most once; `take_raw_query_slot(0)` and `take_commands()` each succeed at most once (`commands_mut()` returns an empty disabled buffer afterward), preventing duplicate `&mut` handles. |
| `s4_spawn_and_moved_commands_validate_component_layout_and_vtable` | **S4** | `SpawnEntry`, `SpawnBatchColumn`, `insert_moved`, and `spawn_moved` fail closed (`UNSUPPORTED` on ABI `0.2`, `TYPE_MISMATCH` on undersized entries, non-ZST markers, foreign UUIDs with matching `(size, align)`, or invalid `extra` entries) and cleanly `Drop` unmoved guest `OwnedDyn` values on every error path. |
| `s5_and_s6_non_pod_message_uuid_enforced_and_callbacks_panic_and_null_safe` | **S5, S6** | `read_non_pod_messages_checked` and `send_non_pod_message_checked` enforce `M::UUID` even when `expected_uuid == None` (`TYPE_MISMATCH` on `MsgA` vs `MsgB`); `forward_field` and `append_field` reject null and misaligned `ctx` pointers with `INVALID_ARGUMENT` without dereferencing. |
| `s7_owned_runtime_allocs_and_system_descriptor_pointers_sound_under_miri` | **S7** | `OwnedDescriptorStorage` (`OwnedRawCString`, `OwnedRawTermSlice`) and `OwnedRuntimeAllocs` (`Box::into_raw`) preserve pointer provenance after `GuestInstance` is boxed/moved in `init_guest_instance`, passing Miri Stacked & Tree Borrows when host `register_system` and post-init `cmds.observe` dereference those pointers. |
| `s8_stride_zero_isize_max_sparse_alignment_and_overlap_guards` | **S8** | (1) `stride == 0` in `ValidatedSlice`/`ValidatedSliceMut` rejects non-ZST casts (`u64`) while allowing ZSTs; (2) `SparseRows`/`SparseRowsMut` reject `len * ptr_size > isize::MAX`; (3) `ChunkColumnDispenser` rejects `isize::MAX` overflows on `entities`/`next_read`/`next_write`; (4) misaligned sparse row pointer arrays return `TYPE_MISMATCH` in `read_mapped_wire_row`; (5) `iter_entities_and_dynamic_slice_mut` rejects overlapping entity and component buffers. |
| `s9_dispenser_overlapping_write_then_read_and_repeat_entities_are_sound` | **S9** | `ChunkColumnDispenser` rejects an overlapping `next_read` after `next_write` with `ACCESS_CONFLICT` *before* forming `&[u32]`, preserving Tree Borrows soundness of the live `&mut [u32]`, and rejects a second `entities()` call whose host range shifts (`ACCESS_CONFLICT`). |
| `s10_restore_state_observer_mut_reentrancy_pod_clone_overlap_and_bridged_bounds` | **S10** | Under Miri Tree Borrows + Strict Provenance: (1) `guest_restore_state_trampoline` rejects `len > isize::MAX` (`OUT_OF_RANGE`) and misaligned `instance` (`INVALID_ARGUMENT`); (2) re-entrant `observer_mut_closure_trampoline` with identical live `event_data` and `propagate` pointers rejects with `ACCESS_CONFLICT` *before* forming `&mut E` or clobbering `*propagate`; (3) `pod_clone_trampoline` handles identical and overlapping `(src, dst)` via `ptr::copy`; (4) bridged dense rows reject `current_len * host_size > isize::MAX` (`OUT_OF_RANGE`) and non-power-of-two `host_align` (`TYPE_MISMATCH`). |
| `s11_batch_col_cache_and_dispenser_epoch_and_scratch_soundness` | **S11** | Verifies batched chunk column pointer caching, `ChunkColumnDispenser` epoch bump invalidation, and `QuerySlotScratch` aliasing/bounds soundness. |
| `s12_query_slot_scratch_reuse_resets_column_info_mask` | **S12** | Verifies caller-owned `QuerySlotScratch` reuse via `ValidatedIter::from_raw_with_scratch` calls `reset_all()` so stale `col_info_mask` entries from a previous iterator/component never skip revalidation. |
| `s13_hot_reload_column_migration_and_scratch_shrink_soundness` | **S13** | Verifies (a) simulated `16 B -> 8 B` hot-reload migration starts a fresh `GuestInstance` with clean `SlotScratchesCell` caches while an un-migrated v1 registration fails closed on Tick 1, and (b) reusing a caller-owned `QuerySlotScratch` across a `16 B -> 8 B` schema shrink rejects `next_read::<[f32; 4]>` before `slice::from_raw_parts` can over-read under Miri. |

---

## 6. Kani Formal Verification Harnesses (`src/kani_proofs.rs` & `src/kani_bounds_proofs.rs`)

### 6.1 `src/kani_proofs.rs` — 22 Harnesses (`PO-G1`, `PO-G3`..`PO-G8`)

| Proof Obligation | Harness Function | What It Symbolically Proves |
|---|---|---|
| **PO-G1** | `po_g1_host_slot_bounds_all_41_extended_slots` | Across all 46 extended/ABI-`0.3.0` slots, `host_slot!` returns `Some` iff `populated && struct_size >= offset_of!(BamHostApi, slot) + size_of::<usize>()` with zero overflow. |
| **PO-G1** | `po_g1_host_from_raw_and_load_ctx_guards` | `Host::from_raw` and `Host::from_load_ctx` reject null, misaligned `api_ptr`, undersized `struct_size`, incompatible `abi_version`, missing core slots, and null `load_host_ctx`; an exact `MIN_V0_2_SIZE` truncated buffer triggers zero OOB reads and leaves all 46 extended slots `None`. |
| **PO-G1** | `po_g1_host_resolve_vtable_guards` | `Host::resolve_vtable` rejects undersized target types, null/misaligned pointers, undersized `VTableHeader::struct_size`, incompatible `abi_version`, and mismatched UUIDs strictly before forming `&V`. |
| **PO-G1 / PO-G6** | `po_g1_mapped_vtables_and_bridged_row_bounds` | `resolve_mapped_wire_vtable` / `resolve_mapped_string_vtable` enforce `"bam.v0."` prefix and non-`None` getters; `ValidatedIter` bridged row accessors reject non-power-of-two `host_align` (`TYPE_MISMATCH`) and `current_len * host_size > isize::MAX` (`OUT_OF_RANGE`) without OOB pointer arithmetic. |
| **PO-G3** | `po_g3_checked_slice_elem_offset_exact_bounds` | `checked_slice_elem_offset` over 22 stride classes (plus symbolic 8-bit `(len, index, stride)`) returns `Some(index * stride)` iff `valid && index < len && expected_size == Some(stride) && len * stride <= isize::MAX`. |
| **PO-G3** | `po_g3_raw_dense_slice_and_mut_soundness` | `raw_dense_slice` / `raw_dense_slice_mut` reject null/misaligned pointers and `> isize::MAX` byte lengths for non-ZSTs, handle ZSTs safely, and preserve exact elements on valid buffers. |
| **PO-G3** | `po_g3_validated_slice_and_mut_info_guards` | `ValidatedSlice` / `ValidatedSliceMut` enforce `stride == 0` non-ZST rejection, `BamColumnInfo` access-mode/immutable/resource/sparse-set/UUID checks, and safe `get_ptr` / `get_mut_ptr`. |
| **PO-G4** | `po_g4_dispenser_borrow_disjointness_and_entities_guard` | `ChunkColumnDispenser::check_and_record_borrow` rejects same-component conflicts, mutable byte-range overlaps (including against `entities_range`), and `usize` overflow cleanly. |
| **PO-G4** | `po_g4_dispenser_capacity_limit` | Exceeding `MAX_TERMS_PER_QUERY` returns `FfiResult::CAPACITY_EXCEEDED` without out-of-bounds array indexing. |
| **PO-G4** | `po_g4_dispenser_overlapping_write_then_read_soundness` | End-to-end `next_write` followed by overlapping `next_read` at symbolic element offsets `0..4` rejects with `ACCESS_CONFLICT` before forming `&[u32]`. |
| **PO-G4** | `po_g4_dispenser_entities_and_iter_overlap_guards` | `ChunkColumnDispenser::entities()` rejects shifted repeat ranges and `> isize::MAX`; `ValidatedIter::read_write_checked` and `write_2_checked` reject byte-overlapping columns across symbolic offsets `0..=2`. |
| **PO-G5** | `po_g5_sparse_rows_and_mut_validation` | `SparseRows` / `SparseRowsMut` reject null/misaligned/`> isize::MAX` outer arrays, null rows when `!allow_null_rows`, and misaligned non-null rows for `u64`, with panic-free `get` / `get_mut` for arbitrary `row: usize`. |
| **PO-G6** | `po_g6_iter_mark_changed_range_overflow_and_bounds` | `ValidatedIter::mark_changed_range` never overflows `start_row + row_count` and returns `OUT_OF_RANGE` iff `start_row + row_count > current_len`. |
| **PO-G6** | `po_g6_system_invocation_clamping_and_slot_exclusivity` | `ValidatedSystemInvocation` clamps query counts to `MAX_QUERIES_PER_SYSTEM`, enforces single-take exclusivity on `take_system_ref` / `take_raw_query_slot`, and enforces `idx_a != idx_b` in `queries_2_mut`. |
| **PO-G7** | `po_g7_trampolines_null_alignment_and_zst_guards` | `pod_clone_trampoline`, `generic_drop_trampoline`, `init_default_trampoline`, and `row_migrate_trampoline` guard null/misaligned pointers, handle ZSTs safely, and preserve values on valid aligned buffers. |
| **PO-G7** | `po_g7_validated_commands_null_entity_component_and_slot_guards` | `ValidatedCommands` rejects null command buffers, null entities, invalid components, non-empty null payloads, mismatched slice lengths, and uncovered ABI `0.3` slots. |
| **PO-G7** | `po_g7_guest_state_and_observer_trampolines` | `guest_restore_state_trampoline` / `guest_save_state_trampoline` and `observer_closure_trampoline` / `observer_mut_closure_trampoline` reject null/misaligned/`> isize::MAX` inputs and re-entrant invocations before forming `&mut E` or touching `*propagate`. |
| **PO-G8** | `po_g8_system_and_run_if_trampolines` | `system_closure_trampoline` and `run_if_closure_trampoline` reject null/misaligned `sys_ctx` / `user_data`, refresh `host_ctx` via `sys_host_ctx`, reject re-entrancy (`ACCESS_CONFLICT` / `FALSE`), and are panic-free. |
| **PO-G8** | `po_g8_lifecycle_observer_trampoline` | `lifecycle_observer_closure_trampoline` rejects null/misaligned `user_data`, forwards `commands`, rejects re-entrancy (`ACCESS_CONFLICT`), and is panic-free. |
| **PO-G8** | `po_g8_par_chunk_task_trampoline` | `par_chunk_task_trampoline` rejects null `sub_iter`, null `user_data`, and misaligned `user_data`, and dispatches chunks panic-free for both `par_for_each_chunk` and `par_for_each_chunk_readonly`. |
| **PO-G8** | `po_g8_pod_component_hook_trampolines` | `pod_on_add_trampoline` and `pod_on_remove_trampoline` reject null/misaligned `user_data` and `component_ptr`, handle ZSTs, reject re-entrancy, and are panic-free. |
| **PO-G8** | `po_g8_non_pod_message_witness_callbacks` | `forward_field` and `append_field` reject null/misaligned `ctx`, reject null non-empty `StrView` / `FfiSlice`, guard `u32` packet-length/field-count overflow (`CAPACITY_EXCEEDED`), and are panic-free. |

### 6.2 `src/kani_bounds_proofs.rs` — Harnesses (`PO-G9` & `PO-G10`)

| Proof ID | Harness Function | What It Symbolically Proves |
|---|---|---|
| **PO-G10** | `kani_proof_raw_msg_stack_scratch_alignment_and_bounds` | `RawMsgStackScratch` fits within `512 B`, keeps `align(16)`, and rejects any host `read_count > cap` (`INTERNAL_ERROR`) without OOB writes. |
| **PO-G10** | `kani_proof_drain_messages_conservation_and_ramp` | `drain_messages_into_vec_checked` conserves all ZST messages (`0..=200`) across probe and exponential ramp chunks cleanly. |
| **PO-G10** | `kani_proof_drain_messages_small_batch_ramp` | `drain_messages_into_vec_checked` conserves `u32` messages across stack-probe to heap-ramp transitions with `DrainOptions`. |
| **PO-G10** | `kani_proof_drain_messages_pre_reserved_exact_fit` | Pre-reserved `Vec<u32>` exact-fit drain fills `spare_capacity_mut()` without spurious heap growth via stack EOF probing. |
| **PO-G9.1** | `po_g9_read_messages_batch_clamps_hostile_count` | Hostile `read_count > max_count` (including `usize::MAX`) is clamped to `max_count` before `scratch.set_len` / `assume_init`; null/`UNSUPPORTED`/`TYPE_MISMATCH`/host errors fail closed. |
| **PO-G9.2** | `po_g9_spawn_batch_column_validation_bounds` | `validate_spawn_batch_column` and `spawn_batch_columns` fail closed before `cmd_spawn_batch` on `entity_count * expected_size > isize::MAX`, `col.entity_count < entity_count`, layout mismatches, and null non-empty pointers, and prove `SpawnBatchColumn::typed` skips `component_layout_info` while still enforcing `entity_count`, null-pointer, and `isize::MAX` overflow bounds. |
| **PO-G9.3a** | `po_g9_init_guest_instance_early_error_paths` | Every pre-builder failure path in `init_guest_instance` leaves `*instance_out == null` cleanly. |
| **PO-G9.3b** | `po_g9_init_guest_instance_build_fn_rollback_and_ok` | Failing `build_fn` rolls back staged allocations and leaves `*instance_out == null`; succeeding `build_fn` returns a valid instance reclaimed cleanly by `shutdown_guest_instance`. |
| **PO-G9.4a** | `po_g9_validated_system_ref_entity_and_resource_guards` | `ValidatedSystemRef::entity_get_checked`, `entity_get_mut_checked`, and `res_ptr_by_handle_with_uuid_checked` fail closed on missing `component_layout_info`, mismatched layout/UUID/flags, and null/misaligned pointers. |
| **PO-G9.4b** | `po_g9_commands_insert_moved_guards` | `ValidatedCommands::insert_moved` fails closed on missing `component_layout_info`, layout/UUID/VTable mismatches, null entity, and null commands while dropping unmoved values on the guest heap. |
| **PO-G9.4c** | `po_g9_commands_spawn_moved_guards` | `ValidatedCommands::spawn_moved` fails closed on missing `component_layout_info`, layout/UUID/VTable mismatches, invalid `extra` entries, and uncovered `cmd_spawn_persistent` while dropping unmoved values. |
| **PO-G9.5** | `po_g9_iter_entities_and_column_overlap_guards` | `ValidatedIter::entities_and_column_mut_checked` and `entities_and_read_write_checked` reject hostile overlapping `(entities, read, write)` pointers and misaligned/null pointers with `None` before forming aliasing slices. |

---

## 7. Running the Suites

```bash
# Unit/integration, proptest, and doctests
cargo test -p bamabi_guest_sys --all-features
cargo test --doc -p bamabi_guest_sys --all-features

# Miri under Tree Borrows & Stacked Borrows (strict provenance)
MIRIFLAGS="-Zmiri-tree-borrows -Zmiri-strict-provenance" cargo +nightly miri test -p bamabi_guest_sys
MIRIFLAGS="-Zmiri-strict-provenance" cargo +nightly miri test -p bamabi_guest_sys --test soundness_regressions

# Kani formal verification
cargo kani -p bamabi_guest_sys

# CI two-step split (runs 58 lightweight harnesses in parallel, then the ~7.5 GB / ~9.4 GB
# RSS PO-G10 drain harnesses serially to fit within 16 GB runner memory)
cargo kani -p bamabi_sys -p bamabi_guest_sys -Z unstable-options --harness-timeout 15m --output-format terse -j --harness po_ --harness kani_proof_raw_msg_stack_scratch_alignment_and_bounds --harness kani_proof_drain_messages_conservation_and_ramp
cargo kani -p bamabi_guest_sys -Z unstable-options --harness-timeout 15m --output-format terse --harness kani_proof_drain_messages_small_batch_ramp --harness kani_proof_drain_messages_pre_reserved_exact_fit
```
