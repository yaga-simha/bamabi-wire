# `bamabi_guest_sys` Architecture & Implementation Guide

`bamabi_guest_sys` (`v0.3.0`, `ABI 0.3.0` / `0x0003_0000`, Rust Edition `2024`, MSRV `1.87`) is the low-level, **zero-Bevy-dependency** guest FFI runtime in Layer 0 (`bamabi-wire`). Its sole responsibility is to isolate **every** guest-side `unsafe` block, raw pointer check, `extern "C"` trampoline, and C-ABI layout projection behind audited `# Safety` contracts so that [`bamabi_guest`](../bamabi_guest/README.md) can enforce `#![forbid(unsafe_code)]`.

---

## 1. Crate Boundaries & Safety Policy

- **Dependency Policy:** Depends strictly on [`bamabi_sys`](../bamabi_sys/README.md) and `std` (`proptest` in `[dev-dependencies]` only). No `bevy_*` crate may ever appear in the dependency tree.
- **Compiler & Lint Gates (`src/lib.rs` & workspace lints):**
  - `#![warn(missing_docs)]`
  - `#![deny(unsafe_op_in_unsafe_fn, clippy::undocumented_unsafe_blocks, clippy::missing_safety_doc)]`
  - Workspace `improper_ctypes_definitions = "deny"`
  - Every `unsafe fn` documents its `# Safety` contract; every `unsafe` block carries an explicit `// SAFETY:` justification.
- **Allocator Isolation:** Guest closures, `CString` names, descriptor arrays, and caught Rust panic payloads (`Box<dyn Any + Send>`) are allocated and dropped **strictly on the guest heap**, never crossing the host/guest allocator boundary.
- **Zero Global Mutable State:** All runtime state lives inside per-World heap-allocated [`GuestInstance`](src/instance.rs) structs so one `.cdylib` can be loaded into multiple Bevy `World`s concurrently.

---

## 2. Module-by-Module Architecture

```text
  Host (.exe / World)
         │  BamHostApi (68 slots) + BamLoadCtx / BamSystemCtx
         ▼
  ┌──────────────────────────────────────────────────────────────────────┐
  │                         bamabi_guest_sys                             │
  │                                                                      │
  │  host.rs ──────► Host (bounded copy + host_slot! + VTableHeader)     │
  │                  ValidatedLoadCtx (init-time registration driver)    │
  │  instance.rs ──► GuestInstance (LIFO teardown, RuntimeAllocTracker,  │
  │                  state save/restore trampolines, init rollback)      │
  │  panic.rs ─────► catch_guest_result / catch_guest_bool / _unit       │
  │  trampoline.rs ► ReentrantCell, ValidatedSystemInvocation,           │
  │                  ValidatedSystemRef, SystemRegistrationConfig,       │
  │                  same-mod ModComponent verification & moved cmds     │
  │  component.rs ─► PodComponentHooks, drop/clone trampolines,          │
  │                  register_relation*, BamRowMigrateFn migrations      │
  │  iter.rs ──────► ValidatedIter, ValidatedSlice{,Mut}, SparseRows*,   │
  │                  ChunkColumnDispenser, witness non-POD messages      │
  │  commands.rs ──► ValidatedCommands, SpawnEntry, SpawnBatchColumn,    │
  │                  checkpoint/rollback, relations, trigger_ex, observe │
  │  wire_types/ ──► Zero-Bevy #[repr(C)] AbiPod Bevy 0.20 mirrors       │
  └──────────────────────────────────────────────────────────────────────┘
         │  Safe Rust APIs only (#![forbid(unsafe_code)])
         ▼
    bamabi_guest
```

### 2.1 `host.rs` & `host_slot!` (`src/host.rs`, `src/lib.rs`)

