//! Kani formal verification proof harnesses for `bamabi_guest_sys` (`PO-G1`, `PO-G3`..`PO-G8`).
//!
//! Symbolically verifies the guest-side `unsafe` FFI boundary:
//! - `PO-G1`: `host_slot!` bounds across all 41 extended/ABI-0.3 `BamHostApi` slots, plus
//!   `Host::from_raw`, `Host::from_load_ctx`, and `Host::resolve_vtable` pointer/alignment/header guards.
//! - `PO-G3`: `checked_slice_elem_offset`, `raw_dense_slice`, `raw_dense_slice_mut`, `ValidatedSlice`,
//!   and `ValidatedSliceMut` layout, alignment, ZST, and overflow bounds.
//! - `PO-G4`: `ChunkColumnDispenser` disjoint borrow tracking and anti-aliasing guarantees (including
//!   hostile overlapping write-then-read / read-then-write / entities-then-write host pointers).
//! - `PO-G5`: `SparseRows` and `SparseRowsMut` outer/inner pointer, alignment, optional-null, and ZST
//!   validation plus panic-free `get`/`get_mut` indexing.
//! - `PO-G6`: `ValidatedIter::mark_changed_range` overflow/bounds safety and `ValidatedSystemInvocation`
//!   query clamping, single-take exclusivity, and `queries_2_mut` disjointness.
//! - `PO-G7`: `pod_clone_trampoline`, `generic_drop_trampoline`, `init_default_trampoline`, and
//!   `ValidatedCommands` null/entity/component/payload/uncovered-slot guards.
//! - `PO-G8`: `extern "C"` trampoline ABI guards (`system_closure_trampoline`,
//!   `run_if_closure_trampoline`, `lifecycle_observer_closure_trampoline`,
//!   `par_chunk_task_trampoline`, `pod_on_add_trampoline`/`pod_on_remove_trampoline`, and
//!   `forward_field`/`append_field` non-POD message witness callbacks).

use core::{
    ffi::{CStr, c_char, c_void},
    mem::{align_of, offset_of, size_of},
    sync::atomic::{AtomicUsize, Ordering},
};

use bamabi_sys::{
    ABI_VERSION, AccessMode, BamChangeTicks, BamColumnInfo, BamCommands, BamComponent, BamHostApi,
    BamHostCtx, BamInstance, BamIter, BamLayoutInfo, BamLoadCtx, BamObserverDesc, BamRelationDesc,
    BamScheduleSetDesc, BamSymbol, BamSystem, BamSystemCtx, BamSystemDesc, BamSystemSet,
    BamTermDesc, BamTriggerDesc, ComponentDesc, ComponentUuid, FfiAssetHandle, FfiAssetLoadState,
    FfiBool, FfiEntity, FfiResult, FfiSlice, MAX_QUERIES_PER_SYSTEM, MAX_TERMS_PER_QUERY,
    MappedStringVTable, MappedWireVTable, ScheduleId, StrView, VTableHeader,
    abi_version_is_compatible,
};

/// Zero-sized `AbiPod` marker alias for ZST proof paths.
type ZstPod = [u8; 0];

use crate::{
    ChunkColumnDispenser, GuestInstance, Host, ReentrantCell, SparseRows, SparseRowsMut,
    UuidFilter, ValidatedCommands, ValidatedIter, ValidatedSlice, ValidatedSliceMut,
    ValidatedSystemInvocation, ValidatedSystemSlice,
    component::{
        HookTrampolineState, RowMigrateCtx, generic_drop_trampoline, pod_clone_trampoline,
        pod_on_add_trampoline, pod_on_remove_trampoline, row_migrate_trampoline,
    },
    guest_restore_state_trampoline, guest_save_state_trampoline,
    instance::init_default_trampoline,
    iter::{
        ImportSinkCtx, PacketBuilder, append_field, checked_slice_elem_offset, forward_field,
        raw_dense_slice, raw_dense_slice_mut,
    },
    trampoline::{
        InvocationHostScratch, ObserverTrampolineState, PinnedHostPtr, RunIfClosureState,
        SlotScratchesCell, SystemClosureState, lifecycle_observer_closure_trampoline,
        observer_closure_trampoline, observer_mut_closure_trampoline, run_if_closure_trampoline,
        system_closure_slice_trampoline, system_closure_trampoline,
    },
};

// ============================================================================
// Shared Zero-Side-Effect Mock `BamHostApi` Stubs
// ============================================================================

