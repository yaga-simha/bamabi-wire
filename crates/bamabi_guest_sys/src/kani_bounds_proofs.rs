//! Kani formal verification proof harnesses for `bamabi_guest_sys` bounds and aliasing (`PO-G9`).
//!
//! Targets ABI-threshold `unsafe` operations not covered by `PO-G1`..`PO-G8`:
//! 1. `ValidatedIter::read_messages_batch_checked`: clamping hostile `read_count > max_count`
//!    before `scratch.set_len(actual)` and `MaybeUninit::assume_init`.
//! 2. `validate_spawn_batch_column` & `ValidatedCommands::spawn_batch_columns`: failing closed on
//!    `entity_count * expected_size > isize::MAX` overflow, `col.entity_count < entity_count`,
//!    layout `(size, align)` mismatches, and null non-empty column pointers before `cmd_spawn_batch`.
//! 3. `init_guest_instance`: leaving `*instance_out == null` with clean rollback on every early
//!    error path (null/misaligned `instance_out`, invalid `BamManifest`, invalid UTF-8 / empty
//!    manifest name, failing `Host::from_load_ctx`, and failing `build_fn`).
//! 4. `ValidatedSystemRef` & moved-component commands: fail-closed `component_layout_info` and
//!    null/alignment guards on `entity_get_checked`, `entity_get_mut_checked`,
//!    `res_ptr_by_handle_with_uuid_checked`, `ValidatedCommands::insert_moved`, and
//!    `ValidatedCommands::spawn_moved`.
//! 5. `ValidatedIter::entities_and_column_mut_checked` & `entities_and_read_write_checked`:
//!    rejecting hostile overlapping `(entities, read, write)` pointers with `None` before forming
//!    aliasing slices.