- **Bounded `BamHostApi` Copy & SemVer Check:**
  - `Host::from_raw(api_ptr, host_ctx)` and `Host::from_load_ctx(api_ptr, load_ctx)` check pointer nullness and alignment (`align_of::<BamHostApi>()`) before calling `bamabi_sys::read_descriptor::<BamHostApi>`.
  - `read_descriptor` reads `struct_size` at offset `0` first and copies `min(struct_size, size_of::<BamHostApi>())` bytes, zero-filling trailing bytes if the host is an older ABI `0.2` table (`BamHostApi::MIN_V0_2_SIZE` = `224` B on 64-bit / `116` B on 32-bit) vs full ABI `0.3.0` (`MIN_V0_3_SIZE` = `552` B on 64-bit / `280` B on 32-bit).
  - `api.validate_for_guest(ABI_VERSION)` verifies major/minor SemVer compatibility and confirms all **27 core ABI `0.2` slots** (ending at `cmd_remove`) are populated.
- **68-Slot `covers_slot` + `UNSUPPORTED` Fallback (`host_slot!`):**
  - All **41 extended/ABI-`0.3.0` slots** (23 extended `0.2` slots + 18 `0.3.0` slots) are accessed exclusively through the crate-private `host_slot!($api, $slot)` macro in `src/lib.rs`.
  - `host_slot!` verifies `api.covers_slot(offset_of!(BamHostApi, $slot), size_of_val(&api.$slot))` *and* returns `Option<fn_ptr>`. Wrappers return `FfiResult::UNSUPPORTED` when a slot is absent or uncovered.
- **Thread-Safe Pointer Newtypes:**
  - `HostCtxPtr(*const BamHostCtx)`, `ManifestNamePtr(*const u8)`, and `RuntimeAllocsPtr(*const RuntimeAllocTracker)` wrap raw pointers inside `Host` (`Clone + Copy`) with explicit `Send + Sync` safety proofs tied to the owning `GuestInstance` lifetime.
- **Custom Symbols & VTable Validation:**
  - `CUSTOM_SYMBOL_PREFIX = b"bam.v0."` is enforced by `Host::resolve_custom_symbol` and `Host::resolve_custom_fn` (checking pointer size/alignment before `ptr::read` into `F`).
  - `Host::resolve_vtable::<V>` checks `size_of::<V>() >= VTableHeader::MIN_V0_2_SIZE`, `align_of::<V>() >= align_of::<VTableHeader>()`, non-nullness/alignment of the host pointer, and `read_descriptor::<VTableHeader>(raw)?.validate(size_of::<V>(), expected_uuid)` *before* forming `&V`.
  - `Host::resolve_mapped_wire_vtable::<W>` and `Host::resolve_mapped_string_vtable` additionally require `"bam.v0."` prefix and non-`None` `get` slots, returning `Copy` handles [`ValidatedWireVTable<W>`](src/host.rs) and [`ValidatedStringVTable`](src/host.rs).
- **ABI `0.3.0` Host Methods & `ValidatedLoadCtx`:**
  - `Host` wraps `last_error` (two-pass `snprintf` sizing protocol: `(null, 0)` query then `byte_len + 1` guest buffer), `asset_load` / `asset_get_load_state` / `asset_release`, `log_diagnostic` (slot 50), `report_guest_fault` (slot 51), `resolve_component_by_uuid` (slot 52), `component_layout_info` (slot 53), `intern_symbol` (slot 55), and `bind_named_object` (slot 56).
  - `ValidatedLoadCtx<'a>` wraps `*mut BamLoadCtx` for the duration of `bamabi_init`, exposing checked wrappers for component/relation registration, observers, systems/handles, schedule sets, system mounting, and column migrations.

### 2.2 `instance.rs` — Per-World Isolation, LIFO Teardown & Hot-Reload (`src/instance.rs`)