unsafe extern "C" fn stub_get_proc(_: *const BamHostCtx, _: *const c_char) -> *const c_void {
    core::ptr::null()
}
unsafe extern "C" fn stub_last_error(_: *const BamHostCtx, _: *mut c_char, _: usize) -> usize {
    0
}
unsafe extern "C" fn stub_load_host_ctx(load_ctx: *const BamLoadCtx) -> *const BamHostCtx {
    load_ctx.cast::<BamHostCtx>()
}
unsafe extern "C" fn stub_load_host_ctx_null(_: *const BamLoadCtx) -> *const BamHostCtx {
    core::ptr::null()
}
unsafe extern "C" fn stub_register_comp(
    _: *mut BamLoadCtx,
    _: *const ComponentDesc,
    _: *mut BamComponent,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_resolve_comp(
    _: *const BamHostCtx,
    _: *const c_char,
    _: *const ComponentUuid,
    _: *mut BamComponent,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_register_sys(_: *mut BamLoadCtx, _: *const BamSystemDesc) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_sys_host_ctx(_: *const BamSystemCtx) -> *const BamHostCtx {
    core::ptr::null()
}
unsafe extern "C" fn stub_sys_query(
    _: *mut BamSystemCtx,
    _: usize,
    out: *mut *mut BamIter,
) -> FfiResult {
    if !out.is_null() {
        // SAFETY: Caller supplies writable stack storage for `out`.
        unsafe { *out = core::ptr::NonNull::<BamIter>::dangling().as_ptr() };
    }
    FfiResult::OK
}
unsafe extern "C" fn stub_sys_commands(
    _: *mut BamSystemCtx,
    out: *mut *mut BamCommands,
) -> FfiResult {
    if !out.is_null() {
        // SAFETY: Caller supplies writable stack storage for `out`.
        unsafe { *out = core::ptr::NonNull::<BamCommands>::dangling().as_ptr() };
    }
    FfiResult::OK
}
unsafe extern "C" fn stub_iter_next(_: *mut BamIter, _: *mut usize) -> FfiBool {
    FfiBool::FALSE
}
unsafe extern "C" fn stub_iter_reset(_: *mut BamIter) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_iter_entities(_: *const BamIter) -> *const FfiEntity {
    core::ptr::null()
}
unsafe extern "C" fn stub_iter_col_ro(_: *const BamIter, _: usize) -> *const u8 {
    core::ptr::null()
}
unsafe extern "C" fn stub_iter_col_mut(_: *mut BamIter, _: usize) -> *mut u8 {
    core::ptr::null_mut()
}
unsafe extern "C" fn stub_iter_mark(_: *mut BamIter, _: usize, _: usize, _: usize) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_iter_rows_ro(_: *mut BamIter, _: usize) -> *const *const u8 {
    core::ptr::null()
}
unsafe extern "C" fn stub_iter_rows_mut(_: *mut BamIter, _: usize) -> *const *mut u8 {
    core::ptr::null()
}
unsafe extern "C" fn stub_iter_info(
    _: *const BamIter,
    _: usize,
    _: *mut BamColumnInfo,
) -> FfiResult {
    FfiResult::NOT_FOUND
}
unsafe extern "C" fn stub_res_ptr(_: *const BamIter, _: usize) -> *const u8 {
    core::ptr::null()
}
unsafe extern "C" fn stub_res_mut_ptr(_: *mut BamIter, _: usize) -> *mut u8 {
    core::ptr::null_mut()
}
unsafe extern "C" fn stub_cmd_spawn_empty(_: *mut BamCommands, _: *mut FfiEntity) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_spawn(
    _: *mut BamCommands,
    _: *const BamComponent,
    _: *const *const u8,
    _: usize,
    _: *mut FfiEntity,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_despawn(_: *mut BamCommands, _: FfiEntity) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_insert(
    _: *mut BamCommands,
    _: FfiEntity,
    _: BamComponent,
    _: *const u8,
    _: usize,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_insert_default(
    _: *mut BamCommands,
    _: FfiEntity,
    _: BamComponent,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_remove(
    _: *mut BamCommands,
    _: FfiEntity,
    _: BamComponent,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_par_chunks(
    _: *mut BamIter,
    _: Option<bamabi_sys::BamChunkTaskFn>,
    _: *mut c_void,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_sys_entity_get(
    _: *const BamSystemCtx,
    _: usize,
    _: FfiEntity,
    _: usize,
) -> *const u8 {
    core::ptr::null()
}
unsafe extern "C" fn stub_sys_entity_get_mut(
    _: *mut BamSystemCtx,
    _: usize,
    _: FfiEntity,
    _: usize,
) -> *mut u8 {
    core::ptr::null_mut()
}
unsafe extern "C" fn stub_msg_read(
    _: *mut BamIter,
    _: usize,
    _: *mut u8,
    _: usize,
    _: *mut usize,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_msg_send(_: *mut BamIter, _: usize, _: *const u8, _: usize) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_msg_write(
    _: *mut BamIter,
    _: usize,
    _: *const u8,
    _: usize,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_res_ticks(
    _: *const BamIter,
    _: usize,
    _: *mut BamChangeTicks,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_sys_res_ptr(
    _: *const BamSystemCtx,
    _: BamComponent,
    _: *mut BamChangeTicks,
) -> *const u8 {
    core::ptr::null()
}
unsafe extern "C" fn stub_sys_res_mut_ptr(
    _: *mut BamSystemCtx,
    _: BamComponent,
    _: *mut BamChangeTicks,
) -> *mut u8 {
    core::ptr::null_mut()
}
unsafe extern "C" fn stub_register_obs(_: *mut BamLoadCtx, _: *const BamObserverDesc) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_trigger(
    _: *mut BamCommands,
    _: BamComponent,
    _: FfiEntity,
    _: *const u8,
    _: usize,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_spawn_batch(
    _: *mut BamCommands,
    _: *const BamComponent,
    _: *const *const u8,
    _: usize,
    _: usize,
    _: *mut FfiEntity,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_despawn_all(
    _: *mut BamCommands,
    _: *const BamTermDesc,
    _: usize,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_trigger_ex(
    _: *mut BamCommands,
    _: *const BamTriggerDesc,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_observe(
    _: *mut BamCommands,
    _: *const BamObserverDesc,
    _: *mut FfiEntity,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_iter_removed(
    _: *mut BamIter,
    _: usize,
    _: *mut FfiEntity,
    _: usize,
    _: *mut usize,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_sys_children(
    _: *const BamSystemCtx,
    _: FfiEntity,
    _: BamComponent,
    _: *mut *const FfiEntity,
    _: *mut usize,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_asset_load(
    _: *const BamHostCtx,
    _: BamComponent,
    _: StrView,
    _: *mut FfiAssetHandle,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_asset_state(
    _: *const BamHostCtx,
    _: *const FfiAssetHandle,
    _: *mut FfiAssetLoadState,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_asset_release(
    _: *const BamHostCtx,
    _: *const FfiAssetHandle,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_spawn_scene(
    _: *mut BamCommands,
    _: *const FfiAssetHandle,
    _: FfiEntity,
    _: *mut FfiEntity,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_log_diag(_: *const BamHostCtx, _: u32, _: StrView) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_report_fault(_: *const BamHostCtx, _: StrView) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_resolve_uuid(
    _: *const BamHostCtx,
    _: *const ComponentUuid,
    _: *mut BamComponent,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_layout_info(
    _: *const BamHostCtx,
    _: BamComponent,
    _: *mut BamLayoutInfo,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_register_rel(
    _: *mut BamLoadCtx,
    _: *const BamRelationDesc,
    _: *mut BamComponent,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_intern_sym(
    _: *const BamHostCtx,
    _: StrView,
    _: *mut BamSymbol,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_bind_named(_: *const BamHostCtx, _: StrView, _: FfiEntity) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_iter_count(_: *mut BamIter, _: *mut usize) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_iter_up_cascade(
    _: *mut BamIter,
    _: BamComponent,
    _: FfiEntity,
    _: usize,
    _: *mut *const c_void,
    _: *mut FfiEntity,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_register_sys_handle(
    _: *mut BamLoadCtx,
    _: *const BamSystemDesc,
    _: *mut BamSystem,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_config_set(
    _: *mut BamLoadCtx,
    _: *const BamScheduleSetDesc,
    _: *mut BamSystemSet,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_mount_sys(
    _: *mut BamLoadCtx,
    _: BamSystem,
    _: ScheduleId,
    _: BamSystemSet,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_checkpoint(_: *mut BamCommands, _: *mut usize) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_rollback(_: *mut BamCommands, _: usize) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_insert_rel(
    _: *mut BamCommands,
    _: FfiEntity,
    _: BamComponent,
    _: FfiEntity,
    _: *const c_void,
    _: usize,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_remove_rel(
    _: *mut BamCommands,
    _: FfiEntity,
    _: BamComponent,
    _: FfiEntity,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_migrate_col(
    _: *mut BamLoadCtx,
    _: BamComponent,
    _: u64,
    _: *const ComponentDesc,
    _: Option<bamabi_sys::BamRowMigrateFn>,
    _: *mut c_void,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_iter_next_chunk_batch(
    _: *mut BamIter,
    _: *mut usize,
    _: *mut *const FfiEntity,
    _: *mut *mut u8,
    _: usize,
    _: u32,
) -> FfiBool {
    FfiBool::FALSE
}
unsafe extern "C" fn stub_iter_column_ticks(
    _: *const BamIter,
    _: usize,
    _: *mut *const u32,
    _: *mut *const u32,
    _: *mut u32,
    _: *mut u32,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_clone_entity(
    _: *mut BamCommands,
    _: FfiEntity,
    _: FfiEntity,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_insert_resource(
    _: *mut BamCommands,
    _: BamComponent,
    _: *const u8,
    _: usize,
) -> FfiResult {
    FfiResult::OK
}
unsafe extern "C" fn stub_cmd_remove_resource(_: *mut BamCommands, _: BamComponent) -> FfiResult {
    FfiResult::OK
}

/// Builds a minimal `BamHostApi` populating only the 27 core ABI `0.2` slots and leaving all 46
/// extended/ABI-`0.3.0` slots as `None`.
fn make_core_api(struct_size: u32, abi_version: u32) -> BamHostApi {
    BamHostApi {
        struct_size,
        abi_version,
        get_proc_address: Some(stub_get_proc),
        last_error: Some(stub_last_error),
        load_host_ctx: Some(stub_load_host_ctx),
        register_component: Some(stub_register_comp),
        resolve_component: Some(stub_resolve_comp),
        register_system: Some(stub_register_sys),
        sys_host_ctx: Some(stub_sys_host_ctx),
        sys_query: Some(stub_sys_query),
        sys_commands: Some(stub_sys_commands),
        iter_next_chunk: Some(stub_iter_next),
        iter_reset: Some(stub_iter_reset),
        iter_entities: Some(stub_iter_entities),
        iter_column_readonly: Some(stub_iter_col_ro),
        iter_column_mutable: Some(stub_iter_col_mut),
        iter_column_mutable_untracked: Some(stub_iter_col_mut),
        iter_mark_changed_range: Some(stub_iter_mark),
        iter_column_rows_readonly: Some(stub_iter_rows_ro),
        iter_column_rows_mutable: Some(stub_iter_rows_mut),
        iter_column_info: Some(stub_iter_info),
        res_ptr: Some(stub_res_ptr),
        res_mut_ptr: Some(stub_res_mut_ptr),
        cmd_spawn_empty: Some(stub_cmd_spawn_empty),
        cmd_spawn: Some(stub_cmd_spawn),
        cmd_despawn: Some(stub_cmd_despawn),
        cmd_insert: Some(stub_cmd_insert),
        cmd_insert_default: Some(stub_cmd_insert_default),
        cmd_remove: Some(stub_cmd_remove),
        par_for_each_chunk: None,
        sys_entity_get: None,
        sys_entity_get_mut: None,
        msg_read_batch: None,
        msg_send: None,
        msg_write_batch: None,
        res_ticks: None,
        sys_res_ptr: None,
        sys_res_mut_ptr: None,
        register_observer: None,
        cmd_trigger: None,
        cmd_spawn_persistent: None,
        cmd_spawn_batch: None,
        cmd_despawn_all: None,
        cmd_insert_if_new: None,
        cmd_trigger_ex: None,
        cmd_observe: None,
        iter_read_removed: None,
        sys_entity_children: None,
        asset_load: None,
        asset_get_load_state: None,
        asset_release: None,
        cmd_spawn_scene: None,
        log_diagnostic: None,
        report_guest_fault: None,
        resolve_component_by_uuid: None,
        component_layout_info: None,
        register_relation: None,
        intern_symbol: None,
        bind_named_object: None,
        iter_count: None,
        iter_up_cascade: None,
        par_for_each_chunk_readonly: None,
        register_system_handle: None,
        configure_schedule_set: None,
        mount_system_in_schedule: None,
        cmd_checkpoint: None,
        cmd_rollback: None,
        cmd_insert_relation: None,
        cmd_remove_relation: None,
        migrate_component_column: None,
        iter_next_chunk_batch: None,
        iter_column_ticks: None,
        cmd_clone_entity: None,
        cmd_insert_resource: None,
        cmd_remove_resource: None,
    }
}

/// Builds a complete 73-slot `BamHostApi` populating all 27 core slots plus all 46 extended/ABI-`0.3.0`
/// slots with zero-side-effect stubs.
fn make_full_api(struct_size: u32, abi_version: u32) -> BamHostApi {
    let mut api = make_core_api(struct_size, abi_version);
    api.par_for_each_chunk = Some(stub_par_chunks);
    api.sys_entity_get = Some(stub_sys_entity_get);
    api.sys_entity_get_mut = Some(stub_sys_entity_get_mut);
    api.msg_read_batch = Some(stub_msg_read);
    api.msg_send = Some(stub_msg_send);
    api.msg_write_batch = Some(stub_msg_write);
    api.res_ticks = Some(stub_res_ticks);
    api.sys_res_ptr = Some(stub_sys_res_ptr);
    api.sys_res_mut_ptr = Some(stub_sys_res_mut_ptr);
    api.register_observer = Some(stub_register_obs);
    api.cmd_trigger = Some(stub_cmd_trigger);
    api.cmd_spawn_persistent = Some(stub_cmd_spawn);
    api.cmd_spawn_batch = Some(stub_cmd_spawn_batch);
    api.cmd_despawn_all = Some(stub_cmd_despawn_all);
    api.cmd_insert_if_new = Some(stub_cmd_insert);
    api.cmd_trigger_ex = Some(stub_cmd_trigger_ex);
    api.cmd_observe = Some(stub_cmd_observe);
    api.iter_read_removed = Some(stub_iter_removed);
    api.sys_entity_children = Some(stub_sys_children);
    api.asset_load = Some(stub_asset_load);
    api.asset_get_load_state = Some(stub_asset_state);
    api.asset_release = Some(stub_asset_release);
    api.cmd_spawn_scene = Some(stub_cmd_spawn_scene);
    api.log_diagnostic = Some(stub_log_diag);
    api.report_guest_fault = Some(stub_report_fault);
    api.resolve_component_by_uuid = Some(stub_resolve_uuid);
    api.component_layout_info = Some(stub_layout_info);
    api.register_relation = Some(stub_register_rel);
    api.intern_symbol = Some(stub_intern_sym);
    api.bind_named_object = Some(stub_bind_named);
    api.iter_count = Some(stub_iter_count);
    api.iter_up_cascade = Some(stub_iter_up_cascade);
    api.par_for_each_chunk_readonly = Some(stub_par_chunks);
    api.register_system_handle = Some(stub_register_sys_handle);
    api.configure_schedule_set = Some(stub_config_set);
    api.mount_system_in_schedule = Some(stub_mount_sys);
    api.cmd_checkpoint = Some(stub_cmd_checkpoint);
    api.cmd_rollback = Some(stub_cmd_rollback);
    api.cmd_insert_relation = Some(stub_cmd_insert_rel);
    api.cmd_remove_relation = Some(stub_cmd_remove_rel);
    api.migrate_component_column = Some(stub_migrate_col);
    api.iter_next_chunk_batch = Some(stub_iter_next_chunk_batch);
    api.iter_column_ticks = Some(stub_iter_column_ticks);
    api.cmd_clone_entity = Some(stub_cmd_clone_entity);
    api.cmd_insert_resource = Some(stub_cmd_insert_resource);
    api.cmd_remove_resource = Some(stub_cmd_remove_resource);
    api
}

/// Validates `api` against `ABI_VERSION` and constructs a `Host` preserving typed function
/// pointers for CBMC via `Host::from_api_for_kani`.
fn make_valid_host(api: BamHostApi, host_ctx: *const BamHostCtx) -> Host {
    assert_eq!(api.validate_for_guest(ABI_VERSION), FfiResult::OK);
    Host::from_api_for_kani(api, host_ctx)
}

// ============================================================================
// PO-G1: `host_slot!` Bounds & `Host` Handshake / VTable Guards
// ============================================================================

/// Asserts that all 41 extended/ABI-0.3 `host_slot!` lookups return `None` on `api`.
fn assert_all_41_extended_slots_none(api: &BamHostApi) {
    assert!(host_slot!(api, par_for_each_chunk).is_none());
    assert!(host_slot!(api, sys_entity_get).is_none());
    assert!(host_slot!(api, sys_entity_get_mut).is_none());
    assert!(host_slot!(api, msg_read_batch).is_none());
    assert!(host_slot!(api, msg_send).is_none());
    assert!(host_slot!(api, msg_write_batch).is_none());
    assert!(host_slot!(api, res_ticks).is_none());
    assert!(host_slot!(api, sys_res_ptr).is_none());
    assert!(host_slot!(api, sys_res_mut_ptr).is_none());
    assert!(host_slot!(api, register_observer).is_none());
    assert!(host_slot!(api, cmd_trigger).is_none());
    assert!(host_slot!(api, cmd_spawn_persistent).is_none());
    assert!(host_slot!(api, cmd_spawn_batch).is_none());
    assert!(host_slot!(api, cmd_despawn_all).is_none());
    assert!(host_slot!(api, cmd_insert_if_new).is_none());
    assert!(host_slot!(api, cmd_trigger_ex).is_none());
    assert!(host_slot!(api, cmd_observe).is_none());
    assert!(host_slot!(api, iter_read_removed).is_none());
    assert!(host_slot!(api, sys_entity_children).is_none());
    assert!(host_slot!(api, asset_load).is_none());
    assert!(host_slot!(api, asset_get_load_state).is_none());
    assert!(host_slot!(api, asset_release).is_none());
    assert!(host_slot!(api, cmd_spawn_scene).is_none());
    assert!(host_slot!(api, log_diagnostic).is_none());
    assert!(host_slot!(api, report_guest_fault).is_none());
    assert!(host_slot!(api, resolve_component_by_uuid).is_none());
    assert!(host_slot!(api, component_layout_info).is_none());
    assert!(host_slot!(api, register_relation).is_none());
    assert!(host_slot!(api, intern_symbol).is_none());
    assert!(host_slot!(api, bind_named_object).is_none());
    assert!(host_slot!(api, iter_count).is_none());
    assert!(host_slot!(api, iter_up_cascade).is_none());
    assert!(host_slot!(api, par_for_each_chunk_readonly).is_none());
    assert!(host_slot!(api, register_system_handle).is_none());
    assert!(host_slot!(api, configure_schedule_set).is_none());
    assert!(host_slot!(api, mount_system_in_schedule).is_none());
    assert!(host_slot!(api, cmd_checkpoint).is_none());
    assert!(host_slot!(api, cmd_rollback).is_none());
    assert!(host_slot!(api, cmd_insert_relation).is_none());
    assert!(host_slot!(api, cmd_remove_relation).is_none());
    assert!(host_slot!(api, migrate_component_column).is_none());
    assert!(host_slot!(api, iter_next_chunk_batch).is_none());
    assert!(host_slot!(api, iter_column_ticks).is_none());
    assert!(host_slot!(api, cmd_clone_entity).is_none());
    assert!(host_slot!(api, cmd_insert_resource).is_none());
    assert!(host_slot!(api, cmd_remove_resource).is_none());
}

/// PO-G1: `host_slot!` across all 46 extended/ABI-0.3 slots returns `Some` iff `populated` and
/// `struct_size >= offset_of!(BamHostApi, slot) + size_of::<usize>()`, with zero overflow.
#[kani::proof]
fn po_g1_host_slot_bounds_all_41_extended_slots() {
    let struct_size: u32 = kani::any();
    let populated: bool = kani::any();
    let api = if populated {
        make_full_api(struct_size, ABI_VERSION)
    } else {
        make_core_api(struct_size, ABI_VERSION)
    };

    macro_rules! check_slot {
        ($slot:ident) => {{
            let end = offset_of!(BamHostApi, $slot) + size_of::<usize>();
            let expected = populated && (struct_size as usize) >= end;
            assert_eq!(host_slot!(&api, $slot).is_some(), expected);
        }};
    }

    check_slot!(par_for_each_chunk);
    check_slot!(sys_entity_get);
    check_slot!(sys_entity_get_mut);
    check_slot!(msg_read_batch);
    check_slot!(msg_send);
    check_slot!(msg_write_batch);
    check_slot!(res_ticks);
    check_slot!(sys_res_ptr);
    check_slot!(sys_res_mut_ptr);
    check_slot!(register_observer);
    check_slot!(cmd_trigger);
    check_slot!(cmd_spawn_persistent);
    check_slot!(cmd_spawn_batch);
    check_slot!(cmd_despawn_all);
    check_slot!(cmd_insert_if_new);
    check_slot!(cmd_trigger_ex);
    check_slot!(cmd_observe);
    check_slot!(iter_read_removed);
    check_slot!(sys_entity_children);
    check_slot!(asset_load);
    check_slot!(asset_get_load_state);
    check_slot!(asset_release);
    check_slot!(cmd_spawn_scene);
    check_slot!(log_diagnostic);
    check_slot!(report_guest_fault);
    check_slot!(resolve_component_by_uuid);
    check_slot!(component_layout_info);
    check_slot!(register_relation);
    check_slot!(intern_symbol);
    check_slot!(bind_named_object);
    check_slot!(iter_count);
    check_slot!(iter_up_cascade);
    check_slot!(par_for_each_chunk_readonly);
    check_slot!(register_system_handle);
    check_slot!(configure_schedule_set);
    check_slot!(mount_system_in_schedule);
    check_slot!(cmd_checkpoint);
    check_slot!(cmd_rollback);
    check_slot!(cmd_insert_relation);
    check_slot!(cmd_remove_relation);
    check_slot!(migrate_component_column);
    check_slot!(iter_next_chunk_batch);
    check_slot!(iter_column_ticks);
    check_slot!(cmd_clone_entity);
    check_slot!(cmd_insert_resource);
    check_slot!(cmd_remove_resource);

    kani::cover!(host_slot!(&api, par_for_each_chunk).is_some());
    kani::cover!(host_slot!(&api, par_for_each_chunk).is_none());
    kani::cover!(host_slot!(&api, iter_next_chunk_batch).is_some());
    kani::cover!(host_slot!(&api, iter_next_chunk_batch).is_none());
    kani::cover!(host_slot!(&api, cmd_remove_resource).is_some());
    kani::cover!(host_slot!(&api, cmd_remove_resource).is_none());
}

/// PO-G1: `Host::from_raw` and `Host::from_load_ctx` reject null pointers, misaligned `api_ptr`,
/// undersized `struct_size`, incompatible `abi_version`, missing mandatory core slots, and null
/// `load_host_ctx` returns without UB, while proving an exact `BamHostApi::MIN_V0_2_SIZE`
/// truncated stack buffer triggers zero out-of-bounds reads and leaves all 46 extended slots `None`.
#[kani::proof]
fn po_g1_host_from_raw_and_load_ctx_guards() {
    #[repr(C)]
    struct AlignedV02ApiBuf {
        _align: [BamHostApi; 0],
        bytes: [u8; BamHostApi::MIN_V0_2_SIZE],
    }

    let mut dummy_load = 1u8;
    let host_ctx = (&raw const dummy_load).cast::<BamHostCtx>();
    let load_ctx = (&raw mut dummy_load).cast::<BamLoadCtx>();

    let valid_api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);

    // Null checks (never dereference).
    // SAFETY: Null pointers are explicitly rejected before dereferencing.
    assert_eq!(
        unsafe { Host::from_raw(core::ptr::null(), host_ctx) }.unwrap_err(),
        FfiResult::NULL_POINTER
    );
    // SAFETY: Null `host_ctx` is rejected before dereferencing.
    assert_eq!(
        unsafe { Host::from_raw(&valid_api, core::ptr::null()) }.unwrap_err(),
        FfiResult::NULL_POINTER
    );
    // SAFETY: Null pointers are explicitly rejected before dereferencing.
    assert_eq!(
        unsafe { Host::from_load_ctx(core::ptr::null(), load_ctx) }.unwrap_err(),
        FfiResult::NULL_POINTER
    );
    // SAFETY: Null `load_ctx` is rejected before dereferencing.
    assert_eq!(
        unsafe { Host::from_load_ctx(&valid_api, core::ptr::null_mut()) }.unwrap_err(),
        FfiResult::NULL_POINTER
    );

    // Misaligned non-null `api_ptr` check (offsets 1..align_of::<BamHostApi>()).
    let unaligned_storage = [0u64; 2];
    let misalign: usize = kani::any();
    kani::assume(misalign >= 1 && misalign < align_of::<BamHostApi>());
    let bad_api_ptr = core::ptr::from_ref(&unaligned_storage)
        .cast::<u8>()
        .wrapping_add(misalign)
        .cast::<BamHostApi>();
    // SAFETY: Alignment guard rejects `bad_api_ptr` before reading memory.
    assert_eq!(
        unsafe { Host::from_raw(bad_api_ptr, host_ctx) }.unwrap_err(),
        FfiResult::INVALID_ARGUMENT
    );
    // SAFETY: Alignment guard rejects `bad_api_ptr` before reading memory.
    assert_eq!(
        unsafe { Host::from_load_ctx(bad_api_ptr, load_ctx) }.unwrap_err(),
        FfiResult::INVALID_ARGUMENT
    );

    // Exact truncated v0.2 `[u8; BamHostApi::MIN_V0_2_SIZE]` aligned stack buffer test.
    let sz: u32 = kani::any();
    kani::assume((sz as usize) <= BamHostApi::MIN_V0_2_SIZE);
    let ver: u32 = kani::any();
    let drop_core_slot: bool = kani::any();
    let null_load_ctx_ret: bool = kani::any();

    let mut sym_api = make_core_api(sz, ver);
    if drop_core_slot {
        sym_api.cmd_remove = None;
    }
    if null_load_ctx_ret {
        sym_api.load_host_ctx = Some(stub_load_host_ctx_null);
    }

    let mut trunc_buf = AlignedV02ApiBuf {
        _align: [],
        bytes: [0u8; BamHostApi::MIN_V0_2_SIZE],
    };
    // SAFETY: `sym_api` is initialized and `trunc_buf.bytes` holds exact `MIN_V0_2_SIZE` bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(
            (&raw const sym_api).cast::<u8>(),
            trunc_buf.bytes.as_mut_ptr(),
            BamHostApi::MIN_V0_2_SIZE,
        );
    }
    let trunc_api_ptr = (&raw const trunc_buf).cast::<BamHostApi>();

    // SAFETY: `trunc_api_ptr` is aligned and readable for `BamHostApi::MIN_V0_2_SIZE >= sz` bytes.
    let res_raw = unsafe { Host::from_raw(trunc_api_ptr, host_ctx) };
    if (sz as usize) < BamHostApi::MIN_V0_2_SIZE || !abi_version_is_compatible(ver, ABI_VERSION) {
        assert_eq!(res_raw.unwrap_err(), FfiResult::INVALID_VERSION);
    } else if drop_core_slot {
        assert_eq!(res_raw.unwrap_err(), FfiResult::NULL_POINTER);
    } else {
        let host = res_raw.expect("valid truncated v0.2 api must succeed");
        assert_eq!(host.api().struct_size, sz);
        assert_eq!(host.api().abi_version, ver);
        assert_eq!(host.host_ctx(), host_ctx);
        assert!(host.api().has_all_v0_2_slots());
        assert_all_41_extended_slots_none(host.api());
    }
    kani::cover!(res_raw.is_ok());
    kani::cover!(matches!(res_raw, Err(FfiResult::INVALID_VERSION)));
    kani::cover!(matches!(res_raw, Err(FfiResult::NULL_POINTER)));

    // SAFETY: `trunc_api_ptr` is aligned and readable for `BamHostApi::MIN_V0_2_SIZE >= sz` bytes.
    let res_load = unsafe { Host::from_load_ctx(trunc_api_ptr, load_ctx) };
    if (sz as usize) < BamHostApi::MIN_V0_2_SIZE || !abi_version_is_compatible(ver, ABI_VERSION) {
        assert_eq!(res_load.unwrap_err(), FfiResult::INVALID_VERSION);
    } else if drop_core_slot || null_load_ctx_ret {
        assert_eq!(res_load.unwrap_err(), FfiResult::NULL_POINTER);
    } else {
        let host = res_load.expect("valid truncated v0.2 api load_ctx must succeed");
        assert_eq!(host.api().struct_size, sz);
        assert_eq!(host.api().abi_version, ver);
        assert!(host.api().has_all_v0_2_slots());
        assert_all_41_extended_slots_none(host.api());
    }
    kani::cover!(res_load.is_ok());
    kani::cover!(matches!(res_load, Err(FfiResult::INVALID_VERSION)));
    kani::cover!(matches!(res_load, Err(FfiResult::NULL_POINTER)));
}

/// PO-G1: `Host::resolve_vtable` rejects undersized target types, null pointers, misaligned
/// pointers, undersized `VTableHeader::struct_size`, incompatible `abi_version`, and mismatched
/// UUIDs strictly before forming `&V`.
#[kani::proof]
fn po_g1_host_resolve_vtable_guards() {
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    struct DummyVTable {
        header: VTableHeader,
        tag: u64,
    }

    #[repr(C)]
    struct ProcCtx {
        returned_ptr: *const c_void,
    }

    unsafe extern "C" fn mock_get_proc(ctx: *const BamHostCtx, _: *const c_char) -> *const c_void {
        // SAFETY: Test binds `ctx` to `&ProcCtx`.
        unsafe { (*ctx.cast::<ProcCtx>()).returned_ptr }
    }

    let mut api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    api.get_proc_address = Some(mock_get_proc);

    let sym: &CStr = CStr::from_bytes_with_nul(b"vt\0").unwrap();
    let expected_uuid = ComponentUuid::from_bytes([1; 16]);

    // 1. Target type smaller than VTableHeader::MIN_V0_2_SIZE rejected before FFI call.
    let ctx_null = ProcCtx {
        returned_ptr: core::ptr::null(),
    };
    let host_null = make_valid_host(api, (&raw const ctx_null).cast::<BamHostCtx>());
    // SAFETY: Size guard rejects `u64` before calling `get_proc_address`.
    assert_eq!(
        unsafe { host_null.resolve_vtable::<u64>(sym, expected_uuid) }.unwrap_err(),
        FfiResult::INVALID_ARGUMENT
    );
    // 2. Null proc address -> NOT_FOUND.
    // SAFETY: Null return is checked before reading header.
    assert_eq!(
        unsafe { host_null.resolve_vtable::<DummyVTable>(sym, expected_uuid) }.unwrap_err(),
        FfiResult::NOT_FOUND
    );

    // 3. Misaligned non-null pointer -> INVALID_ARGUMENT without dereferencing.
    let storage = [0u64; 2];
    let misalign: usize = kani::any();
    kani::assume(misalign >= 1 && misalign < align_of::<DummyVTable>());
    let ctx_unaligned = ProcCtx {
        returned_ptr: core::ptr::from_ref(&storage)
            .cast::<u8>()
            .wrapping_add(misalign)
            .cast::<c_void>(),
    };
    let host_unaligned = make_valid_host(api, (&raw const ctx_unaligned).cast::<BamHostCtx>());
    // SAFETY: Alignment guard rejects pointer before dereferencing.
    assert_eq!(
        unsafe { host_unaligned.resolve_vtable::<DummyVTable>(sym, expected_uuid) }.unwrap_err(),
        FfiResult::INVALID_ARGUMENT
    );

    // 4. Symbolic header fields on a live aligned `DummyVTable`.
    let hdr_size: u32 = kani::any();
    let hdr_ver: u32 = kani::any();
    let uuid_matches: bool = kani::any();
    let tag: u64 = kani::any();
    let vt = DummyVTable {
        header: VTableHeader {
            struct_size: hdr_size,
            abi_version: hdr_ver,
            uuid: if uuid_matches {
                expected_uuid
            } else {
                ComponentUuid::from_bytes([2; 16])
            },
            size: 8,
            align: 8,
        },
        tag,
    };
    let ctx_vt = ProcCtx {
        returned_ptr: core::ptr::from_ref(&vt).cast::<c_void>(),
    };
    let host_vt = make_valid_host(api, (&raw const ctx_vt).cast::<BamHostCtx>());
    // SAFETY: `vt` is aligned, initialized, and prefixed by `VTableHeader`.
    let res = unsafe { host_vt.resolve_vtable::<DummyVTable>(sym, expected_uuid) };
    if (hdr_size as usize) < size_of::<DummyVTable>()
        || !abi_version_is_compatible(hdr_ver, ABI_VERSION)
    {
        assert_eq!(res.unwrap_err(), FfiResult::INVALID_VERSION);
    } else if !uuid_matches {
        assert_eq!(res.unwrap_err(), FfiResult::TYPE_MISMATCH);
    } else {
        let resolved = res.expect("valid VTable must resolve");
        assert_eq!(resolved.tag, tag);
        assert_eq!(resolved.header.uuid, expected_uuid);
    }
    kani::cover!(res.is_ok());
    kani::cover!(matches!(res, Err(FfiResult::INVALID_VERSION)));
    kani::cover!(matches!(res, Err(FfiResult::TYPE_MISMATCH)));
}

// ============================================================================
// PO-G3: Dense Slice Offset, Alignment, Overflow & Info Validation
// ============================================================================

/// PO-G3: `checked_slice_elem_offset` never overflows and succeeds iff `valid`, `index < len`,
/// `expected_size == Some(elem_stride)`, and `len * elem_stride <= isize::MAX`.
///
/// To avoid SAT bit-blasting blowup from unconstrained 64×64→128-bit symbolic multiplication,
/// verifies exact equivalence over the entire 64-bit `(len, index) ∈ usize × usize` space across
/// 22 compile-time-constant stride classes (ZST `0`, odd/even/power-of-two realistic strides, and
/// 32/63/64-bit overflow boundary strides), using exact division thresholds `isize::MAX / STRIDE`,
/// plus a fully symbolic 8-bit `(len, index, elem_stride)` check.
#[kani::proof]
fn po_g3_checked_slice_elem_offset_exact_bounds() {
    fn check_stride_case<const STRIDE: usize>() {
        let valid: bool = kani::any();
        let len: usize = kani::any();
        let expected_size: Option<usize> = kani::any();
        let index: usize = kani::any();

        let res = checked_slice_elem_offset(valid, len, expected_size, index, STRIDE);
        let max_len = if STRIDE == 0 {
            usize::MAX
        } else {
            (isize::MAX as usize) / STRIDE
        };
        let should_ok = valid && index < len && expected_size == Some(STRIDE) && len <= max_len;

        assert_eq!(res.is_some(), should_ok);
        if let Some(off) = res {
            assert!(index < len);
            assert!(len <= max_len);
            assert!(off <= isize::MAX as usize);
            if STRIDE == 0 {
                assert_eq!(off, 0);
            } else {
                let end_byte = off
                    .checked_add(STRIDE)
                    .expect("element end offset cannot overflow");
                assert!(end_byte <= isize::MAX as usize);
                assert_eq!(off / STRIDE, index);
                assert_eq!(off % STRIDE, 0);
            }
        }
    }

    let case_idx: u8 = kani::any();
    kani::assume(case_idx < 22);
    match case_idx {
        0 => check_stride_case::<0>(),
        1 => check_stride_case::<1>(),
        2 => check_stride_case::<2>(),
        3 => check_stride_case::<3>(),
        4 => check_stride_case::<4>(),
        5 => check_stride_case::<7>(),
        6 => check_stride_case::<8>(),
        7 => check_stride_case::<12>(),
        8 => check_stride_case::<16>(),
        9 => check_stride_case::<24>(),
        10 => check_stride_case::<32>(),
        11 => check_stride_case::<48>(),
        12 => check_stride_case::<64>(),
        13 => check_stride_case::<128>(),
        14 => check_stride_case::<256>(),
        15 => check_stride_case::<4096>(),
        16 => check_stride_case::<{ 1usize << 31 }>(),
        17 => check_stride_case::<{ (1usize << 32) + 1 }>(),
        18 => check_stride_case::<{ (isize::MAX as usize) / 2 }>(),
        19 => check_stride_case::<{ (isize::MAX as usize) / 2 + 1 }>(),
        20 => check_stride_case::<{ isize::MAX as usize }>(),
        _ => check_stride_case::<{ usize::MAX }>(),
    }

    // Also check arbitrary symbolic small (len, index, elem_stride) in u8^3 with no fixed stride.
    let s_valid: bool = kani::any();
    let s_len = kani::any::<u8>() as usize;
    let s_idx = kani::any::<u8>() as usize;
    let s_stride = kani::any::<u8>() as usize;
    let s_exp = if kani::any() {
        Some(kani::any::<u8>() as usize)
    } else {
        None
    };
    let s_res = checked_slice_elem_offset(s_valid, s_len, s_exp, s_idx, s_stride);
    let s_ok = s_valid && s_idx < s_len && s_exp == Some(s_stride);
    assert_eq!(s_res.is_some(), s_ok);
    if let Some(off) = s_res {
        assert_eq!(off, s_idx * s_stride);
        assert!(off + s_stride <= s_len * s_stride);
    }
    kani::cover!(s_res.is_some());
    kani::cover!(s_res.is_none());
}

/// PO-G3: `raw_dense_slice` and `raw_dense_slice_mut` reject null pointers, misaligned pointers,
/// and overflowing/`> isize::MAX` byte lengths for non-ZSTs, handle ZSTs safely, and preserve
/// exact slice elements for valid aligned buffers.
#[kani::proof]
fn po_g3_raw_dense_slice_and_mut_soundness() {
    // 1. Null non-ZST pointer always returns None.
    let any_len: usize = kani::any();
    // SAFETY: Null pointer is rejected before creating any slice.
    assert!(unsafe { raw_dense_slice::<u64>(core::ptr::null(), any_len) }.is_none());
    // SAFETY: Null pointer is rejected before creating any slice.
    assert!(unsafe { raw_dense_slice_mut::<u64>(core::ptr::null_mut(), any_len) }.is_none());

    // 2. Misaligned non-null pointer with len > 0 always returns None.
    let mut backing = [0u64; 4];
    let misalign: usize = kani::any();
    kani::assume(misalign >= 1 && misalign < align_of::<u64>());
    let nz_len: usize = kani::any();
    kani::assume(nz_len > 0);
    let bad_ro = core::ptr::from_ref(&backing)
        .cast::<u8>()
        .wrapping_add(misalign);
    let bad_mut = core::ptr::from_mut(&mut backing)
        .cast::<u8>()
        .wrapping_add(misalign);
    // SAFETY: Alignment guard rejects unaligned pointer before creating any slice.
    assert!(unsafe { raw_dense_slice::<u64>(bad_ro, nz_len) }.is_none());
    // SAFETY: Alignment guard rejects unaligned pointer before creating any slice.
    assert!(unsafe { raw_dense_slice_mut::<u64>(bad_mut, nz_len) }.is_none());

    // 3. Overflow / > isize::MAX byte length always returns None.
    let huge_len: usize = kani::any();
    kani::assume(
        huge_len
            .checked_mul(size_of::<u64>())
            .is_none_or(|b| b > isize::MAX as usize),
    );
    // SAFETY: Bounds guard rejects excessive byte length before creating any slice.
    assert!(unsafe { raw_dense_slice::<u64>(backing.as_ptr().cast::<u8>(), huge_len) }.is_none());
    // SAFETY: Bounds guard rejects excessive byte length before creating any slice.
    assert!(
        unsafe { raw_dense_slice_mut::<u64>(backing.as_mut_ptr().cast::<u8>(), huge_len) }
            .is_none()
    );

    // 4. Valid aligned slice read & write for bounded symbolic len 0..=4.
    let init: [u64; 4] = kani::any();
    backing = init;
    let len: usize = kani::any();
    kani::assume(len <= backing.len());
    // SAFETY: `backing` is aligned and holds `len <= 4` initialized `u64`s.
    let ro = unsafe { raw_dense_slice::<u64>(backing.as_ptr().cast::<u8>(), len) }
        .expect("valid aligned read slice must succeed");
    assert_eq!(ro.len(), len);
    if len > 0 {
        let idx: usize = kani::any();
        kani::assume(idx < len);
        assert_eq!(ro[idx], init[idx]);
    }
    // SAFETY: `backing` is uniquely owned and aligned for `len <= 4` `u64`s.
    let rw = unsafe { raw_dense_slice_mut::<u64>(backing.as_mut_ptr().cast::<u8>(), len) }
        .expect("valid aligned mut slice must succeed");
    assert_eq!(rw.len(), len);

    // 5. ZST `ZstPod` checks: (null, len > 0) rejected; (dangling, any_len) accepted cleanly.
    // SAFETY: Null ZST with `nz_len > 0` returns None.
    assert!(unsafe { raw_dense_slice::<ZstPod>(core::ptr::null(), nz_len) }.is_none());
    // SAFETY: Null ZST with `nz_len > 0` returns None.
    assert!(unsafe { raw_dense_slice_mut::<ZstPod>(core::ptr::null_mut(), nz_len) }.is_none());
    let zst_len: usize = kani::any();
    // SAFETY: ZST uses `NonNull::dangling()`.
    let zst_ro = unsafe {
        raw_dense_slice::<ZstPod>(core::ptr::NonNull::<u8>::dangling().as_ptr(), zst_len)
    }
    .expect("ZST with non-null ptr succeeds");
    assert_eq!(zst_ro.len(), zst_len);
    // SAFETY: ZST uses `NonNull::dangling()`.
    let zst_rw = unsafe {
        raw_dense_slice_mut::<ZstPod>(core::ptr::NonNull::<u8>::dangling().as_ptr(), zst_len)
    }
    .expect("ZST mut with non-null ptr succeeds");
    assert_eq!(zst_rw.len(), zst_len);

    kani::cover!(len > 0);
    kani::cover!(len == 0);
    // SAFETY: Null pointer is rejected before creating any slice.
    kani::cover!(unsafe { raw_dense_slice::<u64>(core::ptr::null(), any_len) }.is_none());
}

/// PO-G3: `ValidatedSlice` and `ValidatedSliceMut` enforce mode, storage flags, immutability,
/// size/alignment/UUID matching, and bounds-checked `get_ptr` / `get_mut_ptr`.
#[kani::proof]
fn po_g3_validated_slice_and_mut_info_guards() {
    let mut buf = [11u32, 22, 33, 44];
    let uuid_ok = ComponentUuid::from_bytes([7; 16]);
    let uuid_bad = ComponentUuid::from_bytes([9; 16]);

    let info = BamColumnInfo {
        component: BamComponent::from_raw(1),
        mode: AccessMode(kani::any()),
        uuid: if kani::any() { uuid_ok } else { uuid_bad },
        size: kani::any(),
        align: kani::any(),
        storage_flags: kani::any(),
        reserved0: 0,
    };

    // SAFETY: `buf` holds 4 initialized `u32` values (16 bytes) for the duration of this test;
    // `as_typed_slice_checked` only casts to `u32` when `info.size == 4 && info.align == 4`.
    let ro_slice =
        unsafe { ValidatedSlice::from_raw_parts_with_info(buf.as_ptr().cast::<u8>(), 4, info) };
    let ro_typed = ro_slice.as_typed_slice_checked::<u32>(Some(UuidFilter::exact(uuid_ok)));
    let expect_ro = info.mode.has_entity_column_data()
        && !info.is_resource()
        && !info.is_sparse_set()
        && info.size == size_of::<u32>()
        && info.align == align_of::<u32>()
        && info.uuid == uuid_ok;
    assert_eq!(ro_typed.is_some(), expect_ro);

    // SAFETY: `buf` is uniquely owned and `as_typed_slice_mut_checked` verifies `size == 4 && align == 4`.
    let mut rw_slice = unsafe {
        ValidatedSliceMut::from_raw_parts_with_info(buf.as_mut_ptr().cast::<u8>(), 4, info)
    };
    let rw_typed = rw_slice.as_typed_slice_mut_checked::<u32>(Some(UuidFilter::exact(uuid_ok)));
    let expect_rw = expect_ro && info.mode.is_mutable() && !info.is_immutable();
    assert_eq!(rw_typed.is_some(), expect_rw);

    // `get_ptr` / `get_mut_ptr` with arbitrary symbolic `index` and `stride` never overflows or goes OOB.
    let valid_info = BamColumnInfo {
        component: BamComponent::from_raw(1),
        mode: AccessMode::WRITE,
        uuid: uuid_ok,
        size: size_of::<u32>(),
        align: align_of::<u32>(),
        storage_flags: 0,
        reserved0: 0,
    };
    // SAFETY: `buf` holds 4 `u32` elements matching `valid_info`.
    let mut checked_mut = unsafe {
        ValidatedSliceMut::from_raw_parts_with_info(buf.as_mut_ptr().cast::<u8>(), 4, valid_info)
    };
    let idx: usize = kani::any();
    let stride: usize = kani::any();
    let ptr_opt = checked_mut.get_mut_ptr(idx, stride);
    assert_eq!(ptr_opt.is_some(), idx < 4 && stride == size_of::<u32>());

    kani::cover!(ro_typed.is_some());
    kani::cover!(ro_typed.is_none());
    kani::cover!(rw_typed.is_some());
    kani::cover!(rw_typed.is_none());
    kani::cover!(ptr_opt.is_some());
    kani::cover!(ptr_opt.is_none());
}

// ============================================================================
// PO-G4: `ChunkColumnDispenser` Disjoint Borrow & Anti-Aliasing Proof
// ============================================================================

/// PO-G4: `ChunkColumnDispenser::check_and_record_borrow` never overflows, strictly enforces
/// mutual exclusion on component handle and byte interval against any mutable borrow and against
/// `entities_range`, permits shared read-only borrows, and caps at `MAX_TERMS_PER_QUERY`.
#[kani::proof]
#[kani::unwind(6)]
fn po_g4_dispenser_borrow_disjointness_and_entities_guard() {
    let dummy_ctx = 1u8;
    let api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    let host = make_valid_host(api, (&raw const dummy_ctx).cast::<BamHostCtx>());
    let mut iter = ValidatedIter::empty(&host);
    let mut dispenser = ChunkColumnDispenser::new(&mut iter);

    // Optional symbolic entities byte range [es, ee).
    let has_entities: bool = kani::any();
    let ent_start: usize = kani::any();
    let ent_end: usize = kani::any();
    if has_entities {
        kani::assume(ent_start <= ent_end);
        dispenser.set_entities_range(Some((ent_start, ent_end)));
    }

    // Borrow 1
    let c1 = BamComponent(kani::any());
    let s1: usize = kani::any();
    let l1: usize = kani::any();
    let m1: bool = kani::any();
    let r1 = dispenser.check_and_record_borrow(c1, s1, l1, m1);
    kani::cover!(r1.is_ok());
    kani::cover!(r1.is_err());

    let e1_opt = s1.checked_add(l1);
    if e1_opt.is_none() {
        assert_eq!(r1, Err(FfiResult::OUT_OF_RANGE));
        assert_eq!(dispenser.borrowed_count(), 0);
        return;
    }
    let e1 = e1_opt.unwrap();
    let overlaps_ent1 =
        has_entities && m1 && l1 > 0 && ent_end > ent_start && s1 < ent_end && ent_start < e1;
    if overlaps_ent1 {
        assert_eq!(r1, Err(FfiResult::ACCESS_CONFLICT));
        assert_eq!(dispenser.borrowed_count(), 0);
        return;
    }
    assert_eq!(r1, Ok(()));
    assert_eq!(dispenser.borrowed_count(), 1);

    // Borrow 2
    let c2 = BamComponent(kani::any());
    let s2: usize = kani::any();
    let l2: usize = kani::any();
    let m2: bool = kani::any();
    let r2 = dispenser.check_and_record_borrow(c2, s2, l2, m2);
    kani::cover!(r2.is_ok());
    kani::cover!(r2 == Err(FfiResult::ACCESS_CONFLICT));
    kani::cover!(r2 == Err(FfiResult::OUT_OF_RANGE));

    let e2_opt = s2.checked_add(l2);
    if e2_opt.is_none() {
        assert_eq!(r2, Err(FfiResult::OUT_OF_RANGE));
        assert_eq!(dispenser.borrowed_count(), 1);
        return;
    }
    let e2 = e2_opt.unwrap();
    let overlaps_ent2 =
        has_entities && m2 && l2 > 0 && ent_end > ent_start && s2 < ent_end && ent_start < e2;
    let conflicts_12 = (m1 || m2) && (c1 == c2 || (l1 > 0 && l2 > 0 && s1 < e2 && s2 < e1));

    if overlaps_ent2 || conflicts_12 {
        assert_eq!(r2, Err(FfiResult::ACCESS_CONFLICT));
        assert_eq!(dispenser.borrowed_count(), 1);
    } else {
        assert_eq!(r2, Ok(()));
        assert_eq!(dispenser.borrowed_count(), 2);
        // Postcondition: if either borrow is mutable, they are strictly disjoint in component & bytes!
        if m1 || m2 {
            assert_ne!(c1, c2);
            if l1 > 0 && l2 > 0 {
                assert!(e1 <= s2 || e2 <= s1);
            }
        }
        if has_entities && m2 && l2 > 0 && ent_end > ent_start {
            assert!(e2 <= ent_start || ent_end <= s2);
        }
    }
}

/// PO-G4 (capacity bound): Recording more than `MAX_TERMS_PER_QUERY` (32) distinct borrows
/// deterministically returns `Err(CAPACITY_EXCEEDED)` without out-of-bounds indexing.
#[kani::proof]
#[kani::unwind(35)]
fn po_g4_dispenser_capacity_limit() {
    let dummy_ctx = 1u8;
    let api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    let host = make_valid_host(api, (&raw const dummy_ctx).cast::<BamHostCtx>());
    let mut iter = ValidatedIter::empty(&host);
    let mut dispenser = ChunkColumnDispenser::new(&mut iter);

    let mut i = 0usize;
    while i < MAX_TERMS_PER_QUERY {
        let comp = BamComponent::from_raw((i + 1) as u32);
        assert_eq!(
            dispenser.check_and_record_borrow(comp, i * 16, 8, false),
            Ok(())
        );
        i += 1;
    }
    assert_eq!(dispenser.borrowed_count(), MAX_TERMS_PER_QUERY);
    let over = dispenser.check_and_record_borrow(BamComponent::from_raw(99), 1024, 8, false);
    assert_eq!(over, Err(FfiResult::CAPACITY_EXCEEDED));
    assert_eq!(dispenser.borrowed_count(), MAX_TERMS_PER_QUERY);
    kani::cover!(dispenser.borrowed_count() == MAX_TERMS_PER_QUERY);
    kani::cover!(over == Err(FfiResult::CAPACITY_EXCEEDED));
}

/// PO-G4 (hostile overlapping host pointers): End-to-end verification that `next_write` followed
/// by an overlapping `next_read` (at arbitrary symbolic element offset `0..4`) rejects with
/// `ACCESS_CONFLICT` strictly BEFORE forming an aliasing `&[u32]` reference, preserving soundness
/// of the live `&mut [u32]`.
#[kani::proof]
#[kani::unwind(6)]
fn po_g4_dispenser_overlapping_write_then_read_soundness() {
    #[repr(C)]
    struct HostileIterState {
        buf: [u32; 4],
        read_offset_elems: usize,
        yielded: bool,
    }

    unsafe extern "C" fn mock_next(iter: *mut BamIter, out_len: *mut usize) -> FfiBool {
        // SAFETY: Test binds `iter` to `HostileIterState`.
        let st = unsafe { &mut *iter.cast::<HostileIterState>() };
        if st.yielded {
            return FfiBool::FALSE;
        }
        st.yielded = true;
        // SAFETY: Caller provides writable stack `out_len`.
        unsafe { *out_len = 2 };
        FfiBool::TRUE
    }

    unsafe extern "C" fn mock_info(
        _: *const BamIter,
        term_index: usize,
        out: *mut BamColumnInfo,
    ) -> FfiResult {
        let info = match term_index {
            0 => BamColumnInfo {
                component: BamComponent::from_raw(1),
                mode: AccessMode::WRITE,
                uuid: ComponentUuid::from_bytes([1; 16]),
                size: size_of::<u32>(),
                align: align_of::<u32>(),
                storage_flags: 0,
                reserved0: 0,
            },
            1 => BamColumnInfo {
                component: BamComponent::from_raw(2),
                mode: AccessMode::READ,
                uuid: ComponentUuid::from_bytes([2; 16]),
                size: size_of::<u32>(),
                align: align_of::<u32>(),
                storage_flags: 0,
                reserved0: 0,
            },
            _ => return FfiResult::OUT_OF_RANGE,
        };
        // SAFETY: Caller supplies writable `out`.
        unsafe { *out = info };
        FfiResult::OK
    }

    unsafe extern "C" fn mock_col_mut(iter: *mut BamIter, term_index: usize) -> *mut u8 {
        if term_index != 0 {
            return core::ptr::null_mut();
        }
        // SAFETY: Test binds `iter` to `HostileIterState`; mutable slice covers `buf[0..2]`.
        let st = unsafe { &mut *iter.cast::<HostileIterState>() };
        st.buf.as_mut_ptr().cast::<u8>()
    }

    unsafe extern "C" fn mock_col_ro(iter: *const BamIter, term_index: usize) -> *const u8 {
        if term_index != 1 {
            return core::ptr::null();
        }
        // SAFETY: Test binds `iter` to `HostileIterState`; returns `buf[read_offset_elems..+2]`.
        let st = unsafe { &*iter.cast::<HostileIterState>() };
        // SAFETY: `read_offset_elems <= 2` guarantees `buf.as_ptr().add(read_offset_elems)` is in-bounds for 2 elements.
        unsafe { st.buf.as_ptr().add(st.read_offset_elems).cast::<u8>() }
    }

    let read_offset: usize = kani::any();
    kani::assume(read_offset <= 2);

    let dummy_ctx = 1u8;
    let mut api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    api.iter_next_chunk = Some(mock_next);
    api.iter_column_info = Some(mock_info);
    api.iter_column_mutable = Some(mock_col_mut);
    api.iter_column_readonly = Some(mock_col_ro);
    let host = make_valid_host(api, (&raw const dummy_ctx).cast::<BamHostCtx>());

    let mut state = HostileIterState {
        buf: [10, 20, 30, 40],
        read_offset_elems: read_offset,
        yielded: false,
    };
    // SAFETY: `state` stays pinned on the stack across `iter`.
    let mut iter = unsafe { ValidatedIter::from_raw((&raw mut state).cast::<BamIter>(), &host) };
    assert_eq!(iter.next_chunk(), Some(2));

    let mut dispenser = ChunkColumnDispenser::new(&mut iter);
    let write_slice = dispenser
        .next_write::<u32>(None)
        .expect("initial write column must succeed");
    let read_res = dispenser.next_read::<u32>(AccessMode::READ, None);

    if read_offset < 2 {
        // Ranges [0..2) and [read_offset..read_offset+2) overlap: MUST be rejected!
        assert_eq!(read_res, Err(FfiResult::ACCESS_CONFLICT));
        // Live mutable slice remains soundly writable with zero aliasing UB.
        write_slice[0] = 99;
        write_slice[1] = 100;
        assert_eq!(write_slice.len(), 2);
        assert_eq!(write_slice[0], 99);
        assert_eq!(write_slice[1], 100);
    } else {
        // read_offset == 2 => disjoint [0..2) and [2..4): BOTH succeed cleanly!
        let read_slice = read_res.expect("disjoint read column must succeed");
        write_slice[0] = 77;
        assert_eq!(read_slice.len(), 2);
        assert_eq!(read_slice[0], 30);
        assert_eq!(read_slice[1], 40);
    }
    kani::cover!(read_offset < 2);
    kani::cover!(read_offset == 2);
}

// ============================================================================
// PO-G5: `SparseRows` & `SparseRowsMut` Validation & Bounds
// ============================================================================

/// PO-G5: `SparseRows` and `SparseRowsMut` reject null/misaligned/overflowing outer arrays in
/// `O(1)` at construction time, reject null and misaligned non-ZST inner rows lazily in
/// `get`/`get_mut` without dereferencing, and remain panic-free over arbitrary `row: usize`.
#[kani::proof]
#[kani::unwind(6)]
fn po_g5_sparse_rows_and_mut_validation() {
    // 1. Null / misaligned / overflowing outer pointer checks for len > 0.
    let nz_len: usize = kani::any();
    kani::assume(nz_len > 0);
    // SAFETY: Null outer pointer is rejected before dereferencing.
    assert!(
        unsafe { SparseRows::<u64>::from_raw_parts(core::ptr::null(), nz_len, true) }.is_none()
    );
    // SAFETY: Null outer pointer is rejected before dereferencing.
    assert!(
        unsafe { SparseRowsMut::<u64>::from_raw_parts(core::ptr::null_mut(), nz_len, true) }
            .is_none()
    );

    let huge_len: usize = kani::any();
    kani::assume(
        huge_len
            .checked_mul(size_of::<*const u8>())
            .is_none_or(|b| b > isize::MAX as usize),
    );
    let dummy_outer = [core::ptr::null::<u8>(); 1];
    // SAFETY: Overflow check rejects `huge_len` before indexing `dummy_outer`.
    assert!(
        unsafe { SparseRows::<u64>::from_raw_parts(dummy_outer.as_ptr(), huge_len, true) }
            .is_none()
    );

    // 2. Bounded 3-row array with symbolic null/misaligned/valid entries and `allow_null_rows`.
    let mut vals: [u64; 3] = [101, 202, 303];
    let allow_null: bool = kani::any();
    let kind0: u8 = kani::any();
    let kind1: u8 = kani::any();
    let kind2: u8 = kani::any();
    kani::assume(kind0 <= 2 && kind1 <= 2 && kind2 <= 2);

    let make_ptr = |val: &mut u64, kind: u8| -> *mut u8 {
        match kind {
            0 => core::ptr::null_mut(),
            1 => core::ptr::from_mut(val).cast::<u8>().wrapping_add(1), // misaligned for u64
            _ => core::ptr::from_mut(val).cast::<u8>(),
        }
    };

    let (v0, rest) = vals.split_at_mut(1);
    let (v1, v2) = rest.split_at_mut(1);
    let p0 = make_ptr(&mut v0[0], kind0);
    let p1 = make_ptr(&mut v1[0], kind1);
    let p2 = make_ptr(&mut v2[0], kind2);
    let mut_ptrs: [*mut u8; 3] = [p0, p1, p2];
    let ro_ptrs: [*const u8; 3] = [p0.cast_const(), p1.cast_const(), p2.cast_const()];

    // SAFETY: `ro_ptrs` is readable for 3 pointers; non-null aligned entries point into `vals`.
    let rows = unsafe { SparseRows::<u64>::from_raw_parts(ro_ptrs.as_ptr(), 3, allow_null) }
        .expect("valid outer pointer array must construct SparseRows");
    let q: usize = kani::any();
    let got = rows.get(q);
    match q {
        0 => assert_eq!(got, (kind0 == 2).then_some(&101)),
        1 => assert_eq!(got, (kind1 == 2).then_some(&202)),
        2 => assert_eq!(got, (kind2 == 2).then_some(&303)),
        _ => {
            assert!(got.is_none());
        }
    }

    // SAFETY: `mut_ptrs` is readable for 3 pointers; non-null aligned entries point to distinct `vals` elements.
    let mut rows_mut =
        unsafe { SparseRowsMut::<u64>::from_raw_parts(mut_ptrs.as_ptr(), 3, allow_null) }
            .expect("valid outer pointer array must construct SparseRowsMut");
    let q_mut: usize = kani::any();
    let got_mut = rows_mut.get_mut(q_mut);
    match q_mut {
        0 if kind0 == 2 => {
            let slot = got_mut.expect("valid row 0");
            *slot = 777;
            assert_eq!(rows_mut.get(0), Some(&777));
        }
        1 if kind1 == 2 => {
            let slot = got_mut.expect("valid row 1");
            *slot = 777;
            assert_eq!(rows_mut.get(1), Some(&777));
        }
        2 if kind2 == 2 => {
            let slot = got_mut.expect("valid row 2");
            *slot = 777;
            assert_eq!(rows_mut.get(2), Some(&777));
        }
        _ => {
            assert!(got_mut.is_none());
        }
    }

    // 3. ZST `ZstPod` rows: non-null misaligned pointer is accepted (align == 1, size == 0).
    let zst_ptrs: [*const u8; 2] = [
        core::ptr::NonNull::<u8>::dangling().as_ptr(),
        core::ptr::null(),
    ];
    // SAFETY: `zst_ptrs` has 2 readable entries; ZST never dereferences row pointers.
    let zst_rows = unsafe { SparseRows::<ZstPod>::from_raw_parts(zst_ptrs.as_ptr(), 2, true) }
        .expect("optional ZST sparse rows must succeed");
    assert!(zst_rows.get(0).is_some());
    assert!(zst_rows.get(1).is_none());
    assert!(zst_rows.get(2).is_none());
    kani::cover!(got.is_some());
    kani::cover!(got.is_none());
}

// ============================================================================
// PO-G6: `ValidatedIter::mark_changed_range` & `ValidatedSystemInvocation`
// ============================================================================

/// PO-G6: `ValidatedIter::mark_changed_range` never overflows `start_row + row_count` and returns
/// `OUT_OF_RANGE` iff `start_row + row_count > current_len`.
#[kani::proof]
fn po_g6_iter_mark_changed_range_overflow_and_bounds() {
    struct LenIter {
        chunk_len: usize,
        marked: bool,
    }

    unsafe extern "C" fn mock_next(iter: *mut BamIter, out_len: *mut usize) -> FfiBool {
        // SAFETY: Test binds `iter` to `LenIter`.
        let st = unsafe { &mut *iter.cast::<LenIter>() };
        // SAFETY: Caller supplies writable `out_len`.
        unsafe { *out_len = st.chunk_len };
        FfiBool::TRUE
    }

    unsafe extern "C" fn mock_mark(iter: *mut BamIter, _: usize, _: usize, _: usize) -> FfiResult {
        // SAFETY: Test binds `iter` to `LenIter`.
        let st = unsafe { &mut *iter.cast::<LenIter>() };
        st.marked = true;
        FfiResult::OK
    }

    let dummy_ctx = 1u8;
    let mut api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    api.iter_next_chunk = Some(mock_next);
    api.iter_mark_changed_range = Some(mock_mark);
    let host = make_valid_host(api, (&raw const dummy_ctx).cast::<BamHostCtx>());

    let chunk_len: usize = kani::any();
    let start_row: usize = kani::any();
    let row_count: usize = kani::any();
    let mut state = LenIter {
        chunk_len,
        marked: false,
    };

    // SAFETY: `state` stays pinned on the stack across `iter`.
    let mut iter = unsafe { ValidatedIter::from_raw((&raw mut state).cast::<BamIter>(), &host) };
    assert_eq!(iter.next_chunk(), Some(chunk_len));

    let res = iter.mark_changed_range(0, start_row, row_count);
    let in_bounds = (start_row as u128) + (row_count as u128) <= (chunk_len as u128);
    if in_bounds {
        assert_eq!(res, FfiResult::OK);
        assert!(state.marked);
    } else {
        assert_eq!(res, FfiResult::OUT_OF_RANGE);
        assert!(!state.marked);
    }
    kani::cover!(res == FfiResult::OK);
    kani::cover!(res == FfiResult::OUT_OF_RANGE);
}

/// PO-G6: `ValidatedSystemInvocation` clamps arbitrary query counts to `MAX_QUERIES_PER_SYSTEM`,
/// enforces single-take exclusivity on `take_system_ref` and `take_raw_query_slot`, and enforces
/// disjointness in `queries_2_mut(a, b)` for all `(a, b) in usize^2`.
#[kani::proof]
fn po_g6_system_invocation_clamping_and_slot_exclusivity() {
    let dummy_ctx = 1u8;
    let api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    let host = make_valid_host(api, (&raw const dummy_ctx).cast::<BamHostCtx>());

    let total: usize = kani::any();
    let user: usize = kani::any();
    let res_idx: Option<usize> = kani::any();
    let mut dummy_sys_ctx = 1u8;

    // SAFETY: `dummy_sys_ctx` is a non-null stack token valid for the invocation.
    let mut inv = unsafe {
        ValidatedSystemInvocation::from_raw(
            (&raw mut dummy_sys_ctx).cast::<BamSystemCtx>(),
            &host,
            total,
            user,
            res_idx,
        )
    };

    let clamped_total = total.min(MAX_QUERIES_PER_SYSTEM);
    let clamped_user = user.min(clamped_total);
    assert_eq!(inv.query_count(), clamped_user);
    if let Some(ri) = inv.resource_query_index() {
        assert!(ri < clamped_total);
    }

    // `take_system_ref` succeeds strictly once.
    assert!(inv.take_system_ref().is_some());
    assert!(inv.take_system_ref().is_none());
    assert!(inv.system_ref().is_none());

    // `queries_2_mut(a, b)` over arbitrary `(a, b)` in usize^2.
    let a: usize = kani::any();
    let b: usize = kani::any();
    let pair = inv.queries_2_mut(a, b);
    let expect_pair = a != b && a < clamped_user && b < clamped_user;
    assert_eq!(pair.is_some(), expect_pair);

    // `take_raw_query_slot(idx)` rejects out-of-range and succeeds at most once per valid slot.
    let slot: usize = kani::any();
    let first = inv.take_raw_query_slot(slot);
    assert_eq!(first.is_some(), slot < clamped_total);
    let second = inv.take_raw_query_slot(slot);
    assert!(second.is_none());
    kani::cover!(expect_pair);
    kani::cover!(!expect_pair);
    kani::cover!(slot < clamped_total);
    kani::cover!(slot >= clamped_total);
}

// ============================================================================
// PO-G7: Trampolines & `ValidatedCommands` Null / Bounds / Slot Guards
// ============================================================================

/// PO-G7: `pod_clone_trampoline`, `generic_drop_trampoline`, and `init_default_trampoline` guard
/// against null and misaligned pointers without dereferencing, handle ZSTs safely, and transfer
/// exact values on valid aligned buffers.
#[kani::proof]
fn po_g7_trampolines_null_alignment_and_zst_guards() {
    // 1. `pod_clone_trampoline` (non-ZST u64 and ZST ())
    let src: u64 = kani::any();
    let mut dst: u64 = 0;
    // SAFETY: Null pointer arguments are rejected with NULL_POINTER before dereferencing.
    assert_eq!(
        unsafe {
            pod_clone_trampoline::<u64>(
                core::ptr::null(),
                (&raw mut dst).cast::<c_void>(),
                core::ptr::null_mut(),
            )
        },
        FfiResult::NULL_POINTER
    );
    // SAFETY: Null pointer arguments are rejected with NULL_POINTER before dereferencing.
    assert_eq!(
        unsafe {
            pod_clone_trampoline::<u64>(
                (&raw const src).cast::<c_void>(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            )
        },
        FfiResult::NULL_POINTER
    );
    // SAFETY: `src` and `dst` are distinct, initialized/writable 8-byte stack variables.
    assert_eq!(
        unsafe {
            pod_clone_trampoline::<u64>(
                (&raw const src).cast::<c_void>(),
                (&raw mut dst).cast::<c_void>(),
                core::ptr::null_mut(),
            )
        },
        FfiResult::OK
    );
    assert_eq!(dst, src);
    // SAFETY: ZST `ZstPod` clone succeeds even with null pointers without dereferencing.
    assert_eq!(
        unsafe {
            pod_clone_trampoline::<ZstPod>(
                core::ptr::null(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            )
        },
        FfiResult::OK
    );
    // Overlapping and identical `(src, dst)` offsets inside a shared buffer are sound (`ptr::copy`).
    let orig: [u8; 16] = kani::any();
    let mut shared = orig;
    let src_off: usize = kani::any();
    let dst_off: usize = kani::any();
    kani::assume(src_off <= 8 && dst_off <= 8);
    // SAFETY: Both `src_off..src_off+8` and `dst_off..dst_off+8` lie within `shared[0..16]`.
    assert_eq!(
        unsafe {
            pod_clone_trampoline::<u64>(
                shared.as_ptr().add(src_off).cast::<c_void>(),
                shared.as_mut_ptr().add(dst_off).cast::<c_void>(),
                core::ptr::null_mut(),
            )
        },
        FfiResult::OK
    );
    let check_b: usize = kani::any();
    kani::assume(check_b < 8);
    assert_eq!(shared[dst_off + check_b], orig[src_off + check_b]);

    // 2. `generic_drop_trampoline` (null and arbitrary misalignment 1..8 are safe no-ops)
    // SAFETY: Null pointer is explicitly handled as a no-op.
    unsafe { generic_drop_trampoline::<u64>(core::ptr::null_mut()) };
    let mut raw_buf = [0u64; 2];
    let misalign: usize = kani::any();
    kani::assume(misalign >= 1 && misalign < align_of::<u64>());
    let bad_drop_ptr = core::ptr::from_mut(&mut raw_buf)
        .cast::<u8>()
        .wrapping_add(misalign)
        .cast::<c_void>();
    // SAFETY: Alignment guard returns early without dereferencing `bad_drop_ptr`.
    unsafe { generic_drop_trampoline::<u64>(bad_drop_ptr) };

    // 3. `init_default_trampoline` (null, misalignment, valid write, and ZST)
    // SAFETY: Null destination is rejected with NULL_POINTER.
    assert_eq!(
        unsafe { init_default_trampoline::<u64>(core::ptr::null_mut()) },
        FfiResult::NULL_POINTER
    );
    // SAFETY: Misaligned destination is rejected with INVALID_ARGUMENT before writing.
    assert_eq!(
        unsafe { init_default_trampoline::<u64>(bad_drop_ptr) },
        FfiResult::INVALID_ARGUMENT
    );
    let mut uninit_u64 = core::mem::MaybeUninit::<u64>::uninit();
    // SAFETY: `uninit_u64` is 8-byte aligned and writable for `size_of::<u64>()` bytes.
    assert_eq!(
        unsafe { init_default_trampoline::<u64>(uninit_u64.as_mut_ptr().cast::<c_void>()) },
        FfiResult::OK
    );
    // SAFETY: `init_default_trampoline` returned OK after initializing `uninit_u64`.
    assert_eq!(unsafe { uninit_u64.assume_init() }, 0u64);
    // SAFETY: ZST `()` default initialization succeeds without touching `dst`.
    assert_eq!(
        unsafe { init_default_trampoline::<()>(core::ptr::null_mut()) },
        FfiResult::OK
    );

    // 4. `row_migrate_trampoline` (null/misaligned user_data, src_row, dst_row, valid, and ZST)
    type MigFn = fn(&u32) -> Result<u64, FfiResult>;
    fn mig_u32_to_u64(old: &u32) -> Result<u64, FfiResult> {
        Ok(u64::from(*old).wrapping_add(7))
    }
    let mut mig_ctx = RowMigrateCtx::<MigFn> {
        func: mig_u32_to_u64,
    };
    let old_val: u32 = kani::any();
    let mut new_slot = core::mem::MaybeUninit::<u64>::uninit();
    let ud_ptr = core::ptr::from_mut(&mut mig_ctx).cast::<c_void>();
    // SAFETY: Null user_data is rejected before dereferencing.
    assert_eq!(
        unsafe {
            row_migrate_trampoline::<u32, u64, MigFn>(
                core::ptr::from_ref(&old_val).cast(),
                new_slot.as_mut_ptr().cast(),
                core::ptr::null_mut(),
            )
        },
        FfiResult::NULL_POINTER
    );
    // SAFETY: Misaligned user_data is rejected before dereferencing.
    assert_eq!(
        unsafe {
            row_migrate_trampoline::<u32, u64, MigFn>(
                core::ptr::from_ref(&old_val).cast(),
                new_slot.as_mut_ptr().cast(),
                bad_drop_ptr.cast(),
            )
        },
        FfiResult::INVALID_ARGUMENT
    );
    // SAFETY: Null src_row / dst_row rejected with NULL_POINTER.
    assert_eq!(
        unsafe {
            row_migrate_trampoline::<u32, u64, MigFn>(
                core::ptr::null(),
                new_slot.as_mut_ptr().cast(),
                ud_ptr,
            )
        },
        FfiResult::NULL_POINTER
    );
    // SAFETY: Null dst_row rejected with NULL_POINTER.
    assert_eq!(
        unsafe {
            row_migrate_trampoline::<u32, u64, MigFn>(
                core::ptr::from_ref(&old_val).cast(),
                core::ptr::null_mut(),
                ud_ptr,
            )
        },
        FfiResult::NULL_POINTER
    );
    // SAFETY: Misaligned dst_row rejected with INVALID_ARGUMENT before writing.
    assert_eq!(
        unsafe {
            row_migrate_trampoline::<u32, u64, MigFn>(
                core::ptr::from_ref(&old_val).cast(),
                bad_drop_ptr,
                ud_ptr,
            )
        },
        FfiResult::INVALID_ARGUMENT
    );
    // SAFETY: Valid aligned `old_val`, `new_slot`, and `mig_ctx`.
    assert_eq!(
        unsafe {
            row_migrate_trampoline::<u32, u64, MigFn>(
                core::ptr::from_ref(&old_val).cast(),
                new_slot.as_mut_ptr().cast(),
                ud_ptr,
            )
        },
        FfiResult::OK
    );
    // SAFETY: `row_migrate_trampoline` returned OK after initializing `new_slot`.
    assert_eq!(
        unsafe { new_slot.assume_init() },
        u64::from(old_val).wrapping_add(7)
    );
    type ZstMigFn = fn(&ZstPod) -> Result<ZstPod, FfiResult>;
    fn mig_zst(_: &ZstPod) -> Result<ZstPod, FfiResult> {
        Ok([])
    }
    let mut zst_mig_ctx = RowMigrateCtx::<ZstMigFn> { func: mig_zst };
    let zst_mig_res = unsafe {
        row_migrate_trampoline::<ZstPod, ZstPod, ZstMigFn>(
            core::ptr::null(),
            core::ptr::null_mut(),
            core::ptr::from_mut(&mut zst_mig_ctx).cast(),
        )
    };
    assert_eq!(zst_mig_res, FfiResult::OK);
    kani::cover!(dst == src);
    kani::cover!(zst_mig_res == FfiResult::OK);
}

/// PO-G7: `ValidatedCommands` rejects null command buffers, null entities, invalid components,
/// non-empty null payloads, mismatched slice lengths, and uncovered ABI 0.3 slots cleanly.
#[kani::proof]
fn po_g7_validated_commands_null_entity_component_and_slot_guards() {
    // Minimal v0.2 host (no extended / ABI 0.3 command slots covered).
    let dummy_ctx = 1u8;
    let v02_api = make_core_api(BamHostApi::MIN_V0_2_SIZE as u32, ABI_VERSION);
    let v02_host = make_valid_host(v02_api, (&raw const dummy_ctx).cast::<BamHostCtx>());

    // 1. Empty/null `ValidatedCommands` rejects every operation with NULL_POINTER.
    let mut empty_cmds = ValidatedCommands::empty(&v02_host);
    assert!(!empty_cmds.is_valid());
    assert_eq!(empty_cmds.spawn_empty(), Err(FfiResult::NULL_POINTER));
    assert_eq!(
        empty_cmds.despawn(FfiEntity::from_bits(1)),
        FfiResult::NULL_POINTER
    );
    assert_eq!(
        empty_cmds.insert(FfiEntity::from_bits(1), BamComponent::from_raw(1), &42u32),
        FfiResult::NULL_POINTER
    );
    assert_eq!(
        empty_cmds.insert_default(FfiEntity::from_bits(1), BamComponent::from_raw(1)),
        FfiResult::NULL_POINTER
    );
    assert_eq!(
        empty_cmds.remove(FfiEntity::from_bits(1), BamComponent::from_raw(1)),
        FfiResult::NULL_POINTER
    );
    assert_eq!(empty_cmds.checkpoint(), Err(FfiResult::NULL_POINTER));
    assert_eq!(empty_cmds.rollback(0), FfiResult::NULL_POINTER);
    assert_eq!(
        empty_cmds.insert_relation(
            FfiEntity::from_bits(1),
            BamComponent::from_raw(1),
            FfiEntity::from_bits(2)
        ),
        FfiResult::NULL_POINTER
    );
    assert_eq!(
        empty_cmds.remove_relation(
            FfiEntity::from_bits(1),
            BamComponent::from_raw(1),
            FfiEntity::from_bits(2)
        ),
        FfiResult::NULL_POINTER
    );

    // 2. Non-null `ValidatedCommands` on v0.2 host enforces argument and slot-coverage guards.
    let mut dummy_cmds = 1u8;
    // SAFETY: `dummy_cmds` is a non-null stack token valid for `cmds`.
    let mut cmds = unsafe {
        ValidatedCommands::from_raw((&raw mut dummy_cmds).cast::<BamCommands>(), &v02_host)
    };

    let ent = FfiEntity(kani::any());
    let comp = BamComponent(kani::any());
    let data_len: usize = kani::any();
    let data_non_null: bool = kani::any();
    let byte_val: u8 = 0;
    let data_ptr = if data_non_null {
        &raw const byte_val
    } else {
        core::ptr::null()
    };

    // SAFETY: `stub_cmd_insert` does not dereference `data_ptr`.
    let ins_res = unsafe { cmds.insert_raw(ent, comp, data_ptr, data_len) };
    if ent.is_null() {
        assert_eq!(ins_res, FfiResult::INVALID_ENTITY);
    } else if !comp.is_valid() {
        assert_eq!(ins_res, FfiResult::INVALID_COMPONENT);
    } else if data_len > 0 && !data_non_null {
        assert_eq!(ins_res, FfiResult::NULL_POINTER);
    } else {
        assert_eq!(ins_res, FfiResult::OK);
    }
    kani::cover!(ins_res == FfiResult::OK);
    kani::cover!(ins_res == FfiResult::INVALID_ENTITY);
    kani::cover!(ins_res == FfiResult::INVALID_COMPONENT);
    kani::cover!(ins_res == FfiResult::NULL_POINTER);

    // Mismatched parallel slice lengths in `spawn_raw` -> INVALID_ARGUMENT.
    let comps = [BamComponent::from_raw(1), BamComponent::from_raw(2)];
    let ptrs = [core::ptr::null::<u8>()];
    // SAFETY: Length mismatch is rejected before calling host.
    assert_eq!(
        unsafe { cmds.spawn_raw(&comps, &ptrs) },
        Err(FfiResult::INVALID_ARGUMENT)
    );

    // Uncovered ABI 0.3 slots on v0.2 host return UNSUPPORTED (never read past MIN_V0_2_SIZE).
    assert_eq!(cmds.checkpoint(), Err(FfiResult::UNSUPPORTED));
    assert_eq!(cmds.rollback(0), FfiResult::UNSUPPORTED);
    assert_eq!(
        cmds.insert_relation(
            FfiEntity::from_bits(1),
            BamComponent::from_raw(1),
            FfiEntity::from_bits(2)
        ),
        FfiResult::UNSUPPORTED
    );
    assert_eq!(
        cmds.remove_relation(
            FfiEntity::from_bits(1),
            BamComponent::from_raw(1),
            FfiEntity::from_bits(2)
        ),
        FfiResult::UNSUPPORTED
    );
}

/// PO-G7 (state & observer trampolines): `guest_restore_state_trampoline` rejects null+nonzero
/// buffers, `len > isize::MAX` (`OUT_OF_RANGE`), and misaligned `instance` pointers;
/// `guest_save_state_trampoline` rejects null `writer` and misaligned `instance`; and
/// `observer_mut_closure_trampoline` / `observer_closure_trampoline` reject null/misaligned
/// `user_data` and `event_data` and reject re-entrant entry BEFORE forming an aliasing `&mut E`
/// or touching `*propagate`.
#[kani::proof]
fn po_g7_guest_state_and_observer_trampolines() {
    unsafe extern "C" fn stub_writer(_: *mut c_void, _: *const u8, _: usize) -> FfiResult {
        FfiResult::OK
    }

    // 1. `guest_restore_state_trampoline` bounds & alignment guards
    let dummy_byte: u8 = 0x42;
    let nz_len: usize = kani::any();
    kani::assume(nz_len > 0);
    // SAFETY: Null bytes with non-zero length is rejected before dereferencing.
    assert_eq!(
        unsafe { guest_restore_state_trampoline(core::ptr::null_mut(), core::ptr::null(), nz_len) },
        FfiResult::NULL_POINTER
    );
    let huge_len: usize = kani::any();
    kani::assume(huge_len > isize::MAX as usize);
    // SAFETY: `len > isize::MAX` is rejected with OUT_OF_RANGE before forming any slice.
    assert_eq!(
        unsafe {
            guest_restore_state_trampoline(core::ptr::null_mut(), &raw const dummy_byte, huge_len)
        },
        FfiResult::OUT_OF_RANGE
    );
    let ok_len: usize = kani::any();
    kani::assume(ok_len <= isize::MAX as usize);
    // SAFETY: Null instance with valid length is a no-op returning OK.
    assert_eq!(
        unsafe {
            guest_restore_state_trampoline(core::ptr::null_mut(), &raw const dummy_byte, ok_len)
        },
        FfiResult::OK
    );

    let mut inst_buf = [0u64; 2];
    let inst_misalign: usize = kani::any();
    kani::assume(inst_misalign >= 1 && inst_misalign < align_of::<GuestInstance>());
    let bad_inst = core::ptr::from_mut(&mut inst_buf)
        .cast::<u8>()
        .wrapping_add(inst_misalign)
        .cast::<BamInstance>();
    // SAFETY: Alignment guard rejects misaligned `instance` before dereferencing.
    assert_eq!(
        unsafe { guest_restore_state_trampoline(bad_inst, &raw const dummy_byte, 0) },
        FfiResult::INVALID_ARGUMENT
    );
    // SAFETY: Null writer returns NULL_POINTER; null instance returns OK; misaligned instance returns INVALID_ARGUMENT.
    assert_eq!(
        unsafe { guest_save_state_trampoline(bad_inst, None, core::ptr::null_mut()) },
        FfiResult::NULL_POINTER
    );
    assert_eq!(
        unsafe {
            guest_save_state_trampoline(
                core::ptr::null_mut(),
                Some(stub_writer),
                core::ptr::null_mut(),
            )
        },
        FfiResult::OK
    );
    assert_eq!(
        unsafe { guest_save_state_trampoline(bad_inst, Some(stub_writer), core::ptr::null_mut()) },
        FfiResult::INVALID_ARGUMENT
    );

    // 2. `observer_closure_trampoline` & `observer_mut_closure_trampoline` guards + re-entrancy
    let dummy_ctx = 1u8;
    let api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    let host = make_valid_host(api, (&raw const dummy_ctx).cast::<BamHostCtx>());

    let mut ev_val: u64 = kani::any();
    let orig_ev = ev_val;
    let init_prop: bool = kani::any();
    let mut prop_flag = FfiBool::from_bool(init_prop);

    type MutObsFn = fn(FfiEntity, &mut u64, &mut bool, &mut ValidatedCommands<'_>) -> FfiResult;
    fn mut_obs_inc(
        target: FfiEntity,
        ev: &mut u64,
        prop: &mut bool,
        cmds: &mut ValidatedCommands<'_>,
    ) -> FfiResult {
        let _ = (target, cmds);
        *ev = ev.wrapping_add(1);
        *prop = !*prop;
        FfiResult::OK
    }
    fn mut_obs_noop(
        _: FfiEntity,
        _: &mut u64,
        _: &mut bool,
        _: &mut ValidatedCommands<'_>,
    ) -> FfiResult {
        FfiResult::OK
    }
    type ZstObsFn = fn(FfiEntity, &ZstPod, &mut ValidatedCommands<'_>) -> FfiResult;
    fn zst_obs_noop(_: FfiEntity, _: &ZstPod, _: &mut ValidatedCommands<'_>) -> FfiResult {
        FfiResult::OK
    }

    let mut obs_state = ObserverTrampolineState::<MutObsFn> {
        host: PinnedHostPtr::new(&raw const host),
        callback: ReentrantCell::new(mut_obs_inc),
    };
    let ud = core::ptr::from_mut(&mut obs_state).cast::<c_void>();

    // Null & misaligned user_data / event_data checks on `observer_mut_closure_trampoline`.
    // SAFETY: Null/alignment guards reject before dereferencing.
    assert_eq!(
        unsafe {
            observer_mut_closure_trampoline::<u64, MutObsFn>(
                FfiEntity::from_bits(1),
                (&raw mut ev_val).cast(),
                &raw mut prop_flag,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            )
        },
        FfiResult::NULL_POINTER
    );
    let mut unaligned_buf = [0u64; 2];
    let bad_ev = core::ptr::from_mut(&mut unaligned_buf)
        .cast::<u8>()
        .wrapping_add(1);
    // SAFETY: Misaligned `user_data` rejected with INVALID_ARGUMENT before dereferencing.
    assert_eq!(
        unsafe {
            observer_mut_closure_trampoline::<u64, MutObsFn>(
                FfiEntity::from_bits(1),
                (&raw mut ev_val).cast(),
                &raw mut prop_flag,
                core::ptr::null_mut(),
                bad_ev.cast(),
            )
        },
        FfiResult::INVALID_ARGUMENT
    );
    // SAFETY: Misaligned `event_data` rejected with INVALID_ARGUMENT before forming `&mut u64`.
    assert_eq!(
        unsafe {
            observer_mut_closure_trampoline::<u64, MutObsFn>(
                FfiEntity::from_bits(1),
                bad_ev,
                &raw mut prop_flag,
                core::ptr::null_mut(),
                ud,
            )
        },
        FfiResult::INVALID_ARGUMENT
    );
    // SAFETY: Null `event_data` for non-ZST rejected with INVALID_ARGUMENT.
    assert_eq!(
        unsafe {
            observer_mut_closure_trampoline::<u64, MutObsFn>(
                FfiEntity::from_bits(1),
                core::ptr::null_mut(),
                &raw mut prop_flag,
                core::ptr::null_mut(),
                ud,
            )
        },
        FfiResult::INVALID_ARGUMENT
    );
    // SAFETY: Valid aligned invocation updates `ev_val` and `prop_flag`.
    assert_eq!(
        unsafe {
            observer_mut_closure_trampoline::<u64, MutObsFn>(
                FfiEntity::from_bits(1),
                (&raw mut ev_val).cast(),
                &raw mut prop_flag,
                core::ptr::null_mut(),
                ud,
            )
        },
        FfiResult::OK
    );
    assert_eq!(ev_val, orig_ev.wrapping_add(1));
    assert_eq!(prop_flag.get(), !init_prop);

    // Explicit re-entrancy rejection test (without pointer-to-int cast to keep strict provenance).
    let mut reent_ev: u64 = kani::any();
    let orig_reent_ev = reent_ev;
    let mut reent_prop = FfiBool::from_bool(init_prop);
    let mut reent_state = ObserverTrampolineState::<MutObsFn> {
        host: PinnedHostPtr::new(&raw const host),
        callback: ReentrantCell::new(mut_obs_noop),
    };
    let reent_ud = core::ptr::from_mut(&mut reent_state).cast::<c_void>();
    // Hold `in_use` via outer `try_with_mut` while holding `&mut reent_ev` and calling the trampoline!
    let outer_done = reent_state.callback.try_with_mut(|_| {
        let outer_ref: &mut u64 = &mut reent_ev;
        *outer_ref = outer_ref.wrapping_add(3);
        // SAFETY: `reent_ud` points to `reent_state` whose `ReentrantCell` is locked; the trampoline
        // MUST return `ACCESS_CONFLICT` before forming `&mut u64` or touching `reent_prop`!
        let rc = unsafe {
            observer_mut_closure_trampoline::<u64, MutObsFn>(
                FfiEntity::from_bits(1),
                core::ptr::from_mut(outer_ref).cast::<u8>(),
                &raw mut reent_prop,
                core::ptr::null_mut(),
                reent_ud,
            )
        };
        assert_eq!(rc, FfiResult::ACCESS_CONFLICT);
        *outer_ref = outer_ref.wrapping_add(4);
    });
    assert!(outer_done.is_some());
    assert_eq!(reent_ev, orig_reent_ev.wrapping_add(7));
    assert_eq!(reent_prop.get(), init_prop);

    // ZST `observer_closure_trampoline` accepts null `event_data` cleanly.
    let mut zst_obs = ObserverTrampolineState::<ZstObsFn> {
        host: PinnedHostPtr::new(&raw const host),
        callback: ReentrantCell::new(zst_obs_noop),
    };
    let zst_obs_res = unsafe {
        observer_closure_trampoline::<ZstPod, ZstObsFn>(
            FfiEntity::from_bits(1),
            core::ptr::null(),
            core::ptr::null_mut(),
            core::ptr::from_mut(&mut zst_obs).cast(),
        )
    };
    assert_eq!(zst_obs_res, FfiResult::OK);
    kani::cover!(zst_obs_res == FfiResult::OK);
    kani::cover!(init_prop);
    kani::cover!(!init_prop);
}

/// PO-G4 (entities & `ValidatedIter` split-borrow overlap guards): Verifies
/// `ChunkColumnDispenser::entities()` repeat-shift rejection and overflow rejection, plus
/// `ValidatedIter::read_write_checked` and `write_2_checked` byte-overlap rejection across
/// symbolic offsets `0..=2`.
#[kani::proof]
#[kani::unwind(6)]
fn po_g4_dispenser_entities_and_iter_overlap_guards() {
    #[repr(C)]
    struct SplitState {
        buf: [u32; 4],
        ent_a: [FfiEntity; 2],
        ent_b: [FfiEntity; 2],
        second_offset: usize,
        shift_entities: bool,
        ent_calls: usize,
        chunk_len: usize,
    }

    unsafe extern "C" fn mock_next(iter: *mut BamIter, out_len: *mut usize) -> FfiBool {
        // SAFETY: Test binds `iter` to `SplitState`.
        let st = unsafe { &*iter.cast::<SplitState>() };
        // SAFETY: Caller supplies writable `out_len`.
        unsafe { *out_len = st.chunk_len };
        FfiBool::TRUE
    }
    unsafe extern "C" fn mock_entities(iter: *const BamIter) -> *const FfiEntity {
        // SAFETY: Test binds `iter` to `SplitState`.
        let st = unsafe { &mut *iter.cast_mut().cast::<SplitState>() };
        st.ent_calls += 1;
        if st.ent_calls == 1 || !st.shift_entities {
            st.ent_a.as_ptr()
        } else {
            st.ent_b.as_ptr()
        }
    }
    unsafe extern "C" fn mock_info(
        _: *const BamIter,
        term: usize,
        out: *mut BamColumnInfo,
    ) -> FfiResult {
        let info = match term {
            0 => BamColumnInfo {
                component: BamComponent::from_raw(1),
                mode: AccessMode::WRITE,
                uuid: ComponentUuid::from_bytes([1; 16]),
                size: size_of::<u32>(),
                align: align_of::<u32>(),
                storage_flags: 0,
                reserved0: 0,
            },
            1 => BamColumnInfo {
                component: BamComponent::from_raw(2),
                mode: AccessMode::WRITE,
                uuid: ComponentUuid::from_bytes([2; 16]),
                size: size_of::<u32>(),
                align: align_of::<u32>(),
                storage_flags: 0,
                reserved0: 0,
            },
            _ => return FfiResult::OUT_OF_RANGE,
        };
        // SAFETY: Caller supplies writable `out`.
        unsafe { *out = info };
        FfiResult::OK
    }
    unsafe extern "C" fn mock_ro(iter: *const BamIter, term: usize) -> *const u8 {
        // SAFETY: Test binds `iter` to `SplitState`.
        let st = unsafe { &*iter.cast::<SplitState>() };
        if term == 0 {
            st.buf.as_ptr().cast::<u8>()
        } else {
            // SAFETY: `second_offset <= 2` stays within `buf[0..4]`.
            unsafe { st.buf.as_ptr().add(st.second_offset).cast::<u8>() }
        }
    }
    unsafe extern "C" fn mock_mut(iter: *mut BamIter, term: usize) -> *mut u8 {
        // SAFETY: Test binds `iter` to `SplitState`.
        let st = unsafe { &mut *iter.cast::<SplitState>() };
        if term == 0 {
            st.buf.as_mut_ptr().cast::<u8>()
        } else {
            // SAFETY: `second_offset <= 2` stays within `buf[0..4]`.
            unsafe { st.buf.as_mut_ptr().add(st.second_offset).cast::<u8>() }
        }
    }

    let dummy_ctx = 1u8;
    let mut api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    api.iter_next_chunk = Some(mock_next);
    api.iter_entities = Some(mock_entities);
    api.iter_column_info = Some(mock_info);
    api.iter_column_readonly = Some(mock_ro);
    api.iter_column_mutable = Some(mock_mut);
    let host = make_valid_host(api, (&raw const dummy_ctx).cast::<BamHostCtx>());

    let off: usize = kani::any();
    kani::assume(off <= 2);
    let shift: bool = kani::any();
    let mut state = SplitState {
        buf: [1, 2, 3, 4],
        ent_a: [FfiEntity::from_bits(1), FfiEntity::from_bits(2)],
        ent_b: [FfiEntity::from_bits(3), FfiEntity::from_bits(4)],
        second_offset: off,
        shift_entities: shift,
        ent_calls: 0,
        chunk_len: 2,
    };

    // SAFETY: `state` stays pinned on the stack across `iter`.
    let mut iter = unsafe { ValidatedIter::from_raw((&raw mut state).cast::<BamIter>(), &host) };
    assert_eq!(iter.next_chunk(), Some(2));

    // 1. `read_write_checked` & `write_2_checked` reject overlapping offsets [0, 1] and succeed on 2.
    assert!(
        iter.read_write_checked::<u32, u32>(0, None, 0, None)
            .is_none()
    );
    assert!(iter.write_2_checked::<u32, u32>(0, None, 0, None).is_none());
    let rw = iter.read_write_checked::<u32, u32>(1, None, 0, None);
    assert_eq!(rw.is_some(), off == 2);
    let w2 = iter.write_2_checked::<u32, u32>(0, None, 1, None);
    assert_eq!(w2.is_some(), off == 2);

    // 2. `ChunkColumnDispenser::entities()` accepts repeat identical range and rejects shifted range.
    let mut disp = ChunkColumnDispenser::new(&mut iter);
    assert!(disp.entities().is_ok());
    let repeat = disp.entities();
    if shift {
        assert_eq!(repeat, Err(FfiResult::ACCESS_CONFLICT));
    } else {
        assert!(repeat.is_ok());
    }
    kani::cover!(off == 2);
    kani::cover!(off < 2);
    kani::cover!(repeat.is_ok());
    kani::cover!(repeat == Err(FfiResult::ACCESS_CONFLICT));
}

/// PO-G1 / PO-G6 (mapped VTables & bridged row bounds): Verifies `Host::resolve_mapped_wire_vtable`
/// and `resolve_mapped_string_vtable` prefix/null-getter guards, and proves `ValidatedIter` bridged
/// row accessors reject non-power-of-two `host_align` (`TYPE_MISMATCH`) and `current_len *
/// host_size > isize::MAX` (`OUT_OF_RANGE`) without out-of-bounds pointer arithmetic.
#[kani::proof]
fn po_g1_mapped_vtables_and_bridged_row_bounds() {
    unsafe extern "C" fn stub_wire_get(_: *const c_void, out: *mut u64) -> FfiResult {
        // SAFETY: Caller supplies writable `out`.
        unsafe { *out = 42 };
        FfiResult::OK
    }
    unsafe extern "C" fn stub_str_get(_: *const c_void) -> StrView {
        StrView::empty()
    }

    #[repr(C)]
    struct VtCtx {
        wire_vt: MappedWireVTable<u64>,
        str_vt: MappedStringVTable,
        return_str: bool,
    }

    unsafe extern "C" fn mock_get_proc(ctx: *const BamHostCtx, _: *const c_char) -> *const c_void {
        // SAFETY: Test binds `ctx` to `VtCtx`.
        let st = unsafe { &*ctx.cast::<VtCtx>() };
        if st.return_str {
            core::ptr::from_ref(&st.str_vt).cast::<c_void>()
        } else {
            core::ptr::from_ref(&st.wire_vt).cast::<c_void>()
        }
    }

    let uuid = ComponentUuid::from_bytes([0xAB; 16]);
    let has_get: bool = kani::any();
    // Constrain to interesting layout cases: valid (8, 8), invalid align (0, 3), or huge size.
    let layout_case: u8 = kani::any();
    kani::assume(layout_case < 4);
    let (h_size, h_align) = match layout_case {
        0 => (8usize, 8usize),
        1 => (8usize, 0usize),
        2 => (8usize, 3usize),
        _ => ((isize::MAX as usize) & !7usize, 8usize),
    };

    let mut vt_ctx = VtCtx {
        wire_vt: MappedWireVTable {
            header: VTableHeader::from_raw_parts(
                size_of::<MappedWireVTable<u64>>() as u32,
                uuid,
                h_size,
                h_align,
            ),
            get: has_get.then_some(stub_wire_get),
            set: None,
        },
        str_vt: MappedStringVTable {
            header: VTableHeader::from_raw_parts(
                size_of::<MappedStringVTable>() as u32,
                uuid,
                8,
                8,
            ),
            get: has_get.then_some(stub_str_get),
            set: None,
        },
        return_str: false,
    };

    #[repr(C)]
    struct BridgedIter {
        col_bytes: [u64; 2],
        h_size: usize,
        h_align: usize,
        uuid: ComponentUuid,
    }
    unsafe extern "C" fn b_next(_: *mut BamIter, out_len: *mut usize) -> FfiBool {
        // SAFETY: Caller supplies writable `out_len`.
        unsafe { *out_len = 2 };
        FfiBool::TRUE
    }
    unsafe extern "C" fn b_info(
        iter: *const BamIter,
        _: usize,
        out: *mut BamColumnInfo,
    ) -> FfiResult {
        // SAFETY: Test binds `iter` to `BridgedIter`.
        let st = unsafe { &*iter.cast::<BridgedIter>() };
        // SAFETY: Caller supplies writable `out`.
        unsafe {
            *out = BamColumnInfo {
                component: BamComponent::from_raw(1),
                mode: AccessMode::READ,
                uuid: st.uuid,
                size: st.h_size,
                align: st.h_align,
                storage_flags: 0,
                reserved0: 0,
            };
        }
        FfiResult::OK
    }
    unsafe extern "C" fn b_col_ro(iter: *const BamIter, _: usize) -> *const u8 {
        // SAFETY: Test binds `iter` to `BridgedIter`.
        let st = unsafe { &*iter.cast::<BridgedIter>() };
        st.col_bytes.as_ptr().cast::<u8>()
    }

    let mut api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    api.get_proc_address = Some(mock_get_proc);
    api.iter_next_chunk = Some(b_next);
    api.iter_column_info = Some(b_info);
    api.iter_column_readonly = Some(b_col_ro);
    let host = make_valid_host(api, (&raw const vt_ctx).cast::<BamHostCtx>());

    // Non-`bam.v0.` symbol prefix is always rejected with INVALID_ARGUMENT.
    // SAFETY: Both byte literals are NUL-terminated with no interior NUL bytes.
    let bad_sym = unsafe { CStr::from_bytes_with_nul_unchecked(b"evil.vtable\0") };
    // SAFETY: Byte literal is NUL-terminated with no interior NUL bytes.
    let good_sym = unsafe { CStr::from_bytes_with_nul_unchecked(b"bam.v0.test.vtable\0") };
    assert!(matches!(
        host.resolve_mapped_wire_vtable::<u64>(bad_sym, uuid),
        Err(FfiResult::INVALID_ARGUMENT)
    ));
    assert!(matches!(
        host.resolve_mapped_string_vtable(bad_sym, uuid),
        Err(FfiResult::INVALID_ARGUMENT)
    ));

    let wire_res = host.resolve_mapped_wire_vtable::<u64>(good_sym, uuid);
    kani::cover!(wire_res.is_ok());
    kani::cover!(wire_res.is_err());
    if !has_get {
        assert!(matches!(wire_res, Err(FfiResult::INVALID_ARGUMENT)));
        return;
    }
    let wire_vt = wire_res.expect("valid wire vtable must resolve");

    let mut b_state = BridgedIter {
        col_bytes: [10, 20],
        h_size,
        h_align,
        uuid,
    };
    // SAFETY: `b_state` stays pinned on the stack across `iter`.
    let mut iter = unsafe { ValidatedIter::from_raw((&raw mut b_state).cast::<BamIter>(), &host) };
    assert_eq!(iter.next_chunk(), Some(2));
    let read_res = iter.read_mapped_wire_row(0, 1, &wire_vt);
    match layout_case {
        0 => assert_eq!(read_res, Ok(Some(42))),
        1 | 2 => assert_eq!(read_res, Err(FfiResult::TYPE_MISMATCH)),
        _ => assert_eq!(read_res, Err(FfiResult::OUT_OF_RANGE)),
    }
    kani::cover!(read_res.is_ok());
    kani::cover!(matches!(read_res, Err(FfiResult::TYPE_MISMATCH)));
    kani::cover!(matches!(read_res, Err(FfiResult::OUT_OF_RANGE)));

    // String VTable resolution check.
    vt_ctx.return_str = true;
    let host_str = make_valid_host(api, (&raw const vt_ctx).cast::<BamHostCtx>());
    assert!(
        host_str
            .resolve_mapped_string_vtable(good_sym, uuid)
            .is_ok()
    );
}

// ============================================================================
// PO-G8: `extern "C"` Trampoline ABI Guards
// ============================================================================

/// PO-G8 (`system_closure_trampoline` & `run_if_closure_trampoline`): Proves null/misaligned
/// `sys_ctx`/`user_data` rejection, `sys_host_ctx` context refresh, re-entrancy rejection
/// (`ACCESS_CONFLICT` / `FfiBool::FALSE`), and panic-freedom across all branches.
#[kani::proof]
fn po_g8_system_and_run_if_trampolines() {
    #[repr(C)]
    struct SysHostProbe {
        refreshed_ctx: u64,
        return_refreshed: bool,
    }

    unsafe extern "C" fn probe_sys_host_ctx(sys_ctx: *const BamSystemCtx) -> *const BamHostCtx {
        // SAFETY: Test binds `sys_ctx` to `SysHostProbe`.
        let st = unsafe { &*sys_ctx.cast::<SysHostProbe>() };
        if st.return_refreshed {
            (&raw const st.refreshed_ctx).cast::<BamHostCtx>()
        } else {
            core::ptr::null()
        }
    }

    let orig_ctx_token = 1u64;
    let orig_host_ctx = (&raw const orig_ctx_token).cast::<BamHostCtx>();
    let mut api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    api.sys_host_ctx = Some(probe_sys_host_ctx);
    let host = make_valid_host(api, orig_host_ctx);

    let return_refreshed: bool = kani::any();
    let want_err: bool = kani::any();
    let mut probe = SysHostProbe {
        refreshed_ctx: 2u64,
        return_refreshed,
    };
    let sys_ctx = (&raw mut probe).cast::<BamSystemCtx>();
    let orig_host_ctx_addr = orig_host_ctx as usize;
    let expected_ctx_addr = if return_refreshed {
        (&raw const probe.refreshed_ctx).cast::<BamHostCtx>() as usize
    } else {
        orig_host_ctx_addr
    };

    let sys_fn = move |inv: &mut ValidatedSystemInvocation<'_>| -> FfiResult {
        assert_eq!(inv.query_count(), 1);
        assert_eq!(inv.host().host_ctx() as usize, expected_ctx_addr);
        if want_err {
            FfiResult::INVALID_ARGUMENT
        } else {
            FfiResult::OK
        }
    };
    let mut sys_state = SystemClosureState {
        host: PinnedHostPtr::new(&raw const host),
        total_query_count: 1,
        user_query_count: 1,
        resource_query_index: None,
        host_scratch: InvocationHostScratch::new(),
        slot_scratches: SlotScratchesCell::empty(),
        closure: ReentrantCell::new(sys_fn),
    };
    let sys_ud = core::ptr::from_mut(&mut sys_state).cast::<c_void>();
    fn sys_trampoline_for<F>(_: &SystemClosureState<F>) -> bamabi_sys::BamSystemFn
    where
        F: FnMut(&mut ValidatedSystemInvocation<'_>) -> FfiResult + Send + 'static,
    {
        system_closure_trampoline::<F>
    }
    let sys_trampoline = sys_trampoline_for(&sys_state);

    // Null `sys_ctx` and null `user_data` -> NULL_POINTER.
    // SAFETY: Null pointer arguments are rejected before dereferencing.
    let null_sys_res = unsafe { sys_trampoline(core::ptr::null_mut(), sys_ud) };
    assert_eq!(null_sys_res, FfiResult::NULL_POINTER);
    // SAFETY: Null pointer arguments are rejected before dereferencing.
    let null_ud_res = unsafe { sys_trampoline(sys_ctx, core::ptr::null_mut()) };
    assert_eq!(null_ud_res, FfiResult::NULL_POINTER);

    // Misaligned `user_data` -> INVALID_ARGUMENT.
    let mut align_buf = [0u64; 2];
    let misalign: usize = kani::any();
    kani::assume(misalign >= 1 && misalign < align_of::<usize>());
    let bad_ud = core::ptr::from_mut(&mut align_buf)
        .cast::<u8>()
        .wrapping_add(misalign)
        .cast::<c_void>();
    // SAFETY: Alignment guard rejects `bad_ud` before dereferencing.
    let misaligned_sys_res = unsafe { sys_trampoline(sys_ctx, bad_ud) };
    assert_eq!(misaligned_sys_res, FfiResult::INVALID_ARGUMENT);

    // Valid invocation -> OK or closure error.
    // SAFETY: `sys_ctx` and `sys_ud` are non-null, aligned, and initialized for this call.
    let valid_sys_res = unsafe { sys_trampoline(sys_ctx, sys_ud) };
    assert_eq!(
        valid_sys_res,
        if want_err {
            FfiResult::INVALID_ARGUMENT
        } else {
            FfiResult::OK
        }
    );

    // Re-entrant invocation -> ACCESS_CONFLICT without touching inner closure.
    let mut reent_sys_res = FfiResult::OK;
    let locked = sys_state.closure.try_with_mut(|_| {
        // SAFETY: `sys_state.closure` is locked; trampoline must reject with ACCESS_CONFLICT.
        reent_sys_res = unsafe { sys_trampoline(sys_ctx, sys_ud) };
    });
    assert!(locked.is_some());
    assert_eq!(reent_sys_res, FfiResult::ACCESS_CONFLICT);

    kani::cover!(null_sys_res == FfiResult::NULL_POINTER);
    kani::cover!(misaligned_sys_res == FfiResult::INVALID_ARGUMENT);
    kani::cover!(valid_sys_res == FfiResult::OK);
    kani::cover!(valid_sys_res == FfiResult::INVALID_ARGUMENT);
    kani::cover!(reent_sys_res == FfiResult::ACCESS_CONFLICT);

    // `run_if_closure_trampoline`: null, misaligned, valid (true/false), and re-entrancy.
    let pred_flag: bool = kani::any();
    let run_if_fn = move |h: &Host| -> bool {
        assert_eq!(h.host_ctx() as usize, orig_host_ctx_addr);
        pred_flag
    };
    let mut run_if_state = RunIfClosureState {
        host: PinnedHostPtr::new(&raw const host),
        closure: ReentrantCell::new(run_if_fn),
    };
    let run_ud = core::ptr::from_mut(&mut run_if_state).cast::<c_void>();
    fn run_if_trampoline_for<R>(_: &RunIfClosureState<R>) -> bamabi_sys::BamRunIfFn
    where
        R: FnMut(&Host) -> bool + Send + 'static,
    {
        run_if_closure_trampoline::<R>
    }
    let run_if_trampoline = run_if_trampoline_for(&run_if_state);

    // SAFETY: Null `user_data` returns FfiBool::FALSE without dereferencing.
    assert!(!unsafe { run_if_trampoline(core::ptr::null_mut()) }.get());
    // SAFETY: Misaligned `user_data` returns FfiBool::FALSE without dereferencing.
    assert!(!unsafe { run_if_trampoline(bad_ud) }.get());
    // SAFETY: Valid `run_ud` invokes predicate safely.
    let run_res = unsafe { run_if_trampoline(run_ud) };
    assert_eq!(run_res.get(), pred_flag);

    let mut reent_run_res = FfiBool::TRUE;
    let run_locked = run_if_state.closure.try_with_mut(|_| {
        // SAFETY: `run_if_state.closure` is locked; trampoline must return FALSE without re-entering.
        reent_run_res = unsafe { run_if_trampoline(run_ud) };
    });
    assert!(run_locked.is_some());
    assert!(!reent_run_res.get());

    kani::cover!(run_res.get());
    kani::cover!(!run_res.get());
    kani::cover!(!reent_run_res.get());
}

/// PO-G8 (`ValidatedSystemSlice` & `system_closure_slice_trampoline::<N, F>`): Proves query count
/// clamping, single-take exclusivity, null/misaligned pointer rejection, lazy `sys_host_ctx`
/// refresh via `host_scratch`, re-entrancy rejection (`ACCESS_CONFLICT`), and panic-freedom for
/// `N = 0` and `N = 1`.
#[kani::proof]
fn po_g8_system_slice_trampoline_and_clamping() {
    #[repr(C)]
    struct SliceSysHostProbe {
        refreshed_ctx: u64,
        return_refreshed: bool,
    }

    unsafe extern "C" fn probe_slice_sys_host_ctx(
        sys_ctx: *const BamSystemCtx,
    ) -> *const BamHostCtx {
        // SAFETY: Proof binds `sys_ctx` to `SliceSysHostProbe`.
        let st = unsafe { &*sys_ctx.cast::<SliceSysHostProbe>() };
        if st.return_refreshed {
            (&raw const st.refreshed_ctx).cast::<BamHostCtx>()
        } else {
            core::ptr::null()
        }
    }

    let orig_ctx_token = 1u64;
    let orig_host_ctx = (&raw const orig_ctx_token).cast::<BamHostCtx>();
    let mut api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    api.sys_host_ctx = Some(probe_slice_sys_host_ctx);
    let host = make_valid_host(api, orig_host_ctx);

    let total: usize = kani::any();
    let user: usize = kani::any();
    let res_idx: Option<usize> = if kani::any() { Some(kani::any()) } else { None };
    let mut dummy_sys = 7u8;
    // SAFETY: Non-null dummy `BamSystemCtx` for metadata clamping & single-take checks.
    let mut slice = unsafe {
        ValidatedSystemSlice::from_raw_for_kani(
            (&raw mut dummy_sys).cast::<BamSystemCtx>(),
            &host,
            total,
            user,
            res_idx,
        )
    };
    let clamped_total = total.min(bamabi_sys::MAX_QUERIES_PER_SYSTEM);
    assert!(slice.query_count() <= clamped_total);
    if let Some(r) = slice.resource_query_index() {
        assert!(r < clamped_total);
    }
    assert!(slice.take_system_ref().is_some());
    assert!(slice.take_system_ref().is_none());
    let _ = slice.take_lazy_commands();
    assert!(!slice.take_lazy_commands().resolve().is_valid());
    let probe_idx: usize = kani::any();
    let first = slice.take_raw_query_slot(probe_idx);
    assert_eq!(first.is_some(), probe_idx < clamped_total);
    assert!(slice.take_raw_query_slot(probe_idx).is_none());

    let return_refreshed: bool = kani::any();
    let mut probe = SliceSysHostProbe {
        refreshed_ctx: 2u64,
        return_refreshed,
    };
    let sys_ctx = (&raw mut probe).cast::<BamSystemCtx>();
    let expected_ctx_addr = if return_refreshed {
        (&raw const probe.refreshed_ctx).cast::<BamHostCtx>() as usize
    } else {
        orig_host_ctx as usize
    };

    // Bucket N = 0 trampoline.
    let mut state_0 = SystemClosureState {
        host: PinnedHostPtr::new(&raw const host),
        total_query_count: 0,
        user_query_count: 0,
        resource_query_index: None,
        host_scratch: InvocationHostScratch::new(),
        slot_scratches: SlotScratchesCell::empty(),
        closure: ReentrantCell::new(move |inv: &mut ValidatedSystemSlice<'_>| -> FfiResult {
            assert_eq!(inv.query_count(), 0);
            assert_eq!(inv.host().host_ctx() as usize, expected_ctx_addr);
            FfiResult::OK
        }),
    };
    let ud_0 = core::ptr::from_mut(&mut state_0).cast::<c_void>();
    fn slice_trampoline_for<const N: usize, F>(_: &SystemClosureState<F>) -> bamabi_sys::BamSystemFn
    where
        F: FnMut(&mut ValidatedSystemSlice<'_>) -> FfiResult + Send + 'static,
    {
        system_closure_slice_trampoline::<N, F>
    }
    let tramp_0 = slice_trampoline_for::<0, _>(&state_0);
    // SAFETY: Null pointers are rejected before dereferencing.
    assert_eq!(
        unsafe { tramp_0(core::ptr::null_mut(), ud_0) },
        FfiResult::NULL_POINTER
    );
    // SAFETY: Null pointers are rejected before dereferencing.
    assert_eq!(
        unsafe { tramp_0(sys_ctx, core::ptr::null_mut()) },
        FfiResult::NULL_POINTER
    );
    // SAFETY: `sys_ctx` and `ud_0` are valid for this invocation.
    assert_eq!(unsafe { tramp_0(sys_ctx, ud_0) }, FfiResult::OK);

    // Bucket N = 1 trampoline + misaligned + re-entrancy rejection.
    let mut state_1 = SystemClosureState {
        host: PinnedHostPtr::new(&raw const host),
        total_query_count: 1,
        user_query_count: 1,
        resource_query_index: None,
        host_scratch: InvocationHostScratch::new(),
        slot_scratches: SlotScratchesCell::empty(),
        closure: ReentrantCell::new(move |inv: &mut ValidatedSystemSlice<'_>| -> FfiResult {
            assert_eq!(inv.query_count(), 1);
            assert_eq!(inv.host().host_ctx() as usize, expected_ctx_addr);
            FfiResult::OK
        }),
    };
    let ud_1 = core::ptr::from_mut(&mut state_1).cast::<c_void>();
    let tramp_1 = slice_trampoline_for::<1, _>(&state_1);
    let mut align_buf = [0u64; 2];
    let bad_ud = core::ptr::from_mut(&mut align_buf)
        .cast::<u8>()
        .wrapping_add(1)
        .cast::<c_void>();
    // SAFETY: Alignment guard rejects `bad_ud` before dereferencing.
    assert_eq!(
        unsafe { tramp_1(sys_ctx, bad_ud) },
        FfiResult::INVALID_ARGUMENT
    );
    // SAFETY: `sys_ctx` and `ud_1` are valid for this invocation.
    assert_eq!(unsafe { tramp_1(sys_ctx, ud_1) }, FfiResult::OK);
    let mut reent_res = FfiResult::OK;
    let locked = state_1.closure.try_with_mut(|_| {
        // SAFETY: `state_1.closure` is locked; trampoline must return `ACCESS_CONFLICT`.
        reent_res = unsafe { tramp_1(sys_ctx, ud_1) };
    });
    assert!(locked.is_some());
    assert_eq!(reent_res, FfiResult::ACCESS_CONFLICT);
}

/// PO-G8 (`lifecycle_observer_closure_trampoline`): Proves null/misaligned `user_data` rejection,
/// `commands` forwarding, re-entrancy rejection (`ACCESS_CONFLICT`), and panic-freedom.
#[kani::proof]
fn po_g8_lifecycle_observer_trampoline() {
    let dummy_ctx = 1u8;
    let api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    let host = make_valid_host(api, (&raw const dummy_ctx).cast::<BamHostCtx>());

    let target = FfiEntity(kani::any());
    let with_cmds: bool = kani::any();
    let want_err: bool = kani::any();
    let mut dummy_cmds = 1u8;
    let raw_cmds = if with_cmds {
        (&raw mut dummy_cmds).cast::<BamCommands>()
    } else {
        core::ptr::null_mut()
    };

    let cb = move |got_target: FfiEntity, cmds: &mut ValidatedCommands<'_>| -> FfiResult {
        assert_eq!(got_target, target);
        assert_eq!(cmds.is_valid(), with_cmds);
        if want_err {
            FfiResult::INVALID_ARGUMENT
        } else {
            FfiResult::OK
        }
    };
    let mut state = ObserverTrampolineState {
        host: PinnedHostPtr::new(&raw const host),
        callback: ReentrantCell::new(cb),
    };
    let ud = core::ptr::from_mut(&mut state).cast::<c_void>();
    fn lifecycle_trampoline_for<F>(_: &ObserverTrampolineState<F>) -> bamabi_sys::BamObserverFn
    where
        F: FnMut(FfiEntity, &mut ValidatedCommands<'_>) -> FfiResult + Send + 'static,
    {
        lifecycle_observer_closure_trampoline::<F>
    }
    let lifecycle_trampoline = lifecycle_trampoline_for(&state);

    // Null `user_data` -> NULL_POINTER.
    // SAFETY: Null `user_data` is rejected before dereferencing.
    let null_res =
        unsafe { lifecycle_trampoline(target, core::ptr::null(), raw_cmds, core::ptr::null_mut()) };
    assert_eq!(null_res, FfiResult::NULL_POINTER);

    // Misaligned `user_data` -> INVALID_ARGUMENT.
    let mut align_buf = [0u64; 2];
    let misalign: usize = kani::any();
    kani::assume(misalign >= 1 && misalign < align_of::<usize>());
    let bad_ud = core::ptr::from_mut(&mut align_buf)
        .cast::<u8>()
        .wrapping_add(misalign)
        .cast::<c_void>();
    // SAFETY: Alignment guard rejects `bad_ud` before dereferencing.
    let misaligned_res =
        unsafe { lifecycle_trampoline(target, core::ptr::null(), raw_cmds, bad_ud) };
    assert_eq!(misaligned_res, FfiResult::INVALID_ARGUMENT);

    // Valid invocation -> OK or closure error.
    // SAFETY: `ud` is non-null, aligned, and `raw_cmds` is either null or valid stack storage.
    let valid_res = unsafe { lifecycle_trampoline(target, core::ptr::null(), raw_cmds, ud) };
    assert_eq!(
        valid_res,
        if want_err {
            FfiResult::INVALID_ARGUMENT
        } else {
            FfiResult::OK
        }
    );

    // Re-entrant invocation -> ACCESS_CONFLICT.
    let mut reent_res = FfiResult::OK;
    let locked = state.callback.try_with_mut(|_| {
        // SAFETY: `state.callback` is locked; trampoline must return ACCESS_CONFLICT.
        reent_res = unsafe { lifecycle_trampoline(target, core::ptr::null(), raw_cmds, ud) };
    });
    assert!(locked.is_some());
    assert_eq!(reent_res, FfiResult::ACCESS_CONFLICT);

    kani::cover!(null_res == FfiResult::NULL_POINTER);
    kani::cover!(misaligned_res == FfiResult::INVALID_ARGUMENT);
    kani::cover!(valid_res == FfiResult::OK);
    kani::cover!(valid_res == FfiResult::INVALID_ARGUMENT);
    kani::cover!(reent_res == FfiResult::ACCESS_CONFLICT);
}

/// PO-G8 (`par_chunk_task_trampoline`): Proves null `sub_iter`, null `user_data`, misaligned
/// `user_data`, and panic-free chunk dispatch for both `par_for_each_chunk` and
/// `par_for_each_chunk_readonly`.
#[kani::proof]
#[kani::unwind(4)]
fn po_g8_par_chunk_task_trampoline() {
    #[repr(C)]
    struct ParIterState {
        yielded: bool,
        null_sub_res: FfiResult,
        null_ud_res: FfiResult,
        misaligned_ud_res: FfiResult,
    }

    unsafe extern "C" fn mock_sub_next(iter: *mut BamIter, out_len: *mut usize) -> FfiBool {
        // SAFETY: Test binds `iter` to `ParIterState`.
        let st = unsafe { &mut *iter.cast::<ParIterState>() };
        if st.yielded {
            return FfiBool::FALSE;
        }
        st.yielded = true;
        // SAFETY: Caller provides writable `out_len`.
        unsafe { *out_len = 2 };
        FfiBool::TRUE
    }

    unsafe extern "C" fn mock_par_dispatch(
        iter: *mut BamIter,
        task_fn: Option<bamabi_sys::BamChunkTaskFn>,
        user_data: *mut c_void,
    ) -> FfiResult {
        // SAFETY: Test binds `iter` to `ParIterState`.
        let st = unsafe { &mut *iter.cast::<ParIterState>() };
        let f = task_fn.expect("par_for_each_chunk must pass Some(par_chunk_task_trampoline)");

        // 1. Null sub_iter -> NULL_POINTER.
        // SAFETY: Null pointer guard rejects before dereferencing.
        st.null_sub_res = unsafe { f(core::ptr::null_mut(), user_data) };
        assert_eq!(st.null_sub_res, FfiResult::NULL_POINTER);

        // 2. Null user_data -> NULL_POINTER.
        // SAFETY: Null pointer guard rejects before dereferencing.
        st.null_ud_res = unsafe { f(iter, core::ptr::null_mut()) };
        assert_eq!(st.null_ud_res, FfiResult::NULL_POINTER);

        // 3. Misaligned user_data -> INVALID_ARGUMENT.
        let mut align_buf = [0u64; 2];
        // SAFETY: `align_buf` has 16 bytes; `+ 1` is strictly misaligned for `ParChunkTaskContext`.
        let bad_ud = core::ptr::from_mut(&mut align_buf)
            .cast::<u8>()
            .wrapping_add(1)
            .cast::<c_void>();
        // SAFETY: Alignment guard rejects `bad_ud` before dereferencing.
        st.misaligned_ud_res = unsafe { f(iter, bad_ud) };
        assert_eq!(st.misaligned_ud_res, FfiResult::INVALID_ARGUMENT);

        // 4. Valid sub_iter + user_data.
        // SAFETY: `iter` and `user_data` are valid and aligned for this call.
        unsafe { f(iter, user_data) }
    }

    let dummy_ctx = 1u8;
    let mut api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    api.iter_next_chunk = Some(mock_sub_next);
    api.par_for_each_chunk = Some(mock_par_dispatch);
    api.par_for_each_chunk_readonly = Some(mock_par_dispatch);
    let host = make_valid_host(api, (&raw const dummy_ctx).cast::<BamHostCtx>());

    let want_err: bool = kani::any();
    let use_readonly: bool = kani::any();
    let mut st = ParIterState {
        yielded: false,
        null_sub_res: FfiResult::OK,
        null_ud_res: FfiResult::OK,
        misaligned_ud_res: FfiResult::OK,
    };
    let hits = AtomicUsize::new(0);
    // SAFETY: `st` stays pinned on the stack across `iter`.
    let mut iter = unsafe { ValidatedIter::from_raw((&raw mut st).cast::<BamIter>(), &host) };

    let task = |sub: &mut ValidatedIter<'_>| -> FfiResult {
        assert_eq!(sub.current_len(), 2);
        hits.fetch_add(1, Ordering::Relaxed);
        if want_err {
            FfiResult::INVALID_ARGUMENT
        } else {
            FfiResult::OK
        }
    };
    let res = if use_readonly {
        iter.par_for_each_chunk_readonly(task)
    } else {
        iter.par_for_each_chunk(task)
    };

    assert_eq!(hits.load(Ordering::Relaxed), 1);
    assert_eq!(
        res,
        if want_err {
            FfiResult::INVALID_ARGUMENT
        } else {
            FfiResult::OK
        }
    );
    kani::cover!(res == FfiResult::OK);
    kani::cover!(res == FfiResult::INVALID_ARGUMENT);
    kani::cover!(st.null_sub_res == FfiResult::NULL_POINTER);
    kani::cover!(st.misaligned_ud_res == FfiResult::INVALID_ARGUMENT);
}

/// PO-G8 (`pod_on_add_trampoline` & `pod_on_remove_trampoline`): Proves null/misaligned
/// `user_data` and `component_ptr` rejection, ZST null-pointer handling, re-entrancy rejection,
/// and panic-freedom for POD component lifecycle hooks.
#[kani::proof]
fn po_g8_pod_component_hook_trampolines() {
    static ADD_HITS: AtomicUsize = AtomicUsize::new(0);
    static REM_HITS: AtomicUsize = AtomicUsize::new(0);
    static ZST_HITS: AtomicUsize = AtomicUsize::new(0);

    fn on_add_u64(e: FfiEntity, v: &u64) {
        assert_eq!(e, FfiEntity::from_bits(7));
        assert_eq!(*v, 0x1234_5678);
        ADD_HITS.fetch_add(1, Ordering::Relaxed);
    }
    fn on_rem_u64(e: FfiEntity, v: &u64) {
        assert_eq!(e, FfiEntity::from_bits(7));
        assert_eq!(*v, 0x1234_5678);
        REM_HITS.fetch_add(1, Ordering::Relaxed);
    }
    fn on_add_zst(e: FfiEntity, _: &ZstPod) {
        assert_eq!(e, FfiEntity::from_bits(7));
        ZST_HITS.fetch_add(1, Ordering::Relaxed);
    }

    let entity = FfiEntity::from_bits(7);
    let val: u64 = 0x1234_5678;
    let test_remove: bool = kani::any();

    let mut state = HookTrampolineState::<u64> {
        on_add: Some(ReentrantCell::new(Box::new(
            on_add_u64 as fn(FfiEntity, &u64),
        ))),
        on_insert: None,
        on_discard: None,
        on_remove: Some(ReentrantCell::new(Box::new(
            on_rem_u64 as fn(FfiEntity, &u64),
        ))),
        on_despawn: None,
    };
    let ud = core::ptr::from_mut(&mut state).cast::<c_void>();
    let comp_ptr = (&raw const val).cast::<c_void>();

    let mut align_buf = [0u64; 2];
    let misalign: usize = kani::any();
    kani::assume(misalign >= 1 && misalign < align_of::<u64>());
    let bad_ptr = core::ptr::from_mut(&mut align_buf)
        .cast::<u8>()
        .wrapping_add(misalign)
        .cast::<c_void>();

    if test_remove {
        // SAFETY: Null/misaligned guards reject before dereferencing.
        unsafe {
            pod_on_remove_trampoline::<u64>(entity, comp_ptr, core::ptr::null_mut());
            pod_on_remove_trampoline::<u64>(entity, comp_ptr, bad_ptr);
            pod_on_remove_trampoline::<u64>(entity, core::ptr::null(), ud);
            pod_on_remove_trampoline::<u64>(entity, bad_ptr, ud);
        }
        assert_eq!(REM_HITS.load(Ordering::Relaxed), 0);

        // Re-entrancy rejection while `on_remove` cell is locked.
        let locked = state
            .on_remove
            .as_ref()
            .expect("on_remove populated")
            .try_with_mut(|_| {
                // SAFETY: `on_remove` cell is locked; trampoline must ignore re-entry.
                unsafe { pod_on_remove_trampoline::<u64>(entity, comp_ptr, ud) };
            });
        assert!(locked.is_some());
        assert_eq!(REM_HITS.load(Ordering::Relaxed), 0);

        // Valid invocation runs callback once.
        // SAFETY: `comp_ptr` and `ud` are valid and aligned.
        unsafe { pod_on_remove_trampoline::<u64>(entity, comp_ptr, ud) };
        assert_eq!(REM_HITS.load(Ordering::Relaxed), 1);
    } else {
        // SAFETY: Null/misaligned guards reject before dereferencing.
        unsafe {
            pod_on_add_trampoline::<u64>(entity, comp_ptr, core::ptr::null_mut());
            pod_on_add_trampoline::<u64>(entity, comp_ptr, bad_ptr);
            pod_on_add_trampoline::<u64>(entity, core::ptr::null(), ud);
            pod_on_add_trampoline::<u64>(entity, bad_ptr, ud);
        }
        assert_eq!(ADD_HITS.load(Ordering::Relaxed), 0);

        // Re-entrancy rejection while `on_add` cell is locked.
        let locked = state
            .on_add
            .as_ref()
            .expect("on_add populated")
            .try_with_mut(|_| {
                // SAFETY: `on_add` cell is locked; trampoline must ignore re-entry.
                unsafe { pod_on_add_trampoline::<u64>(entity, comp_ptr, ud) };
            });
        assert!(locked.is_some());
        assert_eq!(ADD_HITS.load(Ordering::Relaxed), 0);

        // Valid invocation runs callback once.
        // SAFETY: `comp_ptr` and `ud` are valid and aligned.
        unsafe { pod_on_add_trampoline::<u64>(entity, comp_ptr, ud) };
        assert_eq!(ADD_HITS.load(Ordering::Relaxed), 1);
    }

    // ZST `ZstPod` hook accepts null `component_ptr` cleanly.
    let mut zst_state = HookTrampolineState::<ZstPod> {
        on_add: Some(ReentrantCell::new(Box::new(
            on_add_zst as fn(FfiEntity, &ZstPod),
        ))),
        on_insert: None,
        on_discard: None,
        on_remove: None,
        on_despawn: None,
    };
    let zst_ud = core::ptr::from_mut(&mut zst_state).cast::<c_void>();
    // SAFETY: ZST hook uses `NonNull::dangling()` without dereferencing `component_ptr`.
    unsafe {
        pod_on_add_trampoline::<ZstPod>(entity, core::ptr::null(), zst_ud);
        // Unpopulated `on_remove` slot is a safe no-op.
        pod_on_remove_trampoline::<ZstPod>(entity, core::ptr::null(), zst_ud);
    }
    assert_eq!(ZST_HITS.load(Ordering::Relaxed), 1);

    kani::cover!(test_remove && REM_HITS.load(Ordering::Relaxed) == 1);
    kani::cover!(!test_remove && ADD_HITS.load(Ordering::Relaxed) == 1);
}

/// PO-G8 (`forward_field` & `append_field`): Proves null/misaligned `ctx` rejection, null
/// non-empty view guards, `u32` packet length/field-count overflow guards (`CAPACITY_EXCEEDED`),
/// and panic-freedom on non-POD message witness callbacks.
#[kani::proof]
fn po_g8_non_pod_message_witness_callbacks() {
    unsafe extern "C" fn mock_import_field(
        dst_ptr: *mut c_void,
        field_name: StrView,
        _type_tag: StrView,
        payload: FfiSlice<u8>,
    ) -> FfiResult {
        // SAFETY: `dst_ptr` points to `u32`.
        let dst = unsafe { &mut *dst_ptr.cast::<u32>() };
        if field_name.is_empty() {
            return FfiResult::INVALID_ARGUMENT;
        }
        // SAFETY: `payload` is a valid slice for this synchronous callback.
        let Ok(bytes) = (unsafe { payload.try_as_slice() }) else {
            return FfiResult::NULL_POINTER;
        };
        if bytes.len() == 4 {
            *dst = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            FfiResult::OK
        } else {
            FfiResult::INVALID_ARGUMENT
        }
    }

    let mut dst_val: u32 = 0;
    let mut sink_ctx = ImportSinkCtx {
        dst_ptr: (&raw mut dst_val).cast::<c_void>(),
        import_fn: mock_import_field,
    };
    let sink_ctx_ptr = (&raw mut sink_ctx).cast::<c_void>();

    let mut align_buf = [0u64; 2];
    let misalign: usize = kani::any();
    kani::assume(misalign >= 1 && misalign < align_of::<ImportSinkCtx>());
    let bad_sink_ctx = core::ptr::from_mut(&mut align_buf)
        .cast::<u8>()
        .wrapping_add(misalign)
        .cast::<c_void>();

    // 1. `forward_field`: null/misaligned ctx, rejection from `import_fn`, and valid forwarding.
    // SAFETY: Null/alignment guards reject before dereferencing `ctx`.
    let fwd_null = unsafe {
        forward_field(
            core::ptr::null_mut(),
            StrView::from_str("v"),
            StrView::from_str("u32"),
            FfiSlice::from_slice(&[1, 2, 3, 4]),
        )
    };
    assert_eq!(fwd_null, FfiResult::INVALID_ARGUMENT);
    // SAFETY: Alignment guard rejects `bad_sink_ctx` before dereferencing.
    let fwd_misaligned = unsafe {
        forward_field(
            bad_sink_ctx,
            StrView::from_str("v"),
            StrView::from_str("u32"),
            FfiSlice::from_slice(&[1, 2, 3, 4]),
        )
    };
    assert_eq!(fwd_misaligned, FfiResult::INVALID_ARGUMENT);

    let fwd_want_err: bool = kani::any();
    let fwd_name = if fwd_want_err { "" } else { "v" };
    // SAFETY: `sink_ctx_ptr` is aligned and points to live `ImportSinkCtx` for this call.
    let fwd_res = unsafe {
        forward_field(
            sink_ctx_ptr,
            StrView::from_str(fwd_name),
            StrView::from_str("u32"),
            FfiSlice::from_slice(&[42, 0, 0, 0]),
        )
    };
    if fwd_want_err {
        assert_eq!(fwd_res, FfiResult::INVALID_ARGUMENT);
        assert_eq!(dst_val, 0);
    } else {
        assert_eq!(fwd_res, FfiResult::OK);
        assert_eq!(dst_val, 42);
    }

    // 2. `append_field`: null/misaligned ctx, null views, `isize::MAX`/`u32` length & count overflow.
    let mut builder = PacketBuilder::new();
    let builder_ptr = (&raw mut builder).cast::<c_void>();
    let bad_builder_ctx = core::ptr::from_mut(&mut align_buf)
        .cast::<u8>()
        .wrapping_add(1)
        .cast::<c_void>();

    // SAFETY: Null/alignment guards reject before dereferencing `ctx`.
    assert_eq!(
        unsafe {
            append_field(
                core::ptr::null_mut(),
                StrView::from_str("v"),
                StrView::from_str("u32"),
                FfiSlice::from_slice(&[1]),
            )
        },
        FfiResult::INVALID_ARGUMENT
    );
    // SAFETY: Alignment guard rejects `bad_builder_ctx` before dereferencing.
    assert_eq!(
        unsafe {
            append_field(
                bad_builder_ctx,
                StrView::from_str("v"),
                StrView::from_str("u32"),
                FfiSlice::from_slice(&[1]),
            )
        },
        FfiResult::INVALID_ARGUMENT
    );

    let app_case: u8 = kani::any();
    kani::assume(app_case <= 6);
    let bad_str = StrView {
        ptr: core::ptr::null(),
        len: 1,
    };
    let dummy_byte: u8 = 0;
    let app_res = match app_case {
        0 => {
            // SAFETY: Null non-empty `field_name` rejected with NULL_POINTER.
            unsafe {
                append_field(
                    builder_ptr,
                    bad_str,
                    StrView::from_str("u"),
                    FfiSlice::from_slice(&[1]),
                )
            }
        }
        1 => {
            // SAFETY: Null non-empty `type_tag` rejected with NULL_POINTER.
            unsafe {
                append_field(
                    builder_ptr,
                    StrView::from_str("v"),
                    bad_str,
                    FfiSlice::from_slice(&[1]),
                )
            }
        }
        2 => {
            // SAFETY: Null non-empty `payload` rejected with NULL_POINTER.
            unsafe {
                append_field(
                    builder_ptr,
                    StrView::from_str("v"),
                    StrView::from_str("u"),
                    FfiSlice {
                        ptr: core::ptr::null(),
                        len: 4,
                    },
                )
            }
        }
        3 => {
            // SAFETY: `len > isize::MAX` rejected by `validate()` before `from_raw_parts`.
            unsafe {
                append_field(
                    builder_ptr,
                    StrView::from_str("v"),
                    StrView::from_str("u"),
                    FfiSlice {
                        ptr: &raw const dummy_byte,
                        len: (isize::MAX as usize) + 1,
                    },
                )
            }
        }
        4 => {
            builder.field_count = u32::MAX;
            // SAFETY: `field_count == u32::MAX` triggers `checked_add(1)` -> CAPACITY_EXCEEDED.
            unsafe {
                append_field(
                    builder_ptr,
                    StrView::from_str("v"),
                    StrView::from_str("u"),
                    FfiSlice::from_slice(&[7, 0, 0, 0]),
                )
            }
        }
        5 => {
            // SAFETY: Empty `(null, 0)` C slices are accepted cleanly via `NonNull::dangling()`.
            unsafe {
                append_field(
                    builder_ptr,
                    StrView::empty(),
                    StrView::empty(),
                    FfiSlice::empty(),
                )
            }
        }
        _ => {
            // SAFETY: Valid `builder_ptr` and slices append 1 encoded field.
            unsafe {
                append_field(
                    builder_ptr,
                    StrView::from_str("v"),
                    StrView::from_str("u"),
                    FfiSlice::from_slice(&[7, 0, 0, 0]),
                )
            }
        }
    };

    match app_case {
        0..=3 => {
            assert_eq!(app_res, FfiResult::NULL_POINTER);
            assert_eq!(builder.field_count, 0);
        }
        4 => {
            assert_eq!(app_res, FfiResult::CAPACITY_EXCEEDED);
            assert_eq!(builder.field_count, u32::MAX);
        }
        5 => {
            assert_eq!(app_res, FfiResult::OK);
            assert_eq!(builder.field_count, 1);
            // 4 (header) + 4 (name_len) + 4 (tag_len) + 4 (val_len) = 16 bytes.
            assert_eq!(builder.buf.len(), 16);
        }
        _ => {
            assert_eq!(app_res, FfiResult::OK);
            assert_eq!(builder.field_count, 1);
            // 4 (header) + 4+1 (name) + 4+1 (tag) + 4+4 (val) = 22 bytes.
            assert_eq!(builder.buf.len(), 22);
        }
    }

    kani::cover!(fwd_res == FfiResult::OK);
    kani::cover!(fwd_res == FfiResult::INVALID_ARGUMENT);
    kani::cover!(app_res == FfiResult::OK);
    kani::cover!(app_res == FfiResult::NULL_POINTER);
    kani::cover!(app_res == FfiResult::CAPACITY_EXCEEDED);
}