use core::{
    ffi::{CStr, c_char, c_void},
    mem::{align_of, size_of},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use bamabi_sys::{
    ABI_VERSION, AccessMode, BAM_STORAGE_FLAG_IMMUTABLE, BAM_STORAGE_FLAG_MESSAGE,
    BAM_STORAGE_FLAG_RESOURCE, BamChangeTicks, BamColumnInfo, BamCommands, BamComponent,
    BamHostApi, BamHostCtx, BamInstance, BamIter, BamLayoutInfo, BamLoadCtx, BamManifest,
    BamObserverDesc, BamSystemCtx, BamTermDesc, BamTriggerDesc, ComponentDesc, ComponentUuid,
    FfiBool, FfiEntity, FfiResult, ModComponent, SymbolEntry, VTableHeader,
};

use crate::{
    GuestInstance, Host, SpawnBatchColumn, TypedComponent, UuidFilter, ValidatedCommands,
    ValidatedIter, ValidatedLoadCtx, ValidatedSystemInvocation, ValidatedSystemRef,
    commands::validate_spawn_batch_column, init_guest_instance,
};

/// Zero-sized `AbiPod` marker alias for ZST proof paths.
type ZstPod = [u8; 0];

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
unsafe extern "C" fn stub_register_sys(
    _: *mut BamLoadCtx,
    _: *const bamabi_sys::BamSystemDesc,
) -> FfiResult {
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

/// Builds a minimal `BamHostApi` populating only the 27 core ABI `0.2` slots for `PO-G9` harnesses.
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

/// Validates `api` against `ABI_VERSION` and constructs a `Host` preserving typed function
/// pointers for CBMC via `Host::from_api_for_kani`.
fn make_valid_host(api: BamHostApi, host_ctx: *const BamHostCtx) -> Host {
    assert_eq!(api.validate_for_guest(ABI_VERSION), FfiResult::OK);
    Host::from_api_for_kani(api, host_ctx)
}

// Unused callback types referenced only to ensure signature compatibility if needed.
#[allow(dead_code)]
type _UnusedDespawnAll =
    unsafe extern "C" fn(*mut BamCommands, *const BamTermDesc, usize) -> FfiResult;
#[allow(dead_code)]
type _UnusedTriggerEx = unsafe extern "C" fn(*mut BamCommands, *const BamTriggerDesc) -> FfiResult;
#[allow(dead_code)]
type _UnusedObserve =
    unsafe extern "C" fn(*mut BamCommands, *const BamObserverDesc, *mut FfiEntity) -> FfiResult;

// ============================================================================
// PO-G9.1: `read_messages_batch_checked` Hostile Count Clamping
// ============================================================================

/// PO-G9.1: Proves a hostile host returning `read_count > max_count` (including `usize::MAX`)
/// is clamped cleanly to `max_count` so `scratch.set_len(actual)` never exposes uninitialized
/// memory, and all rejection paths (`null`, `UNSUPPORTED`, `TYPE_MISMATCH`, host error) fail closed.
#[kani::proof]
#[kani::unwind(5)]
fn po_g9_read_messages_batch_clamps_hostile_count() {
    #[repr(C)]
    struct MsgIterState {
        info: BamColumnInfo,
        info_status: FfiResult,
        read_status: FfiResult,
        reported_count: usize,
        is_zst: bool,
    }

    unsafe extern "C" fn mock_info(
        iter: *const BamIter,
        term: usize,
        out: *mut BamColumnInfo,
    ) -> FfiResult {
        if term != 0 {
            return FfiResult::OUT_OF_RANGE;
        }
        // SAFETY: Test binds `iter` to `MsgIterState`.
        let st = unsafe { &*iter.cast::<MsgIterState>() };
        if st.info_status.is_ok() {
            // SAFETY: Caller supplies writable `out`.
            unsafe { *out = st.info };
        }
        st.info_status
    }

    unsafe extern "C" fn mock_msg_read_batch(
        iter: *mut BamIter,
        _term: usize,
        out_buf: *mut u8,
        max_count: usize,
        out_count: *mut usize,
    ) -> FfiResult {
        // SAFETY: Test binds `iter` to `MsgIterState`.
        let st = unsafe { &*iter.cast::<MsgIterState>() };
        if !st.read_status.is_ok() {
            return st.read_status;
        }
        let actual_init = st.reported_count.min(max_count);
        if !st.is_zst {
            let typed_out = out_buf.cast::<u64>();
            let mut i = 0usize;
            while i < actual_init {
                // SAFETY: `scratch` in `read_messages_batch_checked` has capacity `max_count` of `u64`,
                // and `actual_init <= max_count`.
                unsafe {
                    typed_out
                        .add(i)
                        .write_unaligned(0xBEEF_0000_0000_0000u64 | (i as u64));
                }
                i += 1;
            }
        }
        // SAFETY: Caller supplies writable `out_count`.
        unsafe { *out_count = st.reported_count };
        FfiResult::OK
    }

    let uuid_ok = ComponentUuid::from_bytes([0x11; 16]);
    let dummy_ctx = 1u8;
    let host_ctx = (&raw const dummy_ctx).cast::<BamHostCtx>();

    let mut api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    api.iter_column_info = Some(mock_info);
    api.msg_read_batch = Some(mock_msg_read_batch);
    let full_host = make_valid_host(api, host_ctx);

    // Null iterator and ABI 0.2 (uncovered `msg_read_batch`) fail closed.
    let mut empty_iter = ValidatedIter::empty(&full_host);
    assert_eq!(
        empty_iter
            .read_messages_batch_checked::<u64>(0, None, 4)
            .unwrap_err(),
        FfiResult::NULL_POINTER
    );

    let mut v02_api = make_core_api(BamHostApi::MIN_V0_2_SIZE as u32, ABI_VERSION);
    v02_api.iter_column_info = Some(mock_info);
    v02_api.msg_read_batch = Some(mock_msg_read_batch);
    let v02_host = make_valid_host(v02_api, host_ctx);

    let max_count: usize = if kani::any() { 0 } else { 2 };
    let reported_count: usize = kani::any();
    let use_v02: bool = kani::any();
    let use_write_mode: bool = kani::any();
    let info_bad: bool = kani::any();
    let read_fails: bool = kani::any();
    let is_zst: bool = kani::any();

    let (elem_size, elem_align) = if is_zst {
        (size_of::<ZstPod>(), align_of::<ZstPod>())
    } else {
        (size_of::<u64>(), align_of::<u64>())
    };
    let flags = if info_bad {
        // Missing `BAM_STORAGE_FLAG_MESSAGE` or conflicting `BAM_STORAGE_FLAG_RESOURCE`.
        BAM_STORAGE_FLAG_RESOURCE
    } else {
        BAM_STORAGE_FLAG_MESSAGE
    };
    let mode = if info_bad {
        AccessMode::READ
    } else if use_write_mode {
        AccessMode::MESSAGE_WRITE
    } else {
        AccessMode::MESSAGE_READ
    };

    let mut state = MsgIterState {
        info: BamColumnInfo {
            component: BamComponent::from_raw(1),
            mode,
            uuid: uuid_ok,
            size: elem_size,
            align: elem_align,
            storage_flags: flags,
            reserved0: 0,
        },
        info_status: FfiResult::OK,
        read_status: if read_fails {
            FfiResult::INTERNAL_ERROR
        } else {
            FfiResult::OK
        },
        reported_count,
        is_zst,
    };

    let host_ref = if use_v02 { &v02_host } else { &full_host };
    // SAFETY: `state` stays pinned on the stack across `iter`.
    let mut iter = unsafe { ValidatedIter::from_raw((&raw mut state).cast::<BamIter>(), host_ref) };

    if is_zst {
        let res = iter.read_messages_batch_checked::<ZstPod>(0, None, max_count);
        if use_v02 {
            assert_eq!(res.as_ref().unwrap_err(), &FfiResult::UNSUPPORTED);
        } else if info_bad {
            assert_eq!(res.as_ref().unwrap_err(), &FfiResult::TYPE_MISMATCH);
        } else if max_count == 0 {
            assert!(res.as_ref().is_ok_and(|v| v.is_empty()));
        } else if read_fails {
            assert_eq!(res.as_ref().unwrap_err(), &FfiResult::INTERNAL_ERROR);
        } else {
            let msgs = res.as_ref().expect("valid ZST message batch must succeed");
            assert_eq!(msgs.len(), reported_count.min(max_count));
        }
        kani::cover!(res.is_ok(), "ZST read_messages_batch_checked Ok reachable");
        kani::cover!(
            res.is_err(),
            "ZST read_messages_batch_checked Err reachable"
        );
    } else {
        let res = iter.read_messages_batch_checked::<u64>(0, None, max_count);
        if use_v02 {
            assert_eq!(res.as_ref().unwrap_err(), &FfiResult::UNSUPPORTED);
        } else if info_bad {
            assert_eq!(res.as_ref().unwrap_err(), &FfiResult::TYPE_MISMATCH);
        } else if max_count == 0 {
            assert!(res.as_ref().is_ok_and(|v| v.is_empty()));
        } else if read_fails {
            assert_eq!(res.as_ref().unwrap_err(), &FfiResult::INTERNAL_ERROR);
        } else {
            let msgs = res.as_ref().expect("valid u64 message batch must succeed");
            let expected_len = reported_count.min(max_count);
            assert_eq!(msgs.len(), expected_len);
            let mut i = 0usize;
            while i < expected_len {
                assert_eq!(msgs[i], 0xBEEF_0000_0000_0000u64 | (i as u64));
                i += 1;
            }
        }
        kani::cover!(res.is_ok(), "read_messages_batch_checked Ok reachable");
        kani::cover!(
            res.is_ok() && max_count > 0 && reported_count > max_count,
            "hostile read_count > max_count clamped Ok reachable"
        );
        kani::cover!(
            res.is_ok() && max_count > 0 && reported_count <= max_count,
            "unclamped read_count <= max_count Ok reachable"
        );
        kani::cover!(res.is_err(), "read_messages_batch_checked Err reachable");
    }
}

// ============================================================================
// PO-G9.2: `validate_spawn_batch_column` & `spawn_batch_columns`
// ============================================================================

/// PO-G9.2: Proves `validate_spawn_batch_column` and `ValidatedCommands::spawn_batch_columns`
/// fail closed before reaching `cmd_spawn_batch` on `entity_count * expected_size > isize::MAX`
/// overflow, `col.entity_count < entity_count`, layout `(size, align)` mismatches, and null
/// non-empty column pointers, and proves `SpawnBatchColumn::typed` skips `component_layout_info`
/// while preserving local `entity_count`, null-pointer, and `isize::MAX` overflow guards.
#[kani::proof]
#[kani::unwind(4)]
fn po_g9_spawn_batch_column_validation_bounds() {
    #[repr(C)]
    struct BatchSpy {
        layout_size: usize,
        layout_align: usize,
        layout_status: FfiResult,
        layout_called: core::cell::Cell<bool>,
        batch_called: bool,
        last_entity_count: usize,
    }

    unsafe extern "C" fn mock_layout_info(
        ctx: *const BamHostCtx,
        comp: BamComponent,
        out: *mut BamLayoutInfo,
    ) -> FfiResult {
        // SAFETY: Test binds `ctx` to `BatchSpy`.
        let spy = unsafe { &*ctx.cast::<BatchSpy>() };
        spy.layout_called.set(true);
        if !spy.layout_status.is_ok() {
            return spy.layout_status;
        }
        let info = if comp == BamComponent::from_raw(2) {
            BamLayoutInfo {
                size: 0,
                align: 1,
                uuid: ComponentUuid::from_bytes([0x02; 16]),
                ..BamLayoutInfo::empty()
            }
        } else {
            BamLayoutInfo {
                size: spy.layout_size,
                align: spy.layout_align,
                uuid: ComponentUuid::from_bytes([0x01; 16]),
                ..BamLayoutInfo::empty()
            }
        };
        // SAFETY: Caller supplies writable `out`.
        unsafe { *out = info };
        FfiResult::OK
    }

    unsafe extern "C" fn mock_spawn_batch(
        raw_cmds: *mut BamCommands,
        _comps: *const BamComponent,
        _ptrs: *const *const u8,
        _comp_count: usize,
        entity_count: usize,
        out_entities: *mut FfiEntity,
    ) -> FfiResult {
        // SAFETY: Test binds `raw_cmds` to `BatchSpy`.
        let spy = unsafe { &mut *raw_cmds.cast::<BatchSpy>() };
        spy.batch_called = true;
        spy.last_entity_count = entity_count;
        if !out_entities.is_null() {
            let mut i = 0usize;
            while i < entity_count {
                // SAFETY: Caller verified `out_entities` has at least `entity_count` writable slots.
                unsafe { *out_entities.add(i) = FfiEntity::from_bits((i + 1) as u64) };
                i += 1;
            }
        }
        FfiResult::OK
    }

    let mut spy = BatchSpy {
        layout_size: size_of::<u64>(),
        layout_align: align_of::<u64>(),
        layout_status: FfiResult::OK,
        layout_called: core::cell::Cell::new(false),
        batch_called: false,
        last_entity_count: 0,
    };

    let mut api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    api.component_layout_info = Some(mock_layout_info);
    api.cmd_spawn_batch = Some(mock_spawn_batch);
    let host = make_valid_host(api, (&raw const spy).cast::<BamHostCtx>());

    let mut v02_api = make_core_api(BamHostApi::MIN_V0_2_SIZE as u32, ABI_VERSION);
    v02_api.component_layout_info = Some(mock_layout_info);
    v02_api.cmd_spawn_batch = Some(mock_spawn_batch);
    let v02_host = make_valid_host(v02_api, (&raw const spy).cast::<BamHostCtx>());

    // 1. Null `ValidatedCommands` fails closed.
    let data = [10u64, 20u64];
    let ok_col = SpawnBatchColumn::new(BamComponent::from_raw(1), &data);
    let mut empty_cmds = ValidatedCommands::empty(&host);
    assert_eq!(
        empty_cmds.spawn_batch_columns(&[ok_col], 2, None),
        FfiResult::NULL_POINTER
    );
    assert!(!spy.batch_called);

    // 2. ABI 0.2 host fails closed with `UNSUPPORTED` before `cmd_spawn_batch`.
    // SAFETY: `spy` is pinned on the stack and used only as an opaque handle for `mock_spawn_batch`.
    let mut v02_cmds =
        unsafe { ValidatedCommands::from_raw((&raw mut spy).cast::<BamCommands>(), &v02_host) };
    assert_eq!(
        v02_cmds.spawn_batch_columns(&[ok_col], 2, None),
        FfiResult::UNSUPPORTED
    );
    assert!(!spy.batch_called);

    // 3. `out_entities` shorter than `entity_count` fails closed with `INVALID_ARGUMENT`.
    // SAFETY: `spy` is pinned on the stack and used as mock `BamCommands`.
    let mut cmds =
        unsafe { ValidatedCommands::from_raw((&raw mut spy).cast::<BamCommands>(), &host) };
    let mut short_out = [FfiEntity::NULL; 1];
    assert_eq!(
        cmds.spawn_batch_columns(&[ok_col], 2, Some(&mut short_out)),
        FfiResult::INVALID_ARGUMENT
    );
    assert!(!spy.batch_called);

    // 4. Symbolic failure and success scenarios on `validate_spawn_batch_column` and
    //    `spawn_batch_columns` (including `validated_typed` fast-path cases 9..=12).
    let case_idx: u8 = kani::any();
    kani::assume(case_idx < 13);

    let dummy_non_null = (&raw const data).cast::<u8>();
    let (col, entity_count, host_size, host_align) = match case_idx {
        // Invalid component handle.
        0 => (
            SpawnBatchColumn::new(BamComponent::INVALID, &data),
            2usize,
            size_of::<u64>(),
            align_of::<u64>(),
        ),
        // `col.entity_count < entity_count`.
        1 => (
            SpawnBatchColumn::new(BamComponent::from_raw(1), &data),
            3usize,
            size_of::<u64>(),
            align_of::<u64>(),
        ),
        // Size mismatch (`u32` vs host `u64`).
        2 => (
            SpawnBatchColumn::new(BamComponent::from_raw(1), &data),
            2usize,
            size_of::<u32>(),
            align_of::<u64>(),
        ),
        // Align mismatch (host reports align 16 vs `u64` align 8).
        3 => (
            SpawnBatchColumn::new(BamComponent::from_raw(1), &data),
            2usize,
            size_of::<u64>(),
            16usize,
        ),
        // Null non-empty column pointer when `elem_size = Some(8)` and `entity_count > 0`.
        4 => (
            // SAFETY: Kani bounds harness probes rejection of null non-empty `column_ptr`.
            unsafe {
                SpawnBatchColumn::from_parts_for_kani(
                    BamComponent::from_raw(1),
                    core::ptr::null(),
                    2,
                    Some(size_of::<u64>()),
                    Some(align_of::<u64>()),
                    false,
                )
            },
            2usize,
            size_of::<u64>(),
            align_of::<u64>(),
        ),
        // `entity_count * expected_size > isize::MAX` (in-range mul exceeding `isize::MAX`).
        5 => {
            let over_half = (isize::MAX as usize / size_of::<u64>()) + 1;
            (
                // SAFETY: Kani bounds harness probes `isize::MAX` overflow rejection before `cmd_spawn_batch`.
                unsafe {
                    SpawnBatchColumn::from_parts_for_kani(
                        BamComponent::from_raw(1),
                        dummy_non_null,
                        over_half,
                        Some(size_of::<u64>()),
                        Some(align_of::<u64>()),
                        false,
                    )
                },
                over_half,
                size_of::<u64>(),
                align_of::<u64>(),
            )
        }
        // `entity_count.checked_mul(expected_size)` returns `None` (`usize` wrap).
        6 => {
            let wrap_count = usize::MAX / 2;
            (
                // SAFETY: Kani bounds harness probes `checked_mul` overflow rejection before `cmd_spawn_batch`.
                unsafe {
                    SpawnBatchColumn::from_parts_for_kani(
                        BamComponent::from_raw(1),
                        dummy_non_null,
                        wrap_count,
                        Some(size_of::<u64>()),
                        Some(align_of::<u64>()),
                        false,
                    )
                },
                wrap_count,
                size_of::<u64>(),
                align_of::<u64>(),
            )
        }
        // Valid non-empty `SpawnBatchColumn::new` + ZST marker.
        7 => (
            SpawnBatchColumn::new(BamComponent::from_raw(1), &data),
            2usize,
            size_of::<u64>(),
            align_of::<u64>(),
        ),
        // Valid `entity_count == 0` with `SpawnBatchColumn::new`.
        8 => (
            SpawnBatchColumn::new(BamComponent::from_raw(1), &data),
            0usize,
            size_of::<u64>(),
            align_of::<u64>(),
        ),
        // Valid `SpawnBatchColumn::typed` skips `component_layout_info` (ignores host mismatch).
        9 => (
            // SAFETY: Kani probes the pre-validated `TypedComponent` fast path skipping `component_layout_info`.
            SpawnBatchColumn::typed(
                unsafe { TypedComponent::<u64>::from_raw_unchecked(BamComponent::from_raw(1)) },
                &data,
            ),
            2usize,
            999usize,
            64usize,
        ),
        // `SpawnBatchColumn::typed` still rejects `col.entity_count < entity_count`.
        10 => (
            // SAFETY: Kani probes local bounds enforcement on the `TypedComponent` fast path.
            SpawnBatchColumn::typed(
                unsafe { TypedComponent::<u64>::from_raw_unchecked(BamComponent::from_raw(1)) },
                &data,
            ),
            3usize,
            999usize,
            64usize,
        ),
        // `validated_typed` still rejects null non-empty `column_ptr`.
        11 => (
            // SAFETY: Kani probes null non-empty pointer rejection on the `validated_typed` fast path.
            unsafe {
                SpawnBatchColumn::from_parts_for_kani(
                    BamComponent::from_raw(1),
                    core::ptr::null(),
                    2,
                    Some(size_of::<u64>()),
                    Some(align_of::<u64>()),
                    true,
                )
            },
            2usize,
            999usize,
            64usize,
        ),
        // `validated_typed` still rejects `entity_count * expected_size > isize::MAX`.
        _ => {
            let over_half = (isize::MAX as usize / size_of::<u64>()) + 1;
            (
                // SAFETY: Kani probes `isize::MAX` byte-length overflow rejection on the `validated_typed` fast path.
                unsafe {
                    SpawnBatchColumn::from_parts_for_kani(
                        BamComponent::from_raw(1),
                        dummy_non_null,
                        over_half,
                        Some(size_of::<u64>()),
                        Some(align_of::<u64>()),
                        true,
                    )
                },
                over_half,
                999usize,
                64usize,
            )
        }
    };

    spy.layout_size = host_size;
    spy.layout_align = host_align;
    spy.layout_called.set(false);
    spy.batch_called = false;

    let col_status = validate_spawn_batch_column(&host, &col, entity_count);
    let mut out_entities = [FfiEntity::NULL; 2];
    let marker_col = SpawnBatchColumn::marker(BamComponent::from_raw(2), entity_count);
    let batch_status = if case_idx == 7 {
        cmds.spawn_batch_columns(&[col, marker_col], entity_count, Some(&mut out_entities))
    } else if case_idx == 9 {
        cmds.spawn_batch_columns(&[col], entity_count, Some(&mut out_entities))
    } else {
        cmds.spawn_batch_columns(&[col], entity_count, None)
    };

    assert_eq!(col_status, batch_status);

    match case_idx {
        0 => assert_eq!(batch_status, FfiResult::INVALID_COMPONENT),
        1 => assert_eq!(batch_status, FfiResult::INVALID_ARGUMENT),
        2 | 3 => assert_eq!(batch_status, FfiResult::TYPE_MISMATCH),
        4 => assert_eq!(batch_status, FfiResult::NULL_POINTER),
        5 | 6 => assert_eq!(batch_status, FfiResult::OUT_OF_RANGE),
        7 => {
            assert_eq!(batch_status, FfiResult::OK);
            assert!(spy.batch_called);
            assert_eq!(spy.last_entity_count, 2);
            assert_eq!(out_entities[0], FfiEntity::from_bits(1));
            assert_eq!(out_entities[1], FfiEntity::from_bits(2));
        }
        8 => {
            assert_eq!(batch_status, FfiResult::OK);
            assert!(spy.batch_called);
            assert_eq!(spy.last_entity_count, 0);
        }
        9 => {
            assert_eq!(batch_status, FfiResult::OK);
            assert!(!spy.layout_called.get());
            assert!(spy.batch_called);
            assert_eq!(spy.last_entity_count, 2);
            assert_eq!(out_entities[0], FfiEntity::from_bits(1));
            assert_eq!(out_entities[1], FfiEntity::from_bits(2));
        }
        10 => {
            assert_eq!(batch_status, FfiResult::INVALID_ARGUMENT);
            assert!(!spy.layout_called.get());
        }
        11 => {
            assert_eq!(batch_status, FfiResult::NULL_POINTER);
            assert!(!spy.layout_called.get());
        }
        _ => {
            assert_eq!(batch_status, FfiResult::OUT_OF_RANGE);
            assert!(!spy.layout_called.get());
        }
    }

    if !batch_status.is_ok() {
        assert!(!spy.batch_called);
    }

    kani::cover!(
        batch_status == FfiResult::OK,
        "spawn_batch_columns Ok reachable"
    );
    kani::cover!(
        case_idx == 9 && batch_status == FfiResult::OK && !spy.layout_called.get(),
        "spawn_batch_columns typed fast-path Ok without layout_info reachable"
    );
    kani::cover!(
        batch_status == FfiResult::OUT_OF_RANGE,
        "spawn_batch_columns OUT_OF_RANGE overflow rejection reachable"
    );
    kani::cover!(
        case_idx == 12 && batch_status == FfiResult::OUT_OF_RANGE && !spy.layout_called.get(),
        "spawn_batch_columns typed OUT_OF_RANGE overflow rejection reachable"
    );
    kani::cover!(
        batch_status == FfiResult::INVALID_ARGUMENT,
        "spawn_batch_columns entity_count mismatch rejection reachable"
    );
    kani::cover!(
        batch_status == FfiResult::TYPE_MISMATCH,
        "spawn_batch_columns layout mismatch rejection reachable"
    );
    kani::cover!(
        batch_status == FfiResult::NULL_POINTER,
        "spawn_batch_columns null pointer rejection reachable"
    );
}

// ============================================================================
// PO-G9.3: `init_guest_instance` Early Error Paths & Rollback
// ============================================================================

/// PO-G9.3a: Proves every pre-builder early error path of `init_guest_instance` (null/misaligned
/// `instance_out`, invalid `BamManifest`, invalid UTF-8 / empty manifest name, and failing
/// `Host::from_load_ctx`) leaves `*instance_out == null` cleanly.
#[kani::proof]
#[kani::unwind(4)]
fn po_g9_init_guest_instance_early_error_paths() {
    let mut dummy_load = 1u8;
    let load_ctx = (&raw mut dummy_load).cast::<BamLoadCtx>();
    let valid_api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    let mut null_ctx_api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    null_ctx_api.load_host_ctx = Some(stub_load_host_ctx_null);

    // SAFETY: Byte literals are NUL-terminated with no interior NUL bytes.
    let valid_name = unsafe { CStr::from_bytes_with_nul_unchecked(b"m\0") };
    // SAFETY: Byte literal is NUL-terminated with no interior NUL bytes.
    let valid_ver = unsafe { CStr::from_bytes_with_nul_unchecked(b"1\0") };
    // SAFETY: Byte literal is NUL-terminated with no interior NUL bytes.
    let empty_name = unsafe { CStr::from_bytes_with_nul_unchecked(b"\0") };
    let bad_utf8_bytes: [u8; 2] = [0xFF, 0x00];

    let case_idx: u8 = kani::any();
    kani::assume(case_idx < 8);

    // Pre-seed `instance_out_slot` with a sentinel non-null pointer so we prove
    // `init_guest_instance` resets `*instance_out` to null on every early error path.
    let mut sentinel_byte = 1u8;
    let mut instance_out_slot: *mut BamInstance = (&raw mut sentinel_byte).cast::<BamInstance>();
    let mut unaligned_buf = [0u64; 2];

    let mut manifest = BamManifest::new(valid_name.as_ptr(), valid_ver.as_ptr());
    let mut api_ptr: *const BamHostApi = &raw const valid_api;
    let mut load_ctx_ptr: *mut BamLoadCtx = load_ctx;
    let mut out_ptr: *mut *mut BamInstance = &raw mut instance_out_slot;

    match case_idx {
        // 0: Null `instance_out`.
        0 => out_ptr = core::ptr::null_mut(),
        // 1: Misaligned non-null `instance_out`.
        1 => {
            let misalign: usize = kani::any();
            kani::assume(misalign >= 1 && misalign < align_of::<*mut BamInstance>());
            out_ptr = (&raw mut unaligned_buf)
                .cast::<u8>()
                .wrapping_add(misalign)
                .cast::<*mut BamInstance>();
        }
        // 2: Invalid `BamManifest` (undersized `struct_size`, unknown flags, nonzero `reserved0`,
        //    or null `name`).
        2 => {
            let sub: u8 = kani::any();
            kani::assume(sub < 4);
            match sub {
                0 => manifest.struct_size = (BamManifest::MIN_V0_2_SIZE - 1) as u32,
                1 => manifest.flags = 1,
                2 => manifest.reserved0 = 1,
                _ => manifest.name = core::ptr::null(),
            }
        }
        // 3: Invalid UTF-8 `manifest.name`.
        3 => manifest.name = bad_utf8_bytes.as_ptr().cast::<c_char>(),
        // 4: Empty `manifest.name`.
        4 => manifest.name = empty_name.as_ptr(),
        // 5: Null `api` in `Host::from_load_ctx`.
        5 => api_ptr = core::ptr::null(),
        // 6: Null `load_ctx` in `Host::from_load_ctx`.
        6 => load_ctx_ptr = core::ptr::null_mut(),
        // 7: `load_host_ctx` returning null.
        _ => api_ptr = &raw const null_ctx_api,
    }

    // SAFETY: Each `case_idx` supplies either a guarded invalid pointer or valid stack storage.
    let res = unsafe {
        init_guest_instance(api_ptr, load_ctx_ptr, out_ptr, &manifest, |_, _| {
            FfiResult::OK
        })
    };

    match case_idx {
        0 => assert_eq!(res, FfiResult::NULL_POINTER),
        1 => assert_eq!(res, FfiResult::INVALID_ARGUMENT),
        2 => {
            assert!(!res.is_ok());
            assert!(instance_out_slot.is_null());
        }
        3 => {
            assert_eq!(res, FfiResult::INVALID_UTF8);
            assert!(instance_out_slot.is_null());
        }
        4 => {
            assert_eq!(res, FfiResult::INVALID_ARGUMENT);
            assert!(instance_out_slot.is_null());
        }
        _ => {
            assert_eq!(res, FfiResult::NULL_POINTER);
            assert!(instance_out_slot.is_null());
        }
    }

    kani::cover!(
        res == FfiResult::INVALID_UTF8,
        "init_guest_instance INVALID_UTF8 reachable"
    );
    kani::cover!(
        res == FfiResult::INVALID_ARGUMENT,
        "init_guest_instance INVALID_ARGUMENT reachable"
    );
    kani::cover!(
        res == FfiResult::NULL_POINTER,
        "init_guest_instance NULL_POINTER reachable"
    );
}

/// PO-G9.3b: Proves a failing `build_fn` in `init_guest_instance` leaves `*instance_out == null`
/// with clean rollback of staged allocations, and a succeeding `build_fn` produces a non-null
/// instance torn down cleanly by `shutdown_guest_instance`.
#[kani::proof]
#[kani::unwind(4)]
fn po_g9_init_guest_instance_build_fn_rollback_and_ok() {
    static ALLOC_DROPPED: AtomicBool = AtomicBool::new(false);

    struct TrackedAlloc;
    impl Drop for TrackedAlloc {
        fn drop(&mut self) {
            ALLOC_DROPPED.store(true, Ordering::Release);
        }
    }

    ALLOC_DROPPED.store(false, Ordering::Release);

    let mut dummy_load = 1u8;
    let load_ctx = (&raw mut dummy_load).cast::<BamLoadCtx>();
    let valid_api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    // SAFETY: Byte literals are NUL-terminated with no interior NUL bytes.
    let valid_name = unsafe { CStr::from_bytes_with_nul_unchecked(b"m\0") };
    // SAFETY: Byte literal is NUL-terminated with no interior NUL bytes.
    let valid_ver = unsafe { CStr::from_bytes_with_nul_unchecked(b"1\0") };
    let manifest = BamManifest::new(valid_name.as_ptr(), valid_ver.as_ptr());

    let mut sentinel_byte = 1u8;
    let mut instance_out_slot: *mut BamInstance = (&raw mut sentinel_byte).cast::<BamInstance>();
    let fail_build: bool = kani::any();
    let build_status = if fail_build {
        FfiResult::INVALID_ARGUMENT
    } else {
        FfiResult::OK
    };

    // SAFETY: All pointers and manifest C strings are valid for this call.
    let res = unsafe {
        init_guest_instance(
            &raw const valid_api,
            load_ctx,
            &raw mut instance_out_slot,
            &manifest,
            |inst: &mut GuestInstance, ctx: ValidatedLoadCtx<'_>| {
                assert_eq!(ctx.as_raw(), load_ctx);
                assert_eq!(inst.manifest_name().len(), 1);
                let _ = inst.track_allocation(Box::new(TrackedAlloc));
                assert_eq!(inst.allocation_count(), 1);
                build_status
            },
        )
    };

    if fail_build {
        assert_eq!(res, FfiResult::INVALID_ARGUMENT);
        assert!(instance_out_slot.is_null());
        assert!(ALLOC_DROPPED.load(Ordering::Acquire));
    } else {
        assert_eq!(res, FfiResult::OK);
        assert!(!instance_out_slot.is_null());
        assert!(!ALLOC_DROPPED.load(Ordering::Acquire));
    }

    kani::cover!(res == FfiResult::OK, "init_guest_instance Ok reachable");
    kani::cover!(!res.is_ok(), "init_guest_instance Err rollback reachable");
}

// ============================================================================
// PO-G9.4: `ValidatedSystemRef` & Moved-Component Commands
// ============================================================================

/// PO-G9.4a: Proves `ValidatedSystemRef::entity_get_checked`, `entity_get_mut_checked`, and
/// `res_ptr_by_handle_with_uuid_checked` fail closed on missing `component_layout_info`,
/// mismatched layout/UUID/flags, and null or misaligned host pointers.
#[kani::proof]
fn po_g9_validated_system_ref_entity_and_resource_guards() {
    #[repr(C)]
    struct SysRefMock {
        storage: [u64; 3],
        info: BamColumnInfo,
        layout_info: BamLayoutInfo,
        layout_status: FfiResult,
        return_null: bool,
        misalign: usize,
        ticks: BamChangeTicks,
    }

    unsafe extern "C" fn mock_sys_query(
        ctx: *mut BamSystemCtx,
        _idx: usize,
        out: *mut *mut BamIter,
    ) -> FfiResult {
        // SAFETY: Caller supplies writable `out`.
        unsafe { *out = ctx.cast::<BamIter>() };
        FfiResult::OK
    }

    unsafe extern "C" fn mock_iter_info(
        iter: *const BamIter,
        term: usize,
        out: *mut BamColumnInfo,
    ) -> FfiResult {
        if term != 0 {
            return FfiResult::OUT_OF_RANGE;
        }
        // SAFETY: Test binds `iter` to `SysRefMock`.
        let st = unsafe { &*iter.cast::<SysRefMock>() };
        // SAFETY: Caller supplies writable `out`.
        unsafe { *out = st.info };
        FfiResult::OK
    }

    unsafe extern "C" fn mock_entity_get(
        ctx: *const BamSystemCtx,
        _q: usize,
        _e: FfiEntity,
        _t: usize,
    ) -> *const u8 {
        // SAFETY: Test binds `ctx` to `SysRefMock`.
        let st = unsafe { &*ctx.cast::<SysRefMock>() };
        if st.return_null {
            return core::ptr::null();
        }
        st.storage.as_ptr().cast::<u8>().wrapping_add(st.misalign)
    }

    unsafe extern "C" fn mock_entity_get_mut(
        ctx: *mut BamSystemCtx,
        _q: usize,
        _e: FfiEntity,
        _t: usize,
    ) -> *mut u8 {
        // SAFETY: Test binds `ctx` to `SysRefMock`.
        let st = unsafe { &mut *ctx.cast::<SysRefMock>() };
        if st.return_null {
            return core::ptr::null_mut();
        }
        st.storage
            .as_mut_ptr()
            .cast::<u8>()
            .wrapping_add(st.misalign)
    }

    unsafe extern "C" fn mock_layout_info(
        ctx: *const BamHostCtx,
        _comp: BamComponent,
        out: *mut BamLayoutInfo,
    ) -> FfiResult {
        // SAFETY: Test binds `ctx` to `SysRefMock`.
        let st = unsafe { &*ctx.cast::<SysRefMock>() };
        if !st.layout_status.is_ok() {
            return st.layout_status;
        }
        // SAFETY: Caller supplies writable `out`.
        unsafe { *out = st.layout_info };
        FfiResult::OK
    }

    unsafe extern "C" fn mock_res_ptr(
        ctx: *const BamSystemCtx,
        _comp: BamComponent,
        out_ticks: *mut BamChangeTicks,
    ) -> *const u8 {
        // SAFETY: Test binds `ctx` to `SysRefMock`.
        let st = unsafe { &*ctx.cast::<SysRefMock>() };
        if !out_ticks.is_null() {
            // SAFETY: Caller supplies writable `out_ticks`.
            unsafe { *out_ticks = st.ticks };
        }
        if st.return_null {
            return core::ptr::null();
        }
        st.storage.as_ptr().cast::<u8>().wrapping_add(st.misalign)
    }

    let uuid_ok = ComponentUuid::from_bytes([0x33; 16]);
    let uuid_bad = ComponentUuid::from_bytes([0x44; 16]);
    let init_val: u64 = kani::any();

    let mut mock = SysRefMock {
        storage: [init_val, 0, 0],
        info: BamColumnInfo {
            component: BamComponent::from_raw(1),
            mode: AccessMode::WRITE,
            uuid: uuid_ok,
            size: size_of::<u64>(),
            align: align_of::<u64>(),
            storage_flags: 0,
            reserved0: 0,
        },
        layout_info: BamLayoutInfo {
            size: size_of::<u64>(),
            align: align_of::<u64>(),
            uuid: uuid_ok,
            ..BamLayoutInfo::empty()
        },
        layout_status: FfiResult::OK,
        return_null: false,
        misalign: 0,
        ticks: BamChangeTicks::new(1, 2, 3, 4),
    };

    let mut api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    api.sys_query = Some(mock_sys_query);
    api.iter_column_info = Some(mock_iter_info);
    api.sys_entity_get = Some(mock_entity_get);
    api.sys_entity_get_mut = Some(mock_entity_get_mut);
    api.component_layout_info = Some(mock_layout_info);
    api.sys_res_ptr = Some(mock_res_ptr);
    let full_host = make_valid_host(api, (&raw const mock).cast::<BamHostCtx>());

    let mut v02_api = make_core_api(BamHostApi::MIN_V0_2_SIZE as u32, ABI_VERSION);
    v02_api.sys_query = Some(mock_sys_query);
    v02_api.iter_column_info = Some(mock_iter_info);
    v02_api.sys_entity_get = Some(mock_entity_get);
    v02_api.sys_entity_get_mut = Some(mock_entity_get_mut);
    v02_api.component_layout_info = Some(mock_layout_info);
    v02_api.sys_res_ptr = Some(mock_res_ptr);
    let v02_host = make_valid_host(v02_api, (&raw const mock).cast::<BamHostCtx>());

    // Empty `ValidatedSystemRef` always rejects.
    let empty_ref = ValidatedSystemRef::empty(&full_host);
    // SAFETY: `mock` is pinned on stack and used as mock `BamIter`.
    let mut raw_iter =
        unsafe { ValidatedIter::from_raw((&raw mut mock).cast::<BamIter>(), &full_host) };
    assert!(
        empty_ref
            .entity_get_checked::<u64>(&raw_iter, 0, 0, FfiEntity::from_bits(1), None)
            .is_none()
    );
    assert!(
        empty_ref
            .res_ptr_by_handle_with_uuid_checked::<u64>(BamComponent::from_raw(1), None)
            .is_none()
    );

    // ABI 0.2 host without `component_layout_info` / `sys_entity_get` / `sys_res_ptr` fails closed.
    // SAFETY: `mock` is pinned on stack as `BamSystemCtx`.
    let mut v02_inv = unsafe {
        ValidatedSystemInvocation::from_raw(
            (&raw mut mock).cast::<BamSystemCtx>(),
            &v02_host,
            1,
            1,
            None,
        )
    };
    let mut v02_ref = v02_inv.take_system_ref().expect("sys_ref present");
    let mut v02_iter = v02_inv.take_raw_query_slot(0).expect("query 0 present");
    assert!(
        v02_ref
            .entity_get_checked::<u64>(&v02_iter, 0, 0, FfiEntity::from_bits(1), None)
            .is_none()
    );
    assert!(
        v02_ref
            .entity_get_mut_checked::<u64>(&mut v02_iter, 0, 0, FfiEntity::from_bits(1), None)
            .is_none()
    );
    assert!(
        v02_ref
            .res_ptr_by_handle_with_uuid_checked::<u64>(BamComponent::from_raw(1), None)
            .is_none()
    );

    // Symbolic tests on full ABI 0.3 host.
    let return_null: bool = kani::any();
    let misalign: usize = kani::any();
    kani::assume(misalign < align_of::<u64>());
    let bad_mode_or_flag: bool = kani::any();
    let bad_layout: bool = kani::any();
    let bad_uuid: bool = kani::any();
    let layout_err: bool = kani::any();
    let invalid_comp: bool = kani::any();

    mock.return_null = return_null;
    mock.misalign = misalign;
    if bad_mode_or_flag {
        mock.info.mode = AccessMode::READ;
        mock.info.storage_flags = BAM_STORAGE_FLAG_IMMUTABLE;
    }
    if bad_layout {
        mock.info.size = size_of::<u32>();
        mock.layout_info.size = size_of::<u32>();
    }
    if bad_uuid {
        mock.info.uuid = uuid_bad;
        mock.layout_info.uuid = uuid_bad;
    }
    if layout_err {
        mock.layout_status = FfiResult::NOT_FOUND;
    }

    // SAFETY: `mock` is pinned on stack as `BamSystemCtx`.
    let mut inv = unsafe {
        ValidatedSystemInvocation::from_raw(
            (&raw mut mock).cast::<BamSystemCtx>(),
            &full_host,
            1,
            1,
            None,
        )
    };
    let mut sys_ref = inv.take_system_ref().expect("sys_ref present");

    let get_res = sys_ref.entity_get_checked::<u64>(
        &raw_iter,
        0,
        0,
        FfiEntity::from_bits(1),
        Some(UuidFilter::exact(uuid_ok)),
    );
    let get_expect_ok = !bad_layout && !bad_uuid && !return_null && misalign == 0;
    assert_eq!(get_res.is_some(), get_expect_ok);
    if let Some(val) = get_res {
        assert_eq!(*val, init_val);
    }
    kani::cover!(get_res.is_some(), "entity_get_checked Ok reachable");
    kani::cover!(get_res.is_none(), "entity_get_checked Err reachable");

    let comp = if invalid_comp {
        BamComponent::INVALID
    } else {
        BamComponent::from_raw(1)
    };
    let res_ptr_res =
        sys_ref.res_ptr_by_handle_with_uuid_checked::<u64>(comp, Some(UuidFilter::exact(uuid_ok)));
    let res_expect_ok =
        !invalid_comp && !layout_err && !bad_layout && !bad_uuid && !return_null && misalign == 0;
    assert_eq!(res_ptr_res.is_some(), res_expect_ok);
    if let Some((val, ticks)) = res_ptr_res {
        assert_eq!(*val, init_val);
        assert_eq!(ticks.added, 1);
    }
    kani::cover!(
        res_ptr_res.is_some(),
        "res_ptr_by_handle_with_uuid_checked Ok reachable"
    );
    kani::cover!(
        res_ptr_res.is_none(),
        "res_ptr_by_handle_with_uuid_checked Err reachable"
    );

    let get_mut_res = sys_ref.entity_get_mut_checked::<u64>(
        &mut raw_iter,
        0,
        0,
        FfiEntity::from_bits(1),
        Some(UuidFilter::exact(uuid_ok)),
    );
    let mut_expect_ok =
        !bad_mode_or_flag && !bad_layout && !bad_uuid && !return_null && misalign == 0;
    assert_eq!(get_mut_res.is_some(), mut_expect_ok);
    if let Some(val) = get_mut_res {
        *val = init_val.wrapping_add(1);
    }
    kani::cover!(mut_expect_ok, "entity_get_mut_checked Ok reachable");
    kani::cover!(!mut_expect_ok, "entity_get_mut_checked Err reachable");
}

/// PO-G9.4b: Proves `ValidatedCommands::insert_moved` fails closed on missing
/// `component_layout_info`, layout/UUID/VTable mismatches, null entity, and null command buffers
/// while cleanly dropping unmoved values on the guest heap.
#[kani::proof]
#[kani::unwind(21)]
fn po_g9_commands_insert_moved_guards() {
    static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

    #[repr(C)]
    struct MockDynComp {
        val: u64,
    }
    impl Drop for MockDynComp {
        fn drop(&mut self) {
            DROP_COUNT.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct MockDynVt {
        header: VTableHeader,
    }

    static DYN_UUID: ComponentUuid = ComponentUuid::from_canonical_name("a::X");
    static DYN_VT: MockDynVt = MockDynVt {
        header: VTableHeader::from_raw_parts(
            size_of::<MockDynVt>() as u32,
            DYN_UUID,
            size_of::<MockDynComp>(),
            align_of::<MockDynComp>(),
        ),
    };
    static FOREIGN_VT: MockDynVt = MockDynVt {
        header: VTableHeader::from_raw_parts(
            size_of::<MockDynVt>() as u32,
            DYN_UUID,
            size_of::<MockDynComp>(),
            align_of::<MockDynComp>(),
        ),
    };
    static DYN_SYMS: [SymbolEntry; 1] = [SymbolEntry {
        name: "bam.v0.a::X.vtable",
        ptr: (&raw const DYN_VT).cast::<c_void>(),
    }];

    impl ModComponent for MockDynComp {
        const CANONICAL_NAME: &'static str = "a::X";
        // SAFETY: Byte literal is NUL-terminated with no interior NUL bytes.
        const COMPONENT_CSTR: &'static CStr =
            unsafe { CStr::from_bytes_with_nul_unchecked(b"a::X\0") };
        const UUID: ComponentUuid = DYN_UUID;
        type VTable = MockDynVt;
        fn vtable() -> &'static Self::VTable {
            &DYN_VT
        }
        fn symbols() -> &'static [SymbolEntry] {
            &DYN_SYMS
        }
    }

    #[repr(C)]
    struct InsertSpy {
        layout_size: usize,
        layout_align: usize,
        layout_uuid: ComponentUuid,
        layout_status: FfiResult,
        vtable_kind: u8,
        insert_called: bool,
    }

    unsafe extern "C" fn mock_get_proc(
        ctx: *const BamHostCtx,
        name: *const c_char,
    ) -> *const c_void {
        // SAFETY: Test binds `ctx` to `InsertSpy`.
        let spy = unsafe { &*ctx.cast::<InsertSpy>() };
        const EXPECTED: &[u8] = b"bam.v0.a::X.vtable\0";
        let mut matches = !name.is_null();
        let mut i = 0usize;
        while matches && i < EXPECTED.len() {
            // SAFETY: `resolve_vtable_symbol_for_name` passes a NUL-terminated buffer.
            let byte = unsafe { *name.add(i) as u8 };
            if byte != EXPECTED[i] {
                matches = false;
            }
            i += 1;
        }
        if matches {
            match spy.vtable_kind {
                0 => (&raw const DYN_VT).cast::<c_void>(),
                1 => (&raw const FOREIGN_VT).cast::<c_void>(),
                _ => core::ptr::null(),
            }
        } else {
            core::ptr::null()
        }
    }

    unsafe extern "C" fn mock_layout_info(
        ctx: *const BamHostCtx,
        _comp: BamComponent,
        out: *mut BamLayoutInfo,
    ) -> FfiResult {
        // SAFETY: Test binds `ctx` to `InsertSpy`.
        let spy = unsafe { &*ctx.cast::<InsertSpy>() };
        if !spy.layout_status.is_ok() {
            return spy.layout_status;
        }
        // SAFETY: Caller supplies writable `out`.
        unsafe {
            *out = BamLayoutInfo {
                size: spy.layout_size,
                align: spy.layout_align,
                uuid: spy.layout_uuid,
                ..BamLayoutInfo::empty()
            };
        }
        FfiResult::OK
    }

    unsafe extern "C" fn mock_cmd_insert(
        cmds: *mut BamCommands,
        _e: FfiEntity,
        _c: BamComponent,
        _p: *const u8,
        _len: usize,
    ) -> FfiResult {
        // SAFETY: Test binds `cmds` to `InsertSpy`.
        let spy = unsafe { &mut *cmds.cast::<InsertSpy>() };
        spy.insert_called = true;
        FfiResult::OK
    }

    let mut spy = InsertSpy {
        layout_size: size_of::<MockDynComp>(),
        layout_align: align_of::<MockDynComp>(),
        layout_uuid: DYN_UUID,
        layout_status: FfiResult::OK,
        vtable_kind: 0,
        insert_called: false,
    };

    let mut api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    api.get_proc_address = Some(mock_get_proc);
    api.component_layout_info = Some(mock_layout_info);
    api.cmd_insert = Some(mock_cmd_insert);
    let full_host = make_valid_host(api, (&raw const spy).cast::<BamHostCtx>());

    let mut v02_api = make_core_api(BamHostApi::MIN_V0_2_SIZE as u32, ABI_VERSION);
    v02_api.get_proc_address = Some(mock_get_proc);
    v02_api.component_layout_info = Some(mock_layout_info);
    v02_api.cmd_insert = Some(mock_cmd_insert);
    let v02_host = make_valid_host(v02_api, (&raw const spy).cast::<BamHostCtx>());

    DROP_COUNT.store(0, Ordering::Relaxed);
    let mut empty_cmds = ValidatedCommands::empty(&full_host);
    assert_eq!(
        empty_cmds.insert_moved(
            FfiEntity::from_bits(1),
            BamComponent::from_raw(1),
            MockDynComp { val: 7 }
        ),
        FfiResult::NULL_POINTER
    );
    assert_eq!(DROP_COUNT.load(Ordering::Relaxed), 1);

    // SAFETY: `spy` is pinned on stack as mock `BamCommands`.
    let mut v02_cmds =
        unsafe { ValidatedCommands::from_raw((&raw mut spy).cast::<BamCommands>(), &v02_host) };
    assert_eq!(
        v02_cmds.insert_moved(
            FfiEntity::from_bits(1),
            BamComponent::from_raw(1),
            MockDynComp { val: 7 }
        ),
        FfiResult::UNSUPPORTED
    );
    assert_eq!(DROP_COUNT.load(Ordering::Relaxed), 2);
    assert!(!spy.insert_called);

    let case_idx: u8 = kani::any();
    kani::assume(case_idx < 8);
    match case_idx {
        0 => {}
        1 => spy.layout_status = FfiResult::NOT_FOUND,
        2 => spy.layout_size = size_of::<MockDynComp>() + 8,
        3 => spy.layout_align = 1,
        4 => spy.layout_uuid = ComponentUuid::from_bytes([0x99; 16]),
        5 => spy.vtable_kind = if kani::any() { 1 } else { 2 },
        6 => {}
        _ => {}
    }

    // SAFETY: `spy` is pinned on stack as mock `BamCommands`.
    let mut cmds =
        unsafe { ValidatedCommands::from_raw((&raw mut spy).cast::<BamCommands>(), &full_host) };
    let comp = if case_idx == 0 {
        BamComponent::INVALID
    } else {
        BamComponent::from_raw(1)
    };
    let entity = if case_idx == 6 {
        FfiEntity::NULL
    } else {
        FfiEntity::from_bits(1)
    };

    let drops_before = DROP_COUNT.load(Ordering::Relaxed);
    let res = cmds.insert_moved(entity, comp, MockDynComp { val: 42 });
    if case_idx == 7 {
        assert_eq!(res, FfiResult::OK);
        assert!(spy.insert_called);
        assert_eq!(DROP_COUNT.load(Ordering::Relaxed), drops_before);
    } else {
        assert!(!res.is_ok());
        assert!(!spy.insert_called);
        assert_eq!(DROP_COUNT.load(Ordering::Relaxed), drops_before + 1);
    }
    kani::cover!(res == FfiResult::OK, "insert_moved Ok reachable");
    kani::cover!(!res.is_ok(), "insert_moved Err reachable");
}

/// PO-G9.4c: Proves `ValidatedCommands::spawn_moved` fails closed on missing
/// `component_layout_info`, layout/UUID/VTable mismatches, invalid `extra` entries, and
/// uncovered `cmd_spawn_persistent` while cleanly dropping unmoved values on the guest heap.
#[kani::proof]
#[kani::unwind(18)]
fn po_g9_commands_spawn_moved_guards() {
    static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

    #[repr(C)]
    struct MockDynComp {
        val: u64,
    }
    impl Drop for MockDynComp {
        fn drop(&mut self) {
            DROP_COUNT.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct MockDynVt {
        header: VTableHeader,
    }

    static DYN_UUID: ComponentUuid = ComponentUuid::from_canonical_name("a");
    static DYN_VT: MockDynVt = MockDynVt {
        header: VTableHeader::from_raw_parts(
            size_of::<MockDynVt>() as u32,
            DYN_UUID,
            size_of::<MockDynComp>(),
            align_of::<MockDynComp>(),
        ),
    };
    static FOREIGN_VT: MockDynVt = MockDynVt {
        header: VTableHeader::from_raw_parts(
            size_of::<MockDynVt>() as u32,
            DYN_UUID,
            size_of::<MockDynComp>(),
            align_of::<MockDynComp>(),
        ),
    };
    static DYN_SYMS: [SymbolEntry; 1] = [SymbolEntry {
        name: "bam.v0.a.vtable",
        ptr: (&raw const DYN_VT).cast::<c_void>(),
    }];

    impl ModComponent for MockDynComp {
        const CANONICAL_NAME: &'static str = "a";
        // SAFETY: Byte literal is NUL-terminated with no interior NUL bytes.
        const COMPONENT_CSTR: &'static CStr =
            unsafe { CStr::from_bytes_with_nul_unchecked(b"a\0") };
        const UUID: ComponentUuid = DYN_UUID;
        type VTable = MockDynVt;
        fn vtable() -> &'static Self::VTable {
            &DYN_VT
        }
        fn symbols() -> &'static [SymbolEntry] {
            &DYN_SYMS
        }
    }

    #[repr(C)]
    struct SpawnSpy {
        layout_size: usize,
        layout_align: usize,
        layout_uuid: ComponentUuid,
        layout_status: FfiResult,
        vtable_kind: u8,
        spawn_called: bool,
        persistent_called: bool,
    }

    unsafe extern "C" fn mock_get_proc(
        ctx: *const BamHostCtx,
        name: *const c_char,
    ) -> *const c_void {
        // SAFETY: Test binds `ctx` to `SpawnSpy`.
        let spy = unsafe { &*ctx.cast::<SpawnSpy>() };
        const EXPECTED: &[u8] = b"bam.v0.a.vtable\0";
        let mut matches = !name.is_null();
        let mut i = 0usize;
        while matches && i < EXPECTED.len() {
            // SAFETY: `resolve_vtable_symbol_for_name` passes a NUL-terminated buffer.
            let byte = unsafe { *name.add(i) as u8 };
            if byte != EXPECTED[i] {
                matches = false;
            }
            i += 1;
        }
        if matches {
            match spy.vtable_kind {
                0 => (&raw const DYN_VT).cast::<c_void>(),
                1 => (&raw const FOREIGN_VT).cast::<c_void>(),
                _ => core::ptr::null(),
            }
        } else {
            core::ptr::null()
        }
    }

    unsafe extern "C" fn mock_layout_info(
        ctx: *const BamHostCtx,
        comp: BamComponent,
        out: *mut BamLayoutInfo,
    ) -> FfiResult {
        // SAFETY: Test binds `ctx` to `SpawnSpy`.
        let spy = unsafe { &*ctx.cast::<SpawnSpy>() };
        if !spy.layout_status.is_ok() {
            return spy.layout_status;
        }
        let info = if comp == BamComponent::from_raw(2) {
            BamLayoutInfo {
                size: size_of::<u32>(),
                align: align_of::<u32>(),
                uuid: ComponentUuid::from_bytes([0x77; 16]),
                ..BamLayoutInfo::empty()
            }
        } else {
            BamLayoutInfo {
                size: spy.layout_size,
                align: spy.layout_align,
                uuid: spy.layout_uuid,
                ..BamLayoutInfo::empty()
            }
        };
        // SAFETY: Caller supplies writable `out`.
        unsafe { *out = info };
        FfiResult::OK
    }

    unsafe extern "C" fn mock_cmd_spawn(
        cmds: *mut BamCommands,
        _comps: *const BamComponent,
        _ptrs: *const *const u8,
        _count: usize,
        out: *mut FfiEntity,
    ) -> FfiResult {
        // SAFETY: Test binds `cmds` to `SpawnSpy`.
        let spy = unsafe { &mut *cmds.cast::<SpawnSpy>() };
        spy.spawn_called = true;
        if !out.is_null() {
            // SAFETY: Caller supplies writable `out`.
            unsafe { *out = FfiEntity::from_bits(11) };
        }
        FfiResult::OK
    }

    unsafe extern "C" fn mock_cmd_spawn_persistent(
        cmds: *mut BamCommands,
        _comps: *const BamComponent,
        _ptrs: *const *const u8,
        _count: usize,
        out: *mut FfiEntity,
    ) -> FfiResult {
        // SAFETY: Test binds `cmds` to `SpawnSpy`.
        let spy = unsafe { &mut *cmds.cast::<SpawnSpy>() };
        spy.persistent_called = true;
        if !out.is_null() {
            // SAFETY: Caller supplies writable `out`.
            unsafe { *out = FfiEntity::from_bits(22) };
        }
        FfiResult::OK
    }

    let mut spy = SpawnSpy {
        layout_size: size_of::<MockDynComp>(),
        layout_align: align_of::<MockDynComp>(),
        layout_uuid: DYN_UUID,
        layout_status: FfiResult::OK,
        vtable_kind: 0,
        spawn_called: false,
        persistent_called: false,
    };

    let mut api = make_core_api(size_of::<BamHostApi>() as u32, ABI_VERSION);
    api.get_proc_address = Some(mock_get_proc);
    api.component_layout_info = Some(mock_layout_info);
    api.cmd_spawn = Some(mock_cmd_spawn);
    api.cmd_spawn_persistent = Some(mock_cmd_spawn_persistent);
    let full_host = make_valid_host(api, (&raw const spy).cast::<BamHostCtx>());

    let mut v02_api = make_core_api(BamHostApi::MIN_V0_2_SIZE as u32, ABI_VERSION);
    v02_api.get_proc_address = Some(mock_get_proc);
    v02_api.component_layout_info = Some(mock_layout_info);
    v02_api.cmd_spawn = Some(mock_cmd_spawn);
    v02_api.cmd_spawn_persistent = Some(mock_cmd_spawn_persistent);
    let v02_host = make_valid_host(v02_api, (&raw const spy).cast::<BamHostCtx>());

    DROP_COUNT.store(0, Ordering::Relaxed);

    let case_idx: u8 = kani::any();
    kani::assume(case_idx < 9);
    let persistent: bool = false;

    match case_idx {
        // 0: Null `ValidatedCommands`
        // 1: ABI 0.2 host (`UNSUPPORTED`)
        // 2: Invalid `BamComponent::INVALID`
        // 3: `component_layout_info` error
        3 => spy.layout_status = FfiResult::NOT_FOUND,
        // 4: Size mismatch
        4 => spy.layout_size = size_of::<MockDynComp>() + 8,
        // 5: Align mismatch
        5 => spy.layout_align = 1,
        // 6: Foreign UUID mismatch
        6 => spy.layout_uuid = ComponentUuid::from_bytes([0x99; 16]),
        // 7: Foreign/null VTable symbol pointer
        7 => spy.vtable_kind = if kani::any() { 1 } else { 2 },
        // 8: Valid `spawn_moved`
        _ => {}
    }

    let mut cmds = if case_idx == 0 {
        ValidatedCommands::empty(&full_host)
    } else if case_idx == 1 {
        // SAFETY: `spy` is pinned on stack as mock `BamCommands`.
        unsafe { ValidatedCommands::from_raw((&raw mut spy).cast::<BamCommands>(), &v02_host) }
    } else {
        // SAFETY: `spy` is pinned on stack as mock `BamCommands`.
        unsafe { ValidatedCommands::from_raw((&raw mut spy).cast::<BamCommands>(), &full_host) }
    };
    let comp = if case_idx == 2 {
        BamComponent::INVALID
    } else {
        BamComponent::from_raw(1)
    };

    let sp_res = cmds.spawn_moved(comp, MockDynComp { val: 99 }, &[], persistent);
    if case_idx == 8 {
        assert_eq!(sp_res, Ok(FfiEntity::from_bits(11)));
        assert!(spy.spawn_called);
        assert!(!spy.persistent_called);
        assert_eq!(DROP_COUNT.load(Ordering::Relaxed), 0);
    } else {
        if case_idx == 0 {
            assert_eq!(sp_res, Err(FfiResult::NULL_POINTER));
        } else if case_idx == 1 {
            assert_eq!(sp_res, Err(FfiResult::UNSUPPORTED));
        } else {
            assert!(sp_res.is_err());
        }
        assert!(!spy.spawn_called && !spy.persistent_called);
        assert_eq!(DROP_COUNT.load(Ordering::Relaxed), 1);
    }
    kani::cover!(sp_res.is_ok(), "spawn_moved Ok reachable");
    kani::cover!(sp_res.is_err(), "spawn_moved Err reachable");
}

// ============================================================================
// PO-G9.5: `ValidatedIter::entities_and_column_mut_checked` &
//          `ValidatedIter::entities_and_read_write_checked` Overlap Guards
// ============================================================================

/// PO-G9.5: Proves `ValidatedIter::entities_and_column_mut_checked` and
/// `ValidatedIter::entities_and_read_write_checked` reject hostile overlapping `(entities, read,
/// write)` host pointers and misaligned/null pointers with `None` strictly before forming
/// aliasing `&[FfiEntity]`, `&[R]`, or `&mut [W]` slices.
#[kani::proof]
#[kani::unwind(6)]
fn po_g9_iter_entities_and_column_overlap_guards() {
    #[repr(C)]
    struct OverlapState {
        // 8 * u64 = 64 bytes, 8-byte aligned (matching `FfiEntity` and `u64`).
        buf: [u64; 8],
        e_off: usize,
        r_off: usize,
        w_off: usize,
        e_misalign: usize,
        r_misalign: usize,
        w_misalign: usize,
        chunk_len: usize,
        same_comp: bool,
    }

    unsafe extern "C" fn mock_next(iter: *mut BamIter, out_len: *mut usize) -> FfiBool {
        // SAFETY: Test binds `iter` to `OverlapState`.
        let st = unsafe { &*iter.cast::<OverlapState>() };
        // SAFETY: Caller supplies writable `out_len`.
        unsafe { *out_len = st.chunk_len };
        FfiBool::TRUE
    }

    unsafe extern "C" fn mock_entities(iter: *const BamIter) -> *const FfiEntity {
        // SAFETY: Test binds `iter` to `OverlapState`.
        let st = unsafe { &*iter.cast::<OverlapState>() };
        // SAFETY: `st.e_off + st.chunk_len <= 8` in this harness; `e_misalign` stays inside `buf`.
        unsafe {
            st.buf
                .as_ptr()
                .add(st.e_off)
                .cast::<u8>()
                .add(st.e_misalign)
                .cast::<FfiEntity>()
        }
    }

    unsafe extern "C" fn mock_info(
        iter: *const BamIter,
        term: usize,
        out: *mut BamColumnInfo,
    ) -> FfiResult {
        // SAFETY: Test binds `iter` to `OverlapState`.
        let st = unsafe { &*iter.cast::<OverlapState>() };
        let info = match term {
            0 => BamColumnInfo {
                component: BamComponent::from_raw(1),
                mode: AccessMode::READ,
                uuid: ComponentUuid::from_bytes([1; 16]),
                size: size_of::<u64>(),
                align: align_of::<u64>(),
                storage_flags: 0,
                reserved0: 0,
            },
            1 => BamColumnInfo {
                component: BamComponent::from_raw(if st.same_comp { 1 } else { 2 }),
                mode: AccessMode::WRITE,
                uuid: ComponentUuid::from_bytes([2; 16]),
                size: size_of::<u64>(),
                align: align_of::<u64>(),
                storage_flags: 0,
                reserved0: 0,
            },
            _ => return FfiResult::OUT_OF_RANGE,
        };
        // SAFETY: Caller supplies writable `out`.
        unsafe { *out = info };
        FfiResult::OK
    }

    unsafe extern "C" fn mock_ro(iter: *const BamIter, _term: usize) -> *const u8 {
        // SAFETY: Test binds `iter` to `OverlapState`.
        let st = unsafe { &*iter.cast::<OverlapState>() };
        // SAFETY: `st.r_off + st.chunk_len <= 8` in this harness; `r_misalign` stays inside `buf`.
        unsafe {
            st.buf
                .as_ptr()
                .add(st.r_off)
                .cast::<u8>()
                .add(st.r_misalign)
        }
    }

    unsafe extern "C" fn mock_mut(iter: *mut BamIter, _term: usize) -> *mut u8 {
        // SAFETY: Test binds `iter` to `OverlapState`.
        let st = unsafe { &mut *iter.cast::<OverlapState>() };
        // SAFETY: `st.w_off + st.chunk_len <= 8` in this harness; `w_misalign` stays inside `buf`.
        unsafe {
            st.buf
                .as_mut_ptr()
                .add(st.w_off)
                .cast::<u8>()
                .add(st.w_misalign)
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

    let e_off = 0usize;
    let r_off: usize = kani::any();
    let w_off: usize = kani::any();
    kani::assume(r_off <= 5 && w_off <= 5);
    let e_misalign: usize = kani::any();
    let r_misalign: usize = kani::any();
    let w_misalign: usize = kani::any();
    kani::assume(e_misalign <= 1 && r_misalign <= 1 && w_misalign <= 1);
    let same_comp: bool = kani::any();

    let mut state = OverlapState {
        buf: [10, 20, 30, 40, 50, 60, 70, 80],
        e_off,
        r_off,
        w_off,
        e_misalign,
        r_misalign,
        w_misalign,
        chunk_len: 2,
        same_comp,
    };

    // SAFETY: `state` stays pinned on the stack across `iter`.
    let mut iter = unsafe { ValidatedIter::from_raw((&raw mut state).cast::<BamIter>(), &host) };
    assert_eq!(iter.next_chunk(), Some(2));

    // Byte intervals in `buf`:
    let e_start = e_off * 8 + e_misalign;
    let e_end = e_start + 16;
    let r_start = r_off * 8 + r_misalign;
    let r_end = r_start + 16;
    let w_start = w_off * 8 + w_misalign;
    let w_end = w_start + 16;

    let ew_overlap = e_start < w_end && w_start < e_end;
    let rw_overlap = r_start < w_end && w_start < r_end;

    // 1. `entities_and_column_mut_checked`
    let ew_res = iter.entities_and_column_mut_checked::<u64>(1, None);
    let ew_expected_ok = e_misalign == 0 && w_misalign == 0 && !ew_overlap;
    assert_eq!(ew_res.is_some(), ew_expected_ok);
    if let Some((entities, writes)) = ew_res {
        assert_eq!(entities.len(), 2);
        assert_eq!(writes.len(), 2);
        writes[0] = writes[0].wrapping_add(1);
    }
    kani::cover!(
        ew_expected_ok,
        "entities_and_column_mut_checked disjoint Ok reachable"
    );
    kani::cover!(
        !ew_expected_ok,
        "entities_and_column_mut_checked overlap/misaligned Err reachable"
    );

    // 2. Same term index in `entities_and_read_write_checked` must return `None`.
    assert!(
        iter.entities_and_read_write_checked::<u64, u64>(1, None, 1, None)
            .is_none()
    );

    // 3. `entities_and_read_write_checked`
    let erw_res = iter.entities_and_read_write_checked::<u64, u64>(0, None, 1, None);
    let erw_expected_ok = !same_comp
        && e_misalign == 0
        && r_misalign == 0
        && w_misalign == 0
        && !ew_overlap
        && !rw_overlap;
    assert_eq!(erw_res.is_some(), erw_expected_ok);
    if let Some((entities, reads, writes)) = erw_res {
        assert_eq!(entities.len(), 2);
        assert_eq!(reads.len(), 2);
        assert_eq!(writes.len(), 2);
        writes[0] = reads[0].wrapping_add(1);
        writes[1] = reads[1].wrapping_add(1);
    }
    kani::cover!(
        erw_expected_ok,
        "entities_and_read_write_checked disjoint Ok reachable"
    );
    kani::cover!(
        erw_expected_ok && r_off == 0,
        "entities_and_read_write_checked shared e/r overlap disjoint from w Ok reachable"
    );
    kani::cover!(
        !erw_expected_ok && (ew_overlap || rw_overlap),
        "entities_and_read_write_checked hostile overlap rejection reachable"
    );
}