- **Move-Safe Raw Provenance (`OwnedPinnedHost`, `OwnedRuntimeAllocs`, & `OwnedManifestBytes`):**
  - `GuestInstance` is constructed on the stack in `init_guest_instance` and then moved into `Box::into_raw(Box::new(instance))` at return.
  - Because every system, `run_if`, and observer trampoline stores an 8-byte `PinnedHostPtr` (`*const Host`) pointing into `GuestInstance`'s heap-pinned `Host` (avoiding per-closure 584-byte `Host` copies), and `Host` itself holds `*const u8` (manifest name) and `*const RuntimeAllocTracker`, storing those allocations as `Box<T>` inside `GuestInstance` would retag `Unique` provenance on move and trigger UB under Miri Stacked/Tree Borrows.
  - `OwnedPinnedHost`, `OwnedRuntimeAllocs`, and `OwnedManifestBytes` allocate via `Box::into_raw` and free via `Box::from_raw` only in their `Drop` impls, declared **last** in `GuestInstance` (with `pinned_host` before `_runtime_allocs` and `_manifest_bytes`) so Rust drops them strictly *after* `GuestInstance::drop` tears down every closure, observer, and hook referencing them.
- **Deterministic LIFO Teardown Order:**
  - `GuestInstance::shutdown_guest_instance` invokes `run_shutdown_hooks()` (popping and running each `FnOnce()` inside `catch_guest_unit`), then drops `GuestInstance`:
    1. `self.shutdown_hooks.clear()` (discards unrun hooks on failed-init rollback without running them).
    2. `self.save_state_hook = None` and `self.restore_state_hook = None`.
    3. `self.symbols.clear()`.
    4. `self._runtime_allocs.tracker().drain_lifo()` — swaps out the `Mutex<Vec<ErasedAllocation>>` via `core::mem::take` before popping, preventing deadlocks if a destructor touches the tracker; each `ErasedAllocation::drop` executes `drop_boxed::<T>` inside `catch_guest_unit`.
    5. `while let Some(alloc) = self.allocations.pop() { drop(alloc); }` — frees init-time allocations in reverse registration order.
    6. `_runtime_allocs` and `_manifest_bytes` field destructors run last.
- **Transactional Init Rollback (`init_guest_instance`):**
  - Initializes `*instance_out = null` immediately after non-null/alignment checks so any early validation error (`BamManifest::validate`, UTF-8/empty name, `Host::from_load_ctx`) or `build_fn` error/panic drops the staged `GuestInstance` on the guest heap and leaves `*instance_out == null`.
- **Hot-Reload State Transfer:**
  - `guest_save_state_trampoline` acquires `inst.save_state_hook` via `ReentrantCell::try_with_mut` (returning `ACCESS_CONFLICT` on re-entry), generates a guest `Vec<u8>`, passes `(ptr, len)` to the host `BamStateWriterFn`, and drops the `Vec<u8>` on the guest heap.
  - `guest_restore_state_trampoline` validates `bytes`/`len`, rejects `len > isize::MAX as usize` with `FfiResult::OUT_OF_RANGE` *before* calling `slice::from_raw_parts`, and invokes `inst.restore_state_hook` inside `ReentrantCell`.
- **Identity Qualification & Default Construction:**
  - `qualify_mod_component_identity::<T>` prefixes unqualified `T::CANONICAL_NAME` with `"{manifest_name}::"` and recomputes `ComponentUuid` if `T::UUID` used default `from_canonical_name` or `from_parts`.
  - `init_default_trampoline::<T>` constructs `T::default()` in place (`core::ptr::write`) inside `catch_guest_result` for `ValidatedLoadCtx::register_dynamic_mod_component` (`"bam.v0.<name>.init_default"`).

### 2.3 `panic.rs` — Unwind Containment (`src/panic.rs`)

- Every `extern "C"` guest entry point and callback trampoline wraps execution in:
  - `catch_guest_result` -> returns `FfiResult::PANIC_CAUGHT` on panic.
  - `catch_guest_bool` -> returns `FfiBool::FALSE` on panic.
  - `catch_guest_unit` -> swallows panic (for void destructors and `bamabi_shutdown`).
- Caught `Box<dyn Any + Send>` payloads are passed to `drop_panic_payload_safely`, which drops them inside a nested `catch_unwind` and calls `core::mem::forget(double_panic)` if the payload's own `Drop` panics—guaranteeing a Rust unwind can **never** cross `extern "C"`.
- Under `#[cfg(kani)]` (where Kani compiles with `-C panic=abort` and `catch_unwind` intrinsics are unsupported), the helpers call `f()` directly, proving strict panic-freedom of the guarded bodies.

### 2.4 `trampoline.rs` — `ReentrantCell`, System Frame & Dynamic Moves (`src/trampoline.rs`)

- **`ReentrantCell<T>` (`UnsafeCell<T>` + `AtomicBool`):**
  - `try_with_mut` executes `in_use.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)`.
  - An RAII `ReleaseGuard(&self.in_use)` stores `false` with `Ordering::Release` on drop—even if the closure panics and unwinds into `catch_guest_result` / `catch_guest_bool`.
  - Re-entrant or concurrent invocation observes `Err` and returns `None` *before* forming `&mut *self.value.get()`, mapping to `FfiResult::ACCESS_CONFLICT` (or `FfiBool::FALSE`).
- **System Execution Frame (`ValidatedSystemInvocation`, `ValidatedSystemRef`, & `LazyCommands`):**
  - `system_closure_trampoline` borrows `GuestInstance`'s pinned `Host` directly via `PinnedHostPtr` without copying 584 bytes or calling `BamHostApi::sys_host_ctx(sys_ctx)` eagerly; a stack-pinned `LazyInvocationHost` resolves and caches `sys_host_ctx(sys_ctx)` at most once per tick on first `host()` / `commands()` demand (preserving `raw_ctx` pointer provenance under Miri Stacked/Tree Borrows) before building `ValidatedSystemInvocation<'a>` (clamping `total_query_count` to `MAX_QUERIES_PER_SYSTEM`).
  - Query slots (`queries: [MaybeUninit<ValidatedIter<'a>>; MAX_QUERIES_PER_SYSTEM]` tracked by `u8` init/taken bitmasks) and `commands` (or single-use `LazyCommands<'a>` via `take_lazy_commands()`) are fetched lazily on first access; `queries_2_mut` / `queries_2_and_commands_mut` reject `idx_a == idx_b` and use `split_at_mut` for disjoint `&mut ValidatedIter` borrows.
  - `ValidatedSystemRef<'a>` is intentionally **non-`Copy` and non-`Clone`** (`take_system_ref` succeeds at most once). Its `&mut self` methods (`entity_get_mut_checked`, `res_mut_ptr_by_handle_with_uuid_checked`, `write_mapped_wire_resource_by_handle`) prevent duplicate `&mut` aliasing to the same resource or entity component.
  - All handle-based resource lookups fail closed via `Host::component_layout_info`: if `component_layout_info` is `UNSUPPORTED` (ABI `0.2`), or if `(size, align, uuid, !immutable)` disagree, they return `None` / `Err(...)`.
- **Descriptor Ownership (`OwnedDescriptorStorage`):**
  - `prepare_system_desc` validates system name, `ScheduleId`, `BAM_SYSTEM_KNOWN_FLAGS`, query/term capacities, and set names, then packs all strings (`OwnedRawCString` via `CString::into_raw`) and term slices (`OwnedRawTermSlice` via `Box::into_raw`) into a single `GuestInstance`-tracked `OwnedDescriptorStorage` allocation with move-stable provenance.
- **Callbacks & Observers:**
  - `PreparedRunIf`, `wrap_system_callback!`, and `wrap_run_condition!` provide panic-guarded `BamSystemFn` and `BamRunIfFn` adapters.
  - `observer_closure_trampoline`, `observer_mut_closure_trampoline`, and `lifecycle_observer_closure_trampoline` acquire `ReentrantCell::try_with_mut` **before** creating `&E` / `&mut E` or touching `*propagate`, ensuring Tree Borrows soundness even on hostile re-entry with identical pointers.
- **Same-Mod Dynamic `ModComponent` Verification & Move Commands:**
  - `verify_same_mod_dynamic_component::<T>` checks `(size, align)`, qualified/canonical UUID, and **exact pointer equality** (`core::ptr::eq`) between the host's `"bam.v0.<name>.vtable"` symbol (using a 256-byte stack scratch buffer to avoid heap allocation) and `T::erased_vtable_ptr()`.
  - `ValidatedCommands::insert_moved` and `spawn_moved` wrap `value: T` in `ManuallyDrop<T>`, validate `verify_moved_component_handle::<T>` (and any `extra: &[SpawnEntry]`), pass raw bytes on `OK` so host `CommandArena` takes ownership, or run `ManuallyDrop::drop` on the guest heap on any failure.

### 2.5 `iter.rs` — Validated Columns, Sparse Rows & Anti-Aliasing (`src/iter.rs`)

- **Dense Column Validation:**
  - `raw_dense_slice` / `raw_dense_slice_mut` handle ZSTs via `NonNull::dangling()` (rejecting null non-empty ZSTs where appropriate), and for non-ZSTs verify non-nullness, `align_of::<T>()`, `checked_mul(size_of::<T>())`, and `byte_len <= isize::MAX as usize`.
  - `ValidatedSlice` / `ValidatedSliceMut` enforce `stride != 0` for non-ZST casts (`validate_slice_meta` + `checked_slice_elem_offset`).
- **Indirect Sparse-Set Rows (`SparseRows` & `SparseRowsMut`):**
  - `from_raw_parts(ptrs, len, allow_null_rows)` verifies outer `ptrs` alignment (`align_of::<*const u8>()`), `len * size_of::<*const u8>() <= isize::MAX`, and scans all `len` row pointers upfront: rejecting null rows when `!allow_null_rows` (`OPTIONAL_READ` / `OPTIONAL_WRITE`) and rejecting any non-null misaligned row pointer for non-ZST `T`.
  - `SparseRowsMut::get_mut` and `for_each_mut` tie `&mut T` strictly to `&mut self`, ensuring at most one `&mut T` is ever live even if a faulty host supplies duplicate row pointers.
- **Anti-Aliasing Dispenser (`ChunkColumnDispenser`):**
  - Tracks up to `MAX_TERMS_PER_QUERY` `DispensedBorrow { component, start, end, is_mut }` entries plus `entities_range: Option<(usize, usize)>`.
  - `check_and_record_borrow` checks both `prev.component == component` and byte-interval overlap `(start < prev.end && prev.start < end)` **before** forming `&'chunk [T]` or `&'chunk mut [T]`, rejecting hostile overlapping host pointers with `FfiResult::ACCESS_CONFLICT` before Tree Borrows UB can occur.
  - `entities()` verifies repeated calls return the exact same `(start, end)` range and do not overlap any active mutable column borrow.
- **Change Ticks, Parallelism, Cascades, Messages & Bridged Rows:**
  - `mark_changed_range` uses `start_row.checked_add(row_count)` and checks `end_row <= current_len` (`OUT_OF_RANGE`).
  - `par_for_each_chunk` and `par_for_each_chunk_readonly` wrap each sub-iterator in `par_chunk_task_trampoline` inside `catch_guest_result`.
  - `up_cascade_checked` rejects writable terms (`PERMISSION_DENIED`), validates layout/UUID, and returns `Ok(None)` on `NOT_FOUND` or null entity/ptr.
  - POD messages (`read_messages_batch_checked`, `send_message_checked`, `send_messages_batch_checked`) clamp hostile `read_count.min(max_count)` *before* `scratch.set_len(actual)`.
  - Non-POD messages (`read_non_pod_messages_checked`, `send_non_pod_message_checked`) enforce `M::UUID` (or manifest-qualified UUID) + `"bam.v0.<M>.non_pod_message"` marker symbol, and marshal fields via `forward_field` / `append_field` (`PacketBuilder` wire format: `[field_count: u32; (name_len: u32, name, tag_len: u32, tag, val_len: u32, val)*]`, both callbacks guarded by `catch_guest_result` and `ctx` null/alignment checks).
  - Bridged row/resource access (`read_mapped_wire_row`, `write_mapped_wire_row`, `read_mapped_wire_resource`, `write_mapped_wire_resource`, `read_mapped_string_row`, `write_mapped_string_row`) supports both dense and `BAM_STORAGE_FLAG_SPARSE_SET` columns via `raw_bridged_row_ptr_ro` / `raw_bridged_row_ptr_mut`, checking `host_align.is_power_of_two()`, `host_size % host_align == 0`, and `current_len * host_size <= isize::MAX`.

### 2.6 `commands.rs` — Validated Deferred Commands (`src/commands.rs`)

- **`ValidatedCommands<'a>`** wraps `*mut BamCommands` with null/entity/component/payload checks across all command slots:
  - `spawn_empty`, `spawn` / `spawn_persistent` (stack-allocated `INLINE_SPAWN_CAP = 16` scratch arrays before falling back to `Vec`), `spawn_batch_columns`, `despawn`, `despawn_all`, `insert` / `insert_if_new` / `insert_default`, `remove`, `trigger` / `trigger_ex` / `trigger_lifecycle`, runtime `observe` / `observe_mut` / `observe_lifecycle` (tracked in `RuntimeAllocTracker`), `spawn_scene`, `reborrow`, ABI `0.3.0` `checkpoint` / `rollback`, and `insert_relation` / `insert_relation_with_payload` / `remove_relation`.
- **Fail-Closed Safe Entry Validation:**
  - `SpawnEntry::new` / `marker` and `SpawnBatchColumn::new` / `marker` record static `(size, align)` metadata verified against `Host::component_layout_info` in `validate_spawn_entry` / `validate_spawn_batch_column` (plus `entity_count * expected_size <= isize::MAX` and `col.entity_count >= entity_count` for batch columns), failing closed with `UNSUPPORTED` on ABI `0.2` hosts or `TYPE_MISMATCH` on layout disagreement.

### 2.7 `component.rs` — Component Registration, Hooks & Row Migrations (`src/component.rs`)

- **`PodComponentHooks<T>` & `HookTrampolineState<T>`:**
  - Supports `on_add`, `on_insert`, `on_discard`, `on_remove`, `on_despawn` (each wrapped in `ReentrantCell` and dispatched through `define_pod_hook_trampoline!` + `catch_guest_unit`), bitwise `enable_clone` (`pod_clone_trampoline`), `required_components`, and ABI `0.3` metadata (`schema_fingerprint`, `fields`, `required_init_descs`, `required_custom_bytes`).
  - `pod_clone_trampoline` uses `core::ptr::copy` (`memmove` semantics) so identical or overlapping `(src, dst)` pointers remain sound.
- **Non-POD Escape Hatch & Relations:**
  - `register_dynamic_component_with_drop` wires `generic_drop_trampoline::<T>` (checking nullness and alignment inside `catch_guest_unit`).
  - `register_component_with_layout` registers runtime `(size, align, drop_fn)` descriptors.
  - `register_relation` / `register_relation_raw` wrap slot 54 (`register_relation`).
- **Schema Migration Trampolines (`BamRowMigrateFn`):**
  - `migrate_component_column_raw` (slot 67), `migrate_component_column_pod::<Old, New, F>`, `migrate_mod_component_column::<Old, New, F>`, and `migrate_pod_component_column::<Old, New, F>` validate `new_desc` against `New` and the currently registered layout against `Old` (via `component_layout_info` when available), then invoke `row_migrate_trampoline::<Old, New, F>` inside `catch_guest_result` for each live row.

### 2.8 `wire_types/` — Unconditional Bevy `0.20` POD Projections

`src/wire_types/mod.rs` defines `impl_wire_abi_pod!`, which proves at compile time for every wire projection struct:
1. `!core::mem::needs_drop::<Self>()`,
2. every field implements `bamabi_sys::AbiPod` (`assert_abi_pod!`),
3. `size_of::<Self>() == sum(field sizes)` (zero implicit padding),
4. exact `(size, align)` match the declared ABI layout, and
5. when `schema = [...]` is supplied, synthesizes `pub const FIELD_SCHEMA_HASH: u64` via `ComponentUuid::hash_field_schema` + `core::mem::offset_of!`, matching `#[derive(ModComponent)]` and `#[derive(GuestComponent)]` in `bamabi_bridges` bit-for-bit.

| Submodule | Types & Constants |
|---|---|
| `std_types.rs` | `FfiTransform` (40B), `FfiGlobalTransform` (48B), `FfiVisibility` (1B), `FfiInheritedVisibility` (1B), `FfiViewVisibility` (1B), `FfiColor` (16B), `FfiTimeVirtual` (32B), `FfiTimeFixed` (40B), `FfiTimeReal` (24B), `FfiKeyCode` (2B, all 194 named Bevy `0.20` keys `0..=193` verified by compile-time `ALL` assertion), `FfiKeyboardInput` (96B), `FfiMouseButton` (1B), `FfiMouseInput` (24B), `FfiMouseMotion` (8B), `FfiMouseScroll` (12B), `FfiGamepadButton` (1B), `FfiGamepadState` (72B), `FfiGamepadRumble` (24B) |
| `lifecycle.rs` | `ModStateSlab` (`ModStateHandle`, `ModStateEntry`, `ModStateTransitionRequest`, `ModDespawnOnExit`, `ModDespawnOnEnter`), 64-byte `"BAMT"` `SystemTelemetryDigest64` + `fnv1a_64_hash_system` + `TelemetryFrameError`, `FfiWindowMode`, `FfiWindow`, `FfiWindowResized`, `FfiWindowCloseRequested`, `FfiAppExit`, `FfiAssetEventKind`, `FfiAssetEvent`, `FfiStateTransitionEvent`, and gizmos (`FfiGizmoLine`, `FfiGizmoRay`, `FfiGizmoCircle`, `FfiGizmoSphere`, `FfiGizmoRect`, `FfiGizmoCuboid`, `FfiGizmoArrow`) |
| `picking.rs` | `FfiPointerKind`, `FfiPointerButton`, `FfiPointerPress`, `FfiPointerRelease`, `FfiPointerMove`, `FfiPointerClick`, `FfiPointerOver`, `FfiPointerOut`, `FfiPointerDrag`, `FfiEntityCursor` (`0..=33` `SystemCursorIcon` constants) |
| `camera_light.rs` | `FfiCamera2d`, `FfiCamera3d`, `FfiPointLight`, `FfiSpotLight`, `FfiDirectionalLight`, `FfiPanOrbitCamera`, `FfiSolariLighting` + `CLEAR_COLOR_*`, `DEPTH_LOAD_OP_*`, `TONEMAPPING_*`, `TRANSMISSION_QUALITY_*`, `ORBIT_CONSTRAINT_*` |
| `audio.rs` | `FfiAudioPlaybackSettings`, `FfiAudioPlayerSpec`, `FfiSpatialAudioSinkControl` + `PLAYBACK_MODE_*`, `VOLUME_*`, `SINK_CMD_*`, `MUTE_CMD_*`, `APPLY_*` |
| `ui.rs` | `FfiVal`, `FfiUiRect`, `FfiCornerRadius`, `FfiBorderRadius`, `FfiRgba`, `FfiBorderColor`, `FfiRemSize`, `FfiEmSize`, `FfiUiNode`, `FfiFontSize`, `FfiUiTextStyle`, `FfiEditableTextConfig`, `FfiInlineBox`, `FfiInlineImage`, `FfiTabList`, `FfiTab`, `FfiSelectedTab`, `FfiValueChange`, `FfiThemeContext`, `FfiFeathersColorInput`, `FfiFeathersDropdown` (`FfiFeathersSelect`), `FfiFeathersScrollableList` (`FfiFeathersListView`), `FfiFeathersListRow`, `FfiFeathersNumberInput` |
| `render.rs` | `FfiSprite`, `FfiStandardMaterial`, `FfiWeslMaterialSpec`, `FfiSpriteMaterialSpec`, `FfiMesh2d`, `FfiMesh3d` |

---

## 3. Setup, Building, Integration & Verification

### 3.1 Setup & Building

```bash
# Type-check & build standalone (inside bamabi-wire)
cargo check -p bamabi_guest_sys --all-targets --all-features
cargo build -p bamabi_guest_sys --all-features

# Verify zero Bevy dependencies in the resolved graph
cargo tree -p bamabi_guest_sys | sed 's/ (.*//' | grep -v bamabi | grep -i bevy   # prints nothing
```

### 3.2 Integration Pattern (`.cdylib` Guest Mods)

Guest mods normally depend on `bamabi_guest` (which re-exports safe wrappers and `export_mod!`). Lower-level runtime integrators calling `bamabi_guest_sys` directly follow this lifecycle:

1. In `bamabi_init(api, load_ctx, instance_out)`: call `init_guest_instance(api, load_ctx, instance_out, &MANIFEST, |instance, load| { ... })`.
2. Inside the closure: register components (`load.register_pod_component*` / `load.register_dynamic_mod_component`), relations (`load.register_relation`), schedule sets (`load.configure_schedule_set`), systems (`load.register_system` / `load.register_system_handle` + `load.mount_system_in_schedule`), and observers (`load.register_observer*`), plus optional hot-reload hooks (`instance.set_save_state_hook` / `set_restore_state_hook`).
3. During system execution: interact exclusively through `ValidatedSystemInvocation`, `ValidatedSystemRef`, `ValidatedIter` / `ChunkColumnDispenser`, and `ValidatedCommands`.
4. In `bamabi_shutdown(instance)`: call `shutdown_guest_instance(instance)` to run LIFO shutdown hooks and drop all guest-heap allocations cleanly.

### 3.3 Testing, Miri & Kani

```bash
cargo fmt -p bamabi_guest_sys -- --check
cargo clippy -p bamabi_guest_sys --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc -p bamabi_guest_sys --no-deps --all-features
cargo test -p bamabi_guest_sys --all-features
cargo test --doc -p bamabi_guest_sys --all-features

# Miri dual-model gate (Tree Borrows + Stacked Borrows, strict provenance)
MIRIFLAGS="-Zmiri-tree-borrows -Zmiri-strict-provenance" cargo +nightly miri test -p bamabi_guest_sys
MIRIFLAGS="-Zmiri-strict-provenance" cargo +nightly miri test -p bamabi_guest_sys --test soundness_regressions

# Kani formal verification (PO-G1, PO-G3..PO-G9)
cargo kani -p bamabi_guest_sys
```

See [`TESTS.md`](TESTS.md) for the complete inventory of integration tests, property tests, soundness regressions, and Kani proof obligations.

### 3.4 Benchmarking Notes

- `bamabi_guest_sys` is a **Layer 0** wire/FFI crate and intentionally contains **no Criterion benchmarks** so its dependency graph stays minimal (`bamabi_sys` + `std` only).
- End-to-end runtime ECS throughput and FFI dispatch overhead benchmarks live in the downstream `bamabi-showcase` repository.
- For zero-allocation guest-to-host telemetry reporting, `wire_types::lifecycle` defines [`SystemTelemetryDigest64`](src/wire_types/lifecycle.rs), a fixed 64-byte little-endian `"BAMT"` frame (`p50_us`, `p99_us`, `matched_entities`, `matched_tables`, `skipped_summary_tick_tables`, `chunks`, `commands_queued`).
