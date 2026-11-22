//! Soundness regression test suite verifying audit findings (S1–S13) in `bamabi_guest_sys`.

mod common;

use core::{
    ffi::{CStr, c_char, c_void},
    sync::atomic::{AtomicUsize, Ordering},
};

use bamabi_guest_sys::*;
use bamabi_sys::{
    AccessMode, BAM_STORAGE_FLAG_IMMUTABLE, BAM_STORAGE_FLAG_MESSAGE, BAM_STORAGE_FLAG_RESOURCE,
    BAM_STORAGE_FLAG_SPARSE_SET, BamChangeTicks, BamColumnInfo, BamCommands, BamComponent,
    BamHostCtx, BamInstance, BamIter, BamLayoutInfo, BamLoadCtx, BamManifest, BamSystemCtx,
    BamSystemDesc, BamTermDesc, ComponentUuid, FfiBool, FfiEntity, FfiResult, FfiSlice,
    MappedWireVTable, ModComponent, ScheduleId, StrView, SymbolEntry, VTableHeader,
};
use common::make_mock_api;

// ============================================================================
// S1 & S3: Layout & UUID validation in handle-based resource lookups
// ============================================================================

/// Verifies fail-closed layout, alignment, immutability, and UUID validation for handle-based
/// singleton resource lookups (`res_ptr_by_handle*`, `read/write_mapped_wire_resource_by_handle`).
///
/// Scenario:
/// 1. When `component_layout_info` is absent (ABI `0.2`), all handle-based lookups fail closed.
/// 2. When `component_layout_info` is present, oversized reads (`[u8; 4096]` vs `u32`), wrong
///    alignment (`[u8; 4]` vs `u32`), wrong UUIDs, immutable mutable borrows, and mismatched wire
///    VTable handles are rejected while exact matches succeed.
#[test]
fn s1_and_s3_resource_handle_validation_fails_closed() {
    /// Mock host state storing a 4-byte `u32` resource and a 16-byte wire-bridged `[u64; 2]`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct WireHostState {
        small_res: u32,
        wire_res: [u64; 2],
    }

    static SMALL_UUID: ComponentUuid = ComponentUuid::from_parts("s1::SmallU32", 4, 4, 1);
    static WIRE_UUID: ComponentUuid = ComponentUuid::from_parts("s3::WireHost", 16, 8, 2);
    static IMMUT_UUID: ComponentUuid = ComponentUuid::from_parts("s1::ImmutU32", 4, 4, 3);

    /// Mock wire getter summing `[u64; 2]` into a `u64`.
    unsafe extern "C" fn wire_get(host_ptr: *const c_void, out_wire: *mut u64) -> FfiResult {
        // SAFETY: Test passes aligned `[u64; 2]` and writable `u64`.
        unsafe {
            let pair = &*host_ptr.cast::<[u64; 2]>();
            *out_wire = pair[0] + pair[1];
        }
        FfiResult::OK
    }

    /// Mock wire setter updating `pair[0]`.
    unsafe extern "C" fn wire_set(host_ptr: *mut c_void, value: *const u64) -> FfiResult {
        // SAFETY: Test passes aligned `[u64; 2]` and readable `u64`.
        unsafe {
            let pair = &mut *host_ptr.cast::<[u64; 2]>();
            pair[0] = *value;
        }
        FfiResult::OK
    }

    static WIRE_VTABLE: MappedWireVTable<u64> = MappedWireVTable {
        header: VTableHeader::from_raw_parts(
            core::mem::size_of::<MappedWireVTable<u64>>() as u32,
            WIRE_UUID,
            16,
            8,
        ),
        get: Some(wire_get),
        set: Some(wire_set),
    };

    /// Mock `get_proc_address` exporting `WIRE_VTABLE`.
    unsafe extern "C" fn mock_get_proc(
        _ctx: *const BamHostCtx,
        name: *const c_char,
    ) -> *const c_void {
        // SAFETY: Valid CStr from test.
        if unsafe { CStr::from_ptr(name) }.to_bytes() == b"bam.v0.s3::WireHost.vtable" {
            (&raw const WIRE_VTABLE).cast::<c_void>()
        } else {
            core::ptr::null()
        }
    }

    /// Mock `component_layout_info` describing handles 1 (`SmallU32`), 2 (`WireHost`), 3 (`ImmutU32`).
    unsafe extern "C" fn mock_layout_info(
        _ctx: *const BamHostCtx,
        comp: BamComponent,
        out: *mut BamLayoutInfo,
    ) -> FfiResult {
        let info = match comp.0 {
            1 => BamLayoutInfo {
                flags: BAM_STORAGE_FLAG_RESOURCE,
                uuid: SMALL_UUID,
                size: 4,
                align: 4,
                ..BamLayoutInfo::empty()
            },
            2 => BamLayoutInfo {
                flags: BAM_STORAGE_FLAG_RESOURCE,
                uuid: WIRE_UUID,
                size: 16,
                align: 8,
                ..BamLayoutInfo::empty()
            },
            3 => BamLayoutInfo {
                flags: BAM_STORAGE_FLAG_RESOURCE | BAM_STORAGE_FLAG_IMMUTABLE,
                uuid: IMMUT_UUID,
                size: 4,
                align: 4,
                ..BamLayoutInfo::empty()
            },
            _ => return FfiResult::INVALID_COMPONENT,
        };
        // SAFETY: `out` is writable stack storage.
        unsafe { *out = info };
        FfiResult::OK
    }

    /// Mock `sys_res_ptr` returning shared pointers into `WireHostState`.
    unsafe extern "C" fn mock_sys_res_ptr(
        sys_ctx: *const BamSystemCtx,
        comp: BamComponent,
        out_ticks: *mut BamChangeTicks,
    ) -> *const u8 {
        // SAFETY: `sys_ctx` points to `WireHostState`.
        let st = unsafe { &*sys_ctx.cast::<WireHostState>() };
        if !out_ticks.is_null() {
            // SAFETY: `out_ticks` is writable.
            unsafe { *out_ticks = BamChangeTicks::default() };
        }
        match comp.0 {
            1 | 3 => (&raw const st.small_res).cast::<u8>(),
            2 => (&raw const st.wire_res).cast::<u8>(),
            _ => core::ptr::null(),
        }
    }

    /// Mock `sys_res_mut_ptr` returning mutable pointers into `WireHostState`.
    unsafe extern "C" fn mock_sys_res_mut_ptr(
        sys_ctx: *mut BamSystemCtx,
        comp: BamComponent,
        out_ticks: *mut BamChangeTicks,
    ) -> *mut u8 {
        // SAFETY: `sys_ctx` points to `WireHostState`.
        let st = unsafe { &mut *sys_ctx.cast::<WireHostState>() };
        if !out_ticks.is_null() {
            // SAFETY: `out_ticks` is writable.
            unsafe { *out_ticks = BamChangeTicks::default() };
        }
        match comp.0 {
            1 | 3 => (&raw mut st.small_res).cast::<u8>(),
            2 => (&raw mut st.wire_res).cast::<u8>(),
            _ => core::ptr::null_mut(),
        }
    }

    let mut state = WireHostState {
        small_res: 77,
        wire_res: [10, 20],
    };

    // 1. Without `component_layout_info` (ABI 0.2 host): MUST fail closed!
    let mut api_v02 = make_mock_api();
    api_v02.get_proc_address = Some(mock_get_proc);
    api_v02.sys_res_ptr = Some(mock_sys_res_ptr);
    api_v02.sys_res_mut_ptr = Some(mock_sys_res_mut_ptr);
    api_v02.component_layout_info = None;

    // SAFETY: Stack pointers valid for test.
    let host_v02 = unsafe { Host::from_raw(&api_v02, (&raw mut state).cast()) }.unwrap();
    let vtable = host_v02
        .resolve_mapped_wire_vtable::<u64>(c"bam.v0.s3::WireHost.vtable", WIRE_UUID)
        .unwrap();
    // SAFETY: `state` acts as `BamSystemCtx`.
    let mut inv_v02 = unsafe {
        ValidatedSystemInvocation::from_raw((&raw mut state).cast(), &host_v02, 0, 0, None)
    };
    let sr_v02 = inv_v02.system_ref_mut().unwrap();
    assert!(
        sr_v02
            .res_ptr_by_handle_checked::<u32>(BamComponent::from_raw(1))
            .is_none()
    );
    assert!(
        sr_v02
            .res_mut_ptr_by_handle_checked::<u32>(BamComponent::from_raw(1))
            .is_none()
    );
    assert_eq!(
        sr_v02.read_mapped_wire_resource_by_handle(BamComponent::from_raw(2), &vtable),
        Err(FfiResult::UNSUPPORTED)
    );
    assert_eq!(
        sr_v02.write_mapped_wire_resource_by_handle(BamComponent::from_raw(2), &vtable, &99),
        Err(FfiResult::UNSUPPORTED)
    );

    // 2. With `component_layout_info`: matching types succeed; oversized/mismatched/immutable fail!
    let mut api_v03 = api_v02;
    api_v03.component_layout_info = Some(mock_layout_info);
    // SAFETY: Stack pointers valid for test.
    let host_v03 = unsafe { Host::from_raw(&api_v03, (&raw mut state).cast()) }.unwrap();
    // SAFETY: `state` acts as `BamSystemCtx`.
    let mut inv_v03 = unsafe {
        ValidatedSystemInvocation::from_raw((&raw mut state).cast(), &host_v03, 0, 0, None)
    };
    let sr = inv_v03.system_ref_mut().unwrap();

    // S1: Oversized read/write against a 4-byte `u32` handle MUST fail cleanly (no OOB read/write!).
    assert!(
        sr.res_ptr_by_handle_checked::<[u8; 4096]>(BamComponent::from_raw(1))
            .is_none()
    );
    assert!(
        sr.res_mut_ptr_by_handle_checked::<[u8; 4096]>(BamComponent::from_raw(1))
            .is_none()
    );
    // Wrong alignment (`[u8; 4]` has align 1 vs registered align 4) MUST fail!
    assert!(
        sr.res_ptr_by_handle_checked::<[u8; 4]>(BamComponent::from_raw(1))
            .is_none()
    );
    // Wrong UUID filter MUST fail!
    assert!(
        sr.res_ptr_by_handle_with_uuid_checked::<u32>(
            BamComponent::from_raw(1),
            Some(UuidFilter::exact(WIRE_UUID))
        )
        .is_none()
    );
    // Immutable handle rejects mutable borrow!
    assert!(
        sr.res_ptr_by_handle_checked::<u32>(BamComponent::from_raw(3))
            .is_some()
    );
    assert!(
        sr.res_mut_ptr_by_handle_checked::<u32>(BamComponent::from_raw(3))
            .is_none()
    );
    // Exact layout + UUID succeeds!
    assert_eq!(
        *sr.res_ptr_by_handle_with_uuid_checked::<u32>(
            BamComponent::from_raw(1),
            Some(UuidFilter::exact(SMALL_UUID))
        )
        .unwrap()
        .0,
        77
    );
    *sr.res_mut_ptr_by_handle_checked::<u32>(BamComponent::from_raw(1))
        .unwrap()
        .0 = 88;
    assert_eq!(
        *sr.res_ptr_by_handle_checked::<u32>(BamComponent::from_raw(1))
            .unwrap()
            .0,
        88
    );

    // S3: Passing unrelated handle `1` (4B u32) to a 16B `WireHost` vtable MUST fail with TYPE_MISMATCH!
    assert_eq!(
        sr.read_mapped_wire_resource_by_handle(BamComponent::from_raw(1), &vtable),
        Err(FfiResult::TYPE_MISMATCH)
    );
    assert_eq!(
        sr.write_mapped_wire_resource_by_handle(BamComponent::from_raw(1), &vtable, &55),
        Err(FfiResult::TYPE_MISMATCH)
    );
    // Passing matching handle `2` succeeds!
    assert_eq!(
        sr.read_mapped_wire_resource_by_handle(BamComponent::from_raw(2), &vtable)
            .unwrap()
            .0,
        30
    );
    assert!(
        sr.write_mapped_wire_resource_by_handle(BamComponent::from_raw(2), &vtable, &40)
            .is_ok()
    );
    assert_eq!(
        sr.read_mapped_wire_resource_by_handle(BamComponent::from_raw(2), &vtable)
            .unwrap()
            .0,
        60
    );
}

// ============================================================================
// S2: `ValidatedSystemRef` uniqueness (non-Copy/non-Clone & single take)
// ============================================================================

/// Verifies affine single-extraction semantics for [`ValidatedSystemRef`], query slots, and
/// [`ValidatedCommands`] on [`ValidatedSystemInvocation`], preventing duplicate `&mut` handles.
#[test]
fn s2_validated_system_ref_and_slots_cannot_be_duplicated() {
    let api = make_mock_api();
    let mut dummy = 0u64;
    // SAFETY: Stack pointers valid for test.
    let host = unsafe { Host::from_raw(&api, (&raw mut dummy).cast()) }.unwrap();
    // SAFETY: `dummy` acts as opaque non-null `BamSystemCtx`.
    let mut inv =
        unsafe { ValidatedSystemInvocation::from_raw((&raw mut dummy).cast(), &host, 1, 1, None) };

    assert!(inv.system_ref().is_some());
    let unique_ref = inv.take_system_ref().expect("first take succeeds");
    // Second take MUST return None — no duplicate `ValidatedSystemRef` handles!
    assert!(inv.take_system_ref().is_none());
    assert!(inv.system_ref().is_none());
    assert!(inv.system_ref_mut().is_none());
    let _ = unique_ref;

    // Query slot and commands cannot be extracted twice either!
    assert!(inv.take_raw_query_slot(0).is_some());
    assert!(inv.take_raw_query_slot(0).is_none());
    let c1 = inv.take_commands();
    assert!(c1.is_valid());
    let c2 = inv.take_commands();
    assert!(!c2.is_valid());
    // `commands_mut()` after take must return disabled empty buffer without panicking!
    assert!(!inv.commands_mut().is_valid());
}

// ============================================================================
// S4: `insert_moved`, `spawn_moved`, `SpawnEntry`, `SpawnBatchColumn` validation
// ============================================================================

/// Verifies fail-closed layout and VTable validation across [`SpawnEntry`], [`SpawnBatchColumn`],
/// `insert_moved`, and `spawn_moved`, ensuring rejected moves drop guest-owned values cleanly.
///
/// Covers undersized entries, non-ZST markers, foreign UUIDs with matching `(size, align)`,
/// invalid extra entries in `spawn_moved`, and ABI `0.2` hosts lacking `component_layout_info`.
#[test]
fn s4_spawn_and_moved_commands_validate_component_layout_and_vtable() {
    static DROPS: AtomicUsize = AtomicUsize::new(0);

    /// Non-POD heap-owning component fixture with drop tracking.
    #[derive(Default)]
    struct OwnedDyn {
        payload: String,
    }
    impl Drop for OwnedDyn {
        /// Increments `DROPS` when reclaimed on error or shutdown.
        fn drop(&mut self) {
            let _ = self.payload.len();
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Header-only VTable for `OwnedDyn`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct OwnedDynVt {
        header: VTableHeader,
    }
    static OWNED_UUID: ComponentUuid = ComponentUuid::from_canonical_name("s4::OwnedDyn");
    static OWNED_VT: OwnedDynVt = OwnedDynVt {
        header: VTableHeader::from_raw_parts(
            core::mem::size_of::<OwnedDynVt>() as u32,
            OWNED_UUID,
            core::mem::size_of::<OwnedDyn>(),
            core::mem::align_of::<OwnedDyn>(),
        ),
    };
    static OWNED_SYMS: [SymbolEntry; 1] = [SymbolEntry {
        name: "bam.v0.s4::OwnedDyn.vtable",
        ptr: (&raw const OWNED_VT).cast::<c_void>(),
    }];
    impl ModComponent for OwnedDyn {
        const CANONICAL_NAME: &'static str = "s4::OwnedDyn";
        const COMPONENT_CSTR: &'static CStr = c"s4::OwnedDyn";
        const UUID: ComponentUuid = OWNED_UUID;
        type VTable = OwnedDynVt;
        /// Returns `OWNED_VT`.
        fn vtable() -> &'static Self::VTable {
            &OWNED_VT
        }
        /// Returns `OWNED_SYMS`.
        fn symbols() -> &'static [SymbolEntry] {
            &OWNED_SYMS
        }
    }

    /// Mock `get_proc_address` exporting `OWNED_VT`.
    unsafe extern "C" fn s4_get_proc(
        _ctx: *const BamHostCtx,
        name: *const c_char,
    ) -> *const c_void {
        // SAFETY: Valid CStr from test.
        if unsafe { CStr::from_ptr(name) }.to_bytes() == b"bam.v0.s4::OwnedDyn.vtable" {
            (&raw const OWNED_VT).cast::<c_void>()
        } else {
            core::ptr::null()
        }
    }

    /// Mock `component_layout_info` defining 16B POD, ZST marker, same-mod `OwnedDyn`, and foreign UUID.
    unsafe extern "C" fn s4_layout_info(
        _ctx: *const BamHostCtx,
        comp: BamComponent,
        out: *mut BamLayoutInfo,
    ) -> FfiResult {
        let info = match comp.0 {
            // Handle 1: 16-byte POD component (`[u64; 2]`)
            1 => BamLayoutInfo {
                size: 16,
                align: 8,
                uuid: ComponentUuid::from_parts("s4::BigPod", 16, 8, 0),
                ..BamLayoutInfo::empty()
            },
            // Handle 2: ZST marker (`size = 0, align = 1`)
            2 => BamLayoutInfo {
                size: 0,
                align: 1,
                uuid: ComponentUuid::from_parts("s4::Tag", 0, 1, 0),
                ..BamLayoutInfo::empty()
            },
            // Handle 3: Same-mod `OwnedDyn`
            3 => BamLayoutInfo {
                size: core::mem::size_of::<OwnedDyn>(),
                align: core::mem::align_of::<OwnedDyn>(),
                uuid: OWNED_UUID,
                ..BamLayoutInfo::empty()
            },
            // Handle 4: Foreign component with same size/align as `OwnedDyn`, wrong UUID!
            4 => BamLayoutInfo {
                size: core::mem::size_of::<OwnedDyn>(),
                align: core::mem::align_of::<OwnedDyn>(),
                uuid: ComponentUuid::from_canonical_name("foreign::OtherType"),
                ..BamLayoutInfo::empty()
            },
            _ => return FfiResult::INVALID_COMPONENT,
        };
        // SAFETY: `out` is writable.
        unsafe { *out = info };
        FfiResult::OK
    }

    /// Mock `cmd_spawn` / `cmd_spawn_persistent` returning entity `99`.
    unsafe extern "C" fn s4_spawn_noop(
        _cmds: *mut BamCommands,
        _comps: *const BamComponent,
        _ptrs: *const *const u8,
        _count: usize,
        out_entity: *mut FfiEntity,
    ) -> FfiResult {
        // SAFETY: `out_entity` is writable.
        unsafe { *out_entity = FfiEntity::from_bits(99) };
        FfiResult::OK
    }

    /// Mock `cmd_spawn_batch` populating sequential entity IDs `1..=entity_count`.
    unsafe extern "C" fn s4_spawn_batch_noop(
        _cmds: *mut BamCommands,
        _comps: *const BamComponent,
        _ptrs: *const *const u8,
        _comp_count: usize,
        entity_count: usize,
        out_entities: *mut FfiEntity,
    ) -> FfiResult {
        if !out_entities.is_null() {
            for i in 0..entity_count {
                // SAFETY: Caller provides `entity_count` writable `FfiEntity` slots.
                unsafe { *out_entities.add(i) = FfiEntity::from_bits((i + 1) as u64) };
            }
        }
        FfiResult::OK
    }

    /// Mock `cmd_insert` no-op returning `OK`.
    unsafe extern "C" fn s4_insert_noop(
        _cmds: *mut BamCommands,
        _e: FfiEntity,
        _c: BamComponent,
        _p: *const u8,
        _l: usize,
    ) -> FfiResult {
        FfiResult::OK
    }

    let mut dummy = 0u64;
    let mut api = make_mock_api();
    api.get_proc_address = Some(s4_get_proc);
    api.component_layout_info = Some(s4_layout_info);
    api.cmd_spawn = Some(s4_spawn_noop);
    api.cmd_spawn_persistent = Some(s4_spawn_noop);
    api.cmd_spawn_batch = Some(s4_spawn_batch_noop);
    api.cmd_insert = Some(s4_insert_noop);

    // SAFETY: Stack pointers valid for test.
    let host = unsafe { Host::from_raw(&api, (&raw mut dummy).cast()) }.unwrap();
    // SAFETY: `dummy` acts as opaque non-null `BamCommands`.
    let mut cmds = unsafe { ValidatedCommands::from_raw((&raw mut dummy).cast(), &host) };

    // Undersized `SpawnEntry::new(big_16b_handle, &0u8)` MUST fail with TYPE_MISMATCH!
    let tiny = 0u8;
    assert_eq!(
        cmds.spawn(&[SpawnEntry::new(BamComponent::from_raw(1), &tiny)]),
        Err(FfiResult::TYPE_MISMATCH)
    );
    assert_eq!(
        cmds.spawn_persistent(&[SpawnEntry::new(BamComponent::from_raw(1), &tiny)]),
        Err(FfiResult::TYPE_MISMATCH)
    );
    // `SpawnEntry::marker(non_zst_handle)` MUST fail with TYPE_MISMATCH!
    assert_eq!(
        cmds.spawn(&[SpawnEntry::marker(BamComponent::from_raw(1))]),
        Err(FfiResult::TYPE_MISMATCH)
    );
    // Matching `SpawnEntry` + `marker(zst)` succeed!
    let big = [1u64, 2u64];
    assert!(
        cmds.spawn(&[
            SpawnEntry::new(BamComponent::from_raw(1), &big),
            SpawnEntry::marker(BamComponent::from_raw(2)),
        ])
        .is_ok()
    );

    // Undersized `SpawnBatchColumn::new(big_16b_handle, &[0u8; 2])` MUST fail with TYPE_MISMATCH!
    let bad_col = [0u8, 1u8];
    assert_eq!(
        cmds.spawn_batch_columns(
            &[SpawnBatchColumn::new(BamComponent::from_raw(1), &bad_col)],
            2,
            None
        ),
        FfiResult::TYPE_MISMATCH
    );
    // `SpawnBatchColumn::marker(non_zst_handle, 2)` MUST fail with TYPE_MISMATCH!
    assert_eq!(
        cmds.spawn_batch_columns(
            &[SpawnBatchColumn::marker(BamComponent::from_raw(1), 2)],
            2,
            None
        ),
        FfiResult::TYPE_MISMATCH
    );
    // Matching batch column succeeds!
    let good_col = [[1u64, 2u64], [3u64, 4u64]];
    assert_eq!(
        cmds.spawn_batch_columns(
            &[
                SpawnBatchColumn::new(BamComponent::from_raw(1), &good_col),
                SpawnBatchColumn::marker(BamComponent::from_raw(2), 2),
            ],
            2,
            None,
        ),
        FfiResult::OK
    );

    // `insert_moved` / `spawn_moved` with wrong handle (handle 4: wrong UUID) MUST fail AND drop `OwnedDyn`!
    DROPS.store(0, Ordering::SeqCst);
    assert_eq!(
        cmds.insert_moved(
            FfiEntity::from_bits(1),
            BamComponent::from_raw(4),
            OwnedDyn {
                payload: "wrong_uuid_insert".into()
            },
        ),
        FfiResult::TYPE_MISMATCH
    );
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);

    assert_eq!(
        cmds.spawn_moved(
            BamComponent::from_raw(4),
            OwnedDyn {
                payload: "wrong_uuid_spawn".into()
            },
            &[],
            false,
        ),
        Err(FfiResult::TYPE_MISMATCH)
    );
    assert_eq!(DROPS.load(Ordering::SeqCst), 2);

    // `spawn_moved` with valid primary handle 3 but undersized `extra` entry MUST reject AND drop!
    assert_eq!(
        cmds.spawn_moved(
            BamComponent::from_raw(3),
            OwnedDyn {
                payload: "bad_extra_spawn".into()
            },
            &[SpawnEntry::new(BamComponent::from_raw(1), &tiny)],
            false,
        ),
        Err(FfiResult::TYPE_MISMATCH)
    );
    assert_eq!(DROPS.load(Ordering::SeqCst), 3);

    // Missing `component_layout_info` (ABI 0.2 host) MUST fail closed for safe `SpawnEntry` & `insert_moved`!
    let mut api_v02 = api;
    api_v02.component_layout_info = None;
    // SAFETY: Stack pointers valid for test.
    let host_v02 = unsafe { Host::from_raw(&api_v02, (&raw mut dummy).cast()) }.unwrap();
    // SAFETY: `dummy` acts as `BamCommands`.
    let mut cmds_v02 = unsafe { ValidatedCommands::from_raw((&raw mut dummy).cast(), &host_v02) };
    assert_eq!(
        cmds_v02.spawn(&[SpawnEntry::new(BamComponent::from_raw(1), &big)]),
        Err(FfiResult::UNSUPPORTED)
    );
    assert_eq!(
        cmds_v02.insert_moved(
            FfiEntity::from_bits(1),
            BamComponent::from_raw(3),
            OwnedDyn {
                payload: "v02_fail_closed".into()
            },
        ),
        FfiResult::UNSUPPORTED
    );
    assert_eq!(DROPS.load(Ordering::SeqCst), 4);
}

// ============================================================================
// S5 & S6: Non-POD message UUID bypass rejection + null/panic-guarded callbacks
// ============================================================================

/// Verifies that non-POD message read/send enforces `M::UUID` even when `expected_uuid == None`
/// and that witness sink callbacks (`forward_field`, `append_field`) reject null or misaligned
/// context pointers with `INVALID_ARGUMENT` without dereferencing them.
#[test]
fn s5_and_s6_non_pod_message_uuid_enforced_and_callbacks_panic_and_null_safe() {
    /// Primary non-POD message fixture.
    #[derive(Default, Debug, PartialEq, Eq)]
    struct MsgA;
    /// Conflicting non-POD message fixture on term `1`.
    #[derive(Default, Debug, PartialEq, Eq)]
    struct MsgB;

    /// Mock `export_fields` deliberately probing null and misaligned `sink_ctx` before calling real `sink_ctx`.
    unsafe extern "C" fn export_tests_null_and_misaligned_ctx(
        _src_ptr: *const c_void,
        sink: Option<
            unsafe extern "C" fn(
                sink_ctx: *mut c_void,
                field_name: StrView,
                type_tag: StrView,
                payload: FfiSlice<u8>,
            ) -> FfiResult,
        >,
        sink_ctx: *mut c_void,
    ) -> FfiResult {
        let sink_fn = sink.unwrap();
        // 1. Passing null `ctx` must return INVALID_ARGUMENT (not UB deref!).
        // SAFETY: Testing null-pointer guard inside `forward_field` / `append_field`.
        let null_rc = unsafe {
            sink_fn(
                core::ptr::null_mut(),
                StrView::from_str("a"),
                StrView::from_str("u32"),
                FfiSlice::from_slice(&[1, 2, 3, 4]),
            )
        };
        assert_eq!(null_rc, FfiResult::INVALID_ARGUMENT);
        // 2. Passing a misaligned `ctx` pointer must also return INVALID_ARGUMENT cleanly!
        let mut aligned_base = [0u64; 4];
        // SAFETY: `aligned_base` is 8-byte aligned with 32 bytes, so `+ 1` byte is strictly misaligned.
        let misaligned = unsafe { aligned_base.as_mut_ptr().cast::<u8>().add(1) }.cast::<c_void>();
        // SAFETY: Testing alignment guard inside `forward_field` / `append_field`.
        let misaligned_rc = unsafe {
            sink_fn(
                misaligned,
                StrView::from_str("a"),
                StrView::from_str("u32"),
                FfiSlice::from_slice(&[1, 2, 3, 4]),
            )
        };
        assert_eq!(misaligned_rc, FfiResult::INVALID_ARGUMENT);
        // 3. Call real `sink_ctx`.
        // SAFETY: `sink_ctx` is caller-provided context.
        unsafe {
            sink_fn(
                sink_ctx,
                StrView::from_str("a"),
                StrView::from_str("u32"),
                FfiSlice::from_slice(&[1, 2, 3, 4]),
            )
        }
    }

    /// No-op `import_field` stub returning `OK`.
    unsafe extern "C" fn import_ok(
        _dst: *mut c_void,
        _name: StrView,
        _tag: StrView,
        _payload: FfiSlice<u8>,
    ) -> FfiResult {
        FfiResult::OK
    }

    static SYMS_A: [SymbolEntry; 2] = [
        SymbolEntry {
            name: "bam.v0.s5::MsgA.export_fields",
            ptr: export_tests_null_and_misaligned_ctx as *const c_void,
        },
        SymbolEntry {
            name: "bam.v0.s5::MsgA.import_field",
            ptr: import_ok as *const c_void,
        },
    ];
    impl ModComponent for MsgA {
        const CANONICAL_NAME: &'static str = "s5::MsgA";
        const COMPONENT_CSTR: &'static CStr = c"s5::MsgA";
        const UUID: ComponentUuid = ComponentUuid::from_canonical_name("s5::MsgA");
        type VTable = ();
        /// Returns unit VTable.
        fn vtable() -> &'static Self::VTable {
            &()
        }
        /// Returns `SYMS_A`.
        fn symbols() -> &'static [SymbolEntry] {
            &SYMS_A
        }
    }
    impl ModComponent for MsgB {
        const CANONICAL_NAME: &'static str = "s5::MsgB";
        const COMPONENT_CSTR: &'static CStr = c"s5::MsgB";
        const UUID: ComponentUuid = ComponentUuid::from_canonical_name("s5::MsgB");
        type VTable = ();
        /// Returns unit VTable.
        fn vtable() -> &'static Self::VTable {
            &()
        }
        /// Returns empty symbols.
        fn symbols() -> &'static [SymbolEntry] {
            &[]
        }
    }

    /// Mock `get_proc_address` exporting `MsgA` witness and `MsgA`/`MsgB` non-POD markers.
    unsafe extern "C" fn s5_get_proc(_ctx: *const BamHostCtx, sym: *const c_char) -> *const c_void {
        // SAFETY: Valid CStr.
        match unsafe { CStr::from_ptr(sym) }.to_bytes() {
            b"bam.v0.s5::MsgA.export_fields" => {
                export_tests_null_and_misaligned_ctx as *const c_void
            }
            b"bam.v0.s5::MsgA.non_pod_message" | b"bam.v0.s5::MsgB.non_pod_message" => {
                s5_read_batch as *const c_void
            }
            _ => core::ptr::null(),
        }
    }

    /// Mock `iter_column_info` mapping term `0` to `MsgA` and term `1` to `MsgB`.
    unsafe extern "C" fn s5_col_info(
        _iter: *const BamIter,
        term_index: usize,
        out_info: *mut BamColumnInfo,
    ) -> FfiResult {
        let uuid = match term_index {
            0 => MsgA::UUID,
            1 => MsgB::UUID,
            _ => return FfiResult::OUT_OF_RANGE,
        };
        // SAFETY: `out_info` is writable.
        unsafe {
            *out_info = BamColumnInfo {
                component: BamComponent::from_raw((term_index + 1) as u32),
                uuid,
                size: core::mem::size_of::<*const c_void>(),
                align: core::mem::align_of::<*const c_void>(),
                mode: AccessMode::MESSAGE_WRITE,
                storage_flags: BAM_STORAGE_FLAG_MESSAGE,
                reserved0: 0,
            };
        }
        FfiResult::OK
    }

    static DUMMY_MSG: u64 = 42;

    /// Mock `msg_read_batch` returning one dummy message pointer.
    unsafe extern "C" fn s5_read_batch(
        _iter: *mut BamIter,
        _term_index: usize,
        out_buf: *mut u8,
        max_count: usize,
        out_count: *mut usize,
    ) -> FfiResult {
        if max_count == 0 {
            return FfiResult::OK;
        }
        let ptr = (&raw const DUMMY_MSG).cast::<c_void>();
        // SAFETY: `out_buf` holds `max_count` pointer slots; `out_count` is writable.
        #[allow(clippy::cast_ptr_alignment)]
        unsafe {
            core::ptr::write_unaligned(out_buf.cast::<*const c_void>(), ptr);
            *out_count = 1;
        }
        FfiResult::OK
    }

    /// Mock `msg_send` accepting any serialized packet.
    unsafe extern "C" fn s5_send(
        _iter: *mut BamIter,
        _term_index: usize,
        _data_ptr: *const u8,
        _data_len: usize,
    ) -> FfiResult {
        FfiResult::OK
    }

    let mut api = make_mock_api();
    api.get_proc_address = Some(s5_get_proc);
    api.iter_column_info = Some(s5_col_info);
    api.msg_read_batch = Some(s5_read_batch);
    api.msg_send = Some(s5_send);

    let mut dummy = 0u64;
    // SAFETY: Stack pointers valid for test.
    let host = unsafe { Host::from_raw(&api, (&raw mut dummy).cast()) }.unwrap();
    // SAFETY: `dummy` acts as `BamIter`.
    let mut iter = unsafe { ValidatedIter::from_raw((&raw mut dummy).cast(), &host) };

    // S5: Passing `expected_uuid = None` for `MsgA` on term 1 (`MsgB`) MUST NOT skip UUID validation!
    assert_eq!(
        iter.read_non_pod_messages_checked::<MsgA>(1, None, 4),
        Err(FfiResult::TYPE_MISMATCH)
    );
    assert_eq!(
        iter.send_non_pod_message_checked::<MsgA>(1, None, &MsgA),
        FfiResult::TYPE_MISMATCH
    );
    // Even passing an explicit wrong `UuidFilter::exact(MsgB::UUID)` for `MsgA` MUST fail!
    assert_eq!(
        iter.read_non_pod_messages_checked::<MsgA>(1, Some(UuidFilter::exact(MsgB::UUID)), 4),
        Err(FfiResult::TYPE_MISMATCH)
    );

    // S6: Calling `read_non_pod_messages_checked::<MsgA>` and `send_non_pod_message_checked::<MsgA>`
    // on term 0 exercises null & misaligned `ctx` guards inside `forward_field` and `append_field`.
    assert_eq!(
        iter.read_non_pod_messages_checked::<MsgA>(0, None, 1),
        Ok(vec![MsgA])
    );
    assert_eq!(
        iter.send_non_pod_message_checked::<MsgA>(0, None, &MsgA),
        FfiResult::OK
    );
}

// ============================================================================
// S7: Stacked/Tree Borrows soundness of `GuestInstance` & `prepare_system_desc`
// ============================================================================

/// Verifies Miri Stacked/Tree Borrows soundness of `OwnedDescriptorStorage` raw pointers inside
/// `prepare_system_desc` and `OwnedRuntimeAllocs` after `GuestInstance` is moved into a `Box`.
#[test]
fn s7_owned_runtime_allocs_and_system_descriptor_pointers_sound_under_miri() {
    /// Mock `register_system` dereferencing every string and term pointer inside `BamSystemDesc`.
    unsafe extern "C" fn inspect_desc_register_system(
        _load_ctx: *mut BamLoadCtx,
        desc: *const BamSystemDesc,
    ) -> FfiResult {
        // SAFETY: Host inspects every descriptor string and term pointer passed by `prepare_system_desc`!
        let d = unsafe { &*desc };
        let st = d.validate();
        if !st.is_ok() {
            return st;
        }
        // Dereference `name`, `queries[i].terms`, `after`, `before`, `in_sets`, `after_weak`, `before_weak`, `ambiguous_with`.
        // Under Miri Stacked Borrows, any Box-move retagging invalidation triggers immediate UB here!
        // SAFETY: Valid pointers produced by `prepare_system_desc`.
        unsafe {
            assert_eq!(CStr::from_ptr(d.name).to_str().unwrap(), "s7_sys");
            let queries = core::slice::from_raw_parts(d.queries, d.query_count);
            assert_eq!(queries.len(), 2);
            let t0 = core::slice::from_raw_parts(queries[0].terms, queries[0].term_count);
            assert_eq!(t0.len(), 1);
            let t1 = core::slice::from_raw_parts(queries[1].terms, queries[1].term_count);
            assert_eq!(t1.len(), 1);
            for (ptr, count, expected) in [
                (d.after, d.after_count, "set_after"),
                (d.before, d.before_count, "set_before"),
                (d.in_sets, d.in_set_count, "set_in"),
                (d.after_weak, d.after_weak_count, "set_after_w"),
                (d.before_weak, d.before_weak_count, "set_before_w"),
                (d.ambiguous_with, d.ambiguous_with_count, "set_ambig"),
            ] {
                let slice = core::slice::from_raw_parts(ptr, count);
                assert_eq!(CStr::from_ptr(slice[0]).to_str().unwrap(), expected);
            }
        }
        FfiResult::OK
    }

    /// Mock `cmd_observe` returning entity `123`.
    unsafe extern "C" fn mock_cmd_observe(
        _cmds: *mut BamCommands,
        desc: *const bamabi_sys::BamObserverDesc,
        out: *mut FfiEntity,
    ) -> FfiResult {
        // SAFETY: Valid test pointers.
        unsafe {
            assert!((*desc).validate().is_ok());
            *out = FfiEntity::from_bits(123);
        }
        FfiResult::OK
    }

    let mut api = make_mock_api();
    api.register_system = Some(inspect_desc_register_system);
    api.cmd_observe = Some(mock_cmd_observe);

    let mut dummy = 0u64;
    let manifest = BamManifest::new(c"s7_mod".as_ptr(), c"0.3.0".as_ptr());
    let mut raw_inst: *mut BamInstance = core::ptr::null_mut();

    // SAFETY: Stack pointers valid for test.
    let rc = unsafe {
        init_guest_instance(
            &api,
            (&raw mut dummy).cast::<BamLoadCtx>(),
            &mut raw_inst,
            &manifest,
            |inst, load| {
                let q_terms = [BamTermDesc::new(
                    BamComponent::from_raw(1),
                    AccessMode::READ,
                )];
                let r_terms = [BamTermDesc::new(
                    BamComponent::from_raw(2),
                    AccessMode::RESOURCE_READ,
                )];
                let cfg = SystemRegistrationConfig {
                    name: c"s7_sys",
                    user_queries: &[&q_terms],
                    system_resources: &r_terms,
                    schedule: ScheduleId::UPDATE,
                    flags: 0,
                    after: &[c"set_after"],
                    before: &[c"set_before"],
                    in_sets: &[c"set_in"],
                    after_weak: &[c"set_after_w"],
                    before_weak: &[c"set_before_w"],
                    ambiguous_with: &[c"set_ambig"],
                    ..Default::default()
                };
                load.register_system(inst, &cfg, None, |_| FfiResult::OK)
            },
        )
    };
    assert_eq!(rc, FfiResult::OK);
    assert!(!raw_inst.is_null());

    // After `GuestInstance` has been moved into `Box::new(instance)` inside `init_guest_instance`,
    // track a runtime observer allocation via `Host::track_runtime_allocation`!
    // SAFETY: `raw_inst` is a live `GuestInstance` until `shutdown_guest_instance`.
    let host_after_move = unsafe { *(*raw_inst.cast::<GuestInstance>()).host() };
    // SAFETY: `dummy` acts as `BamCommands`.
    let mut cmds =
        unsafe { ValidatedCommands::from_raw((&raw mut dummy).cast(), &host_after_move) };
    let obs_e = cmds
        .observe::<u32, _>(
            Some("post_move_runtime_obs"),
            BamComponent::from_raw(1),
            FfiEntity::NULL,
            0,
            BamComponent::INVALID,
            |_, _, _| FfiResult::OK,
        )
        .unwrap();
    assert_eq!(obs_e.to_bits(), 123);

    // SAFETY: `raw_inst` came from `init_guest_instance`.
    unsafe { shutdown_guest_instance(raw_inst) };
}

// ============================================================================
// S8: `stride == 0` wildcard rejection, `isize::MAX`, sparse alignment, & overlap
// ============================================================================

/// Verifies five boundary-hardening guards: (1) `stride == 0` rejects non-ZST slice casts,
/// (2) `SparseRows`/`SparseRowsMut` reject `len * ptr_size > isize::MAX`, (3) `ChunkColumnDispenser`
/// rejects `isize::MAX` overflows, (4) misaligned sparse row pointer arrays return `TYPE_MISMATCH`,
/// and (5) `iter_entities_and_dynamic_slice_mut` rejects overlapping entity and component buffers.
#[test]
fn s8_stride_zero_isize_max_sparse_alignment_and_overlap_guards() {
    // 1. `ValidatedSlice` / `ValidatedSliceMut` with `stride == 0` MUST reject non-ZST types!
    let mut buf = [10u64, 20u64];
    let raw_ptr = buf.as_mut_ptr().cast::<u8>();
    // SAFETY: Testing `stride == 0` rejection before any non-ZST read.
    let s_ro = unsafe { ValidatedSlice::from_raw_parts(raw_ptr, 2, 0) };
    assert!(s_ro.as_typed_slice::<u64>().is_none());
    assert!(s_ro.get_ptr(0, 8).is_none());
    assert_eq!(s_ro.as_typed_slice::<[u8; 0]>().unwrap().len(), 2);

    // SAFETY: Testing `stride == 0` rejection on mutable slice.
    let mut s_mut = unsafe { ValidatedSliceMut::from_raw_parts(raw_ptr, 2, 0) };
    assert!(s_mut.as_typed_slice::<u64>().is_none());
    assert!(s_mut.as_typed_slice_mut::<u64>().is_none());
    assert!(s_mut.get_mut_ptr(0, 8).is_none());
    assert_eq!(s_mut.as_typed_slice_mut::<[u8; 0]>().unwrap().len(), 2);
    assert!(s_mut.into_typed_slice_mut_checked::<u64>(None).is_none());

    // 2. `SparseRows` / `SparseRowsMut` reject `len * size_of::<*const u8>() > isize::MAX`!
    let huge_len = (isize::MAX as usize / core::mem::size_of::<*const u8>()) + 1;
    let dummy_ro: *const *const u8 = core::ptr::NonNull::dangling().as_ptr();
    let dummy_mut: *const *mut u8 = core::ptr::NonNull::dangling().as_ptr();
    // SAFETY: Rejected on `isize::MAX` guard before dereferencing `ptrs`.
    assert!(unsafe { SparseRows::<u32>::from_raw_parts(dummy_ro, huge_len, true) }.is_none());
    // SAFETY: Rejected on `isize::MAX` guard before dereferencing `ptrs`.
    assert!(unsafe { SparseRowsMut::<u32>::from_raw_parts(dummy_mut, huge_len, true) }.is_none());

    // 3. `ChunkColumnDispenser` rejects `len * size > isize::MAX` on `entities`, `next_read`, `next_write`!
    /// Adversarial mock iterator state serving huge chunk lengths, overlapping buffers, and misaligned sparse arrays.
    struct HugeChunkIter {
        len: usize,
        buf: [u64; 2],
        aligned_sparse_storage: [u64; 4],
    }

    /// Mock `iter_next_chunk` reporting `HugeChunkIter::len`.
    unsafe extern "C" fn s8_next_chunk(iter: *mut BamIter, out_len: *mut usize) -> FfiBool {
        // SAFETY: `iter` points to `HugeChunkIter`.
        let st = unsafe { &*iter.cast::<HugeChunkIter>() };
        // SAFETY: `out_len` is writable.
        unsafe { *out_len = st.len };
        FfiBool::TRUE
    }
    /// Mock `iter_entities` returning `HugeChunkIter::buf`.
    unsafe extern "C" fn s8_entities(iter: *const BamIter) -> *const FfiEntity {
        // SAFETY: `iter` points to `HugeChunkIter`.
        let st = unsafe { &*iter.cast::<HugeChunkIter>() };
        st.buf.as_ptr().cast::<FfiEntity>()
    }
    /// Mock `iter_column_readonly` returning `HugeChunkIter::buf`.
    unsafe extern "C" fn s8_col_ro(iter: *const BamIter, _idx: usize) -> *const u8 {
        // SAFETY: `iter` points to `HugeChunkIter`.
        let st = unsafe { &*iter.cast::<HugeChunkIter>() };
        st.buf.as_ptr().cast::<u8>()
    }
    /// Mock `iter_column_mutable` deliberately aliasing `HugeChunkIter::buf` with `entities`.
    unsafe extern "C" fn s8_col_mut(iter: *mut BamIter, _idx: usize) -> *mut u8 {
        // SAFETY: `iter` points to `HugeChunkIter` (deliberately overlaps `entities`!).
        let st = unsafe { &mut *iter.cast::<HugeChunkIter>() };
        st.buf.as_mut_ptr().cast::<u8>()
    }
    /// Mock `iter_column_rows_readonly` returning a deliberately 1-byte misaligned pointer array.
    unsafe extern "C" fn s8_rows_ro(iter: *mut BamIter, _idx: usize) -> *const *const u8 {
        // SAFETY: `iter` points to `HugeChunkIter`.
        let st = unsafe { &*iter.cast::<HugeChunkIter>() };
        // Intentionally return a 1-byte misaligned `*const *const u8` from an 8-byte aligned base.
        #[allow(clippy::cast_ptr_alignment)]
        // SAFETY: `aligned_sparse_storage` is 8-byte aligned with 32 bytes so `+ 1` byte stays in bounds and is guaranteed misaligned.
        let unaligned =
            unsafe { st.aligned_sparse_storage.as_ptr().cast::<u8>().add(1) }.cast::<*const u8>();
        unaligned
    }
    /// Mock `iter_column_info` describing dense read/write, sparse wire, and overlapping dynamic columns.
    unsafe extern "C" fn s8_col_info(
        _iter: *const BamIter,
        term_index: usize,
        out: *mut BamColumnInfo,
    ) -> FfiResult {
        let info = match term_index {
            0 => BamColumnInfo {
                component: BamComponent::from_raw(1),
                uuid: ComponentUuid::from_parts("s8::U64", 8, 8, 0),
                size: 8,
                align: 8,
                mode: AccessMode::READ,
                storage_flags: 0,
                reserved0: 0,
            },
            1 => BamColumnInfo {
                component: BamComponent::from_raw(2),
                uuid: ComponentUuid::from_parts("s8::U64Mut", 8, 8, 0),
                size: 8,
                align: 8,
                mode: AccessMode::WRITE,
                storage_flags: 0,
                reserved0: 0,
            },
            2 => BamColumnInfo {
                component: BamComponent::from_raw(3),
                uuid: ComponentUuid::from_parts("s8::SparseWire", 8, 8, 0),
                size: 8,
                align: 8,
                mode: AccessMode::READ,
                storage_flags: BAM_STORAGE_FLAG_SPARSE_SET,
                reserved0: 0,
            },
            3 => BamColumnInfo {
                component: BamComponent::from_raw(4),
                uuid: OverlapDyn::UUID,
                size: core::mem::size_of::<OverlapDyn>(),
                align: core::mem::align_of::<OverlapDyn>(),
                mode: AccessMode::WRITE,
                storage_flags: 0,
                reserved0: 0,
            },
            _ => return FfiResult::OUT_OF_RANGE,
        };
        // SAFETY: `out` is writable.
        unsafe { *out = info };
        FfiResult::OK
    }

    /// Same-mod dynamic component fixture used to test entity/column overlap rejection.
    #[repr(C)]
    struct OverlapDyn {
        val: u64,
    }
    /// Header-only VTable for `OverlapDyn`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct OverlapDynVt {
        header: VTableHeader,
    }
    static OVERLAP_UUID: ComponentUuid = ComponentUuid::from_canonical_name("s8::OverlapDyn");
    static OVERLAP_VT: OverlapDynVt = OverlapDynVt {
        header: VTableHeader::from_raw_parts(
            core::mem::size_of::<OverlapDynVt>() as u32,
            OVERLAP_UUID,
            8,
            8,
        ),
    };
    static OVERLAP_SYMS: [SymbolEntry; 1] = [SymbolEntry {
        name: "bam.v0.s8::OverlapDyn.vtable",
        ptr: (&raw const OVERLAP_VT).cast::<c_void>(),
    }];
    impl ModComponent for OverlapDyn {
        const CANONICAL_NAME: &'static str = "s8::OverlapDyn";
        const COMPONENT_CSTR: &'static CStr = c"s8::OverlapDyn";
        const UUID: ComponentUuid = OVERLAP_UUID;
        type VTable = OverlapDynVt;
        /// Returns `OVERLAP_VT`.
        fn vtable() -> &'static Self::VTable {
            &OVERLAP_VT
        }
        /// Returns `OVERLAP_SYMS`.
        fn symbols() -> &'static [SymbolEntry] {
            &OVERLAP_SYMS
        }
    }

    /// Mock wire getter returning `1`.
    unsafe extern "C" fn s8_wire_get(_p: *const c_void, out: *mut u64) -> FfiResult {
        // SAFETY: Writable `u64`.
        unsafe { *out = 1 };
        FfiResult::OK
    }
    static S8_SPARSE_WIRE_VT: MappedWireVTable<u64> = MappedWireVTable {
        header: VTableHeader::from_raw_parts(
            core::mem::size_of::<MappedWireVTable<u64>>() as u32,
            ComponentUuid::from_parts("s8::SparseWire", 8, 8, 0),
            8,
            8,
        ),
        get: Some(s8_wire_get),
        set: None,
    };
    /// Mock `get_proc_address` exporting `OVERLAP_VT` and `S8_SPARSE_WIRE_VT`.
    unsafe extern "C" fn s8_get_proc(_ctx: *const BamHostCtx, sym: *const c_char) -> *const c_void {
        // SAFETY: Valid CStr.
        match unsafe { CStr::from_ptr(sym) }.to_bytes() {
            b"bam.v0.s8::OverlapDyn.vtable" => (&raw const OVERLAP_VT).cast::<c_void>(),
            b"bam.v0.s8::SparseWire.vtable" => (&raw const S8_SPARSE_WIRE_VT).cast::<c_void>(),
            _ => core::ptr::null(),
        }
    }

    let mut api = make_mock_api();
    api.get_proc_address = Some(s8_get_proc);
    api.iter_next_chunk = Some(s8_next_chunk);
    api.iter_entities = Some(s8_entities);
    api.iter_column_readonly = Some(s8_col_ro);
    api.iter_column_mutable = Some(s8_col_mut);
    api.iter_column_rows_readonly = Some(s8_rows_ro);
    api.iter_column_info = Some(s8_col_info);

    let mut state = HugeChunkIter {
        len: (isize::MAX as usize / 8) + 1,
        buf: [1, 2],
        aligned_sparse_storage: [0u64; 4],
    };
    // SAFETY: Stack pointers valid for test.
    let host = unsafe { Host::from_raw(&api, (&raw mut state).cast()) }.unwrap();
    // SAFETY: `state` acts as `BamIter`.
    let mut iter = unsafe { ValidatedIter::from_raw((&raw mut state).cast(), &host) };
    assert!(iter.next_chunk().is_some());

    // Dispenser rejects `isize::MAX` overflow cleanly on `entities`, `next_read`, and `next_write`!
    {
        let mut disp = ChunkColumnDispenser::new(&mut iter);
        assert_eq!(disp.entities(), Err(FfiResult::OUT_OF_RANGE));
        assert_eq!(
            disp.next_read::<u64>(AccessMode::READ, None),
            Err(FfiResult::OUT_OF_RANGE)
        );
        assert_eq!(disp.next_write::<u64>(None), Err(FfiResult::OUT_OF_RANGE));
    }

    // 4. Misaligned sparse pointer array in `read_mapped_wire_row` MUST return TYPE_MISMATCH!
    let wire_vt = host
        .resolve_mapped_wire_vtable::<u64>(
            c"bam.v0.s8::SparseWire.vtable",
            ComponentUuid::from_parts("s8::SparseWire", 8, 8, 0),
        )
        .unwrap();
    state.len = 2;
    // SAFETY: `state` is a valid `HugeChunkIter` matching the mock API callbacks.
    let mut iter2 = unsafe { ValidatedIter::from_raw((&raw mut state).cast(), &host) };
    assert!(iter2.next_chunk().is_some());
    assert_eq!(
        iter2.read_mapped_wire_row(2, 0, &wire_vt),
        Err(FfiResult::TYPE_MISMATCH)
    );

    // 5. `iter_entities_and_dynamic_slice_mut` rejects overlapping `entities` and component bytes!
    assert!(iter_entities_and_dynamic_slice_mut::<OverlapDyn>(&mut iter2, 3).is_none());
}

// ============================================================================
// S9: ChunkColumnDispenser overlapping write-then-read (Tree Borrows soundness)
// ============================================================================

/// Scenario: Verifies that `ChunkColumnDispenser` rejects an overlapping write-then-read borrow on
/// the same raw address range (`ACCESS_CONFLICT`) while allowing repeated `entities()` calls without
/// violating Rust aliasing rules.
#[test]
fn s9_dispenser_overlapping_write_then_read_and_repeat_entities_are_sound() {
    #[repr(C)]
    struct OverlapHostIter {
        buf: [u32; 4],
        ent_a: [FfiEntity; 2],
        ent_b: [FfiEntity; 2],
        ent_calls: usize,
        yielded: bool,
    }

    unsafe extern "C" fn s9_next_chunk(iter: *mut BamIter, out_len: *mut usize) -> FfiBool {
        // SAFETY: Test binds `iter` to `OverlapHostIter`.
        let st = unsafe { &mut *iter.cast::<OverlapHostIter>() };
        if st.yielded {
            return FfiBool::FALSE;
        }
        st.yielded = true;
        // SAFETY: `out_len` is writable stack storage.
        unsafe { *out_len = 2 };
        FfiBool::TRUE
    }

    unsafe extern "C" fn s9_entities(iter: *const BamIter) -> *const FfiEntity {
        // SAFETY: Test binds `iter` to `OverlapHostIter` with interior call counter mutation.
        let st = unsafe { &mut *iter.cast_mut().cast::<OverlapHostIter>() };
        st.ent_calls += 1;
        if st.ent_calls == 1 {
            st.ent_a.as_ptr()
        } else {
            st.ent_b.as_ptr()
        }
    }

    unsafe extern "C" fn s9_col_info(
        _: *const BamIter,
        term: usize,
        out: *mut BamColumnInfo,
    ) -> FfiResult {
        let info = match term {
            0 => BamColumnInfo {
                component: BamComponent::from_raw(1),
                mode: AccessMode::WRITE,
                uuid: ComponentUuid::from_parts("s9::WriteCol", 4, 4, 0),
                size: 4,
                align: 4,
                storage_flags: 0,
                reserved0: 0,
            },
            1 => BamColumnInfo {
                component: BamComponent::from_raw(2),
                mode: AccessMode::READ,
                uuid: ComponentUuid::from_parts("s9::ReadCol", 4, 4, 0),
                size: 4,
                align: 4,
                storage_flags: 0,
                reserved0: 0,
            },
            _ => return FfiResult::OUT_OF_RANGE,
        };
        // SAFETY: `out` is writable stack storage.
        unsafe { *out = info };
        FfiResult::OK
    }

    unsafe extern "C" fn s9_col_mut(iter: *mut BamIter, term: usize) -> *mut u8 {
        if term != 0 {
            return core::ptr::null_mut();
        }
        // SAFETY: Test binds `iter` to `OverlapHostIter`.
        let st = unsafe { &mut *iter.cast::<OverlapHostIter>() };
        st.buf.as_mut_ptr().cast::<u8>()
    }

    unsafe extern "C" fn s9_col_ro(iter: *const BamIter, term: usize) -> *const u8 {
        if term != 1 {
            return core::ptr::null();
        }
        // SAFETY: Deliberately return an overlapping pointer (`buf[1..3]` overlaps `buf[0..2]`).
        // Project directly through raw pointers so we do not retag `&OverlapHostIter` over the
        // live mutable borrow `w: &mut [u32]` of `buf[0..2]` under Stacked Borrows.
        unsafe {
            core::ptr::addr_of!((*iter.cast::<OverlapHostIter>()).buf)
                .cast::<u32>()
                .add(1)
                .cast::<u8>()
        }
    }

    let mut api = make_mock_api();
    api.iter_next_chunk = Some(s9_next_chunk);
    api.iter_entities = Some(s9_entities);
    api.iter_column_info = Some(s9_col_info);
    api.iter_column_mutable = Some(s9_col_mut);
    api.iter_column_readonly = Some(s9_col_ro);

    let mut state = OverlapHostIter {
        buf: [10, 20, 30, 40],
        ent_a: [FfiEntity::from_bits(1), FfiEntity::from_bits(2)],
        ent_b: [FfiEntity::from_bits(3), FfiEntity::from_bits(4)],
        ent_calls: 0,
        yielded: false,
    };
    // SAFETY: Stack allocations live across `host` and `iter`.
    let host = unsafe { Host::from_raw(&api, (&raw const state).cast()) }.unwrap();
    // SAFETY: `state` acts as `BamIter`.
    let mut iter = unsafe { ValidatedIter::from_raw((&raw mut state).cast(), &host) };
    assert_eq!(iter.next_chunk(), Some(2));

    let mut disp = ChunkColumnDispenser::new(&mut iter);
    let w = disp.next_write::<u32>(None).unwrap();
    w[0] = 111;
    // Overlapping read MUST fail with ACCESS_CONFLICT BEFORE forming `&[u32]`, so subsequent
    // writes through `w` remain Tree-Borrows-sound!
    assert_eq!(
        disp.next_read::<u32>(AccessMode::READ, None),
        Err(FfiResult::ACCESS_CONFLICT)
    );
    w[1] = 222;
    assert_eq!(w, &[111, 222]);

    // First `entities()` succeeds; a hostile second `entities()` returning a different range fails!
    assert_eq!(
        disp.entities().unwrap(),
        &[FfiEntity::from_bits(1), FfiEntity::from_bits(2)]
    );
    assert_eq!(disp.entities(), Err(FfiResult::ACCESS_CONFLICT));
}

// ============================================================================
// S10: State restore `isize::MAX`, re-entrant mutable observer Tree Borrows,
//      overlapping `pod_clone_trampoline`, and bridged row bounds under Miri
// ============================================================================

/// Verifies under Miri (`-Zmiri-tree-borrows -Zmiri-strict-provenance`) that:
/// 1. `guest_restore_state_trampoline` rejects `len > isize::MAX` (`OUT_OF_RANGE`) and misaligned
///    `instance` pointers (`INVALID_ARGUMENT`) without UB.
/// 2. Re-entrant `register_observer_mut_closure` invocation with the SAME live `event_data` and
///    `propagate` pointers rejects with `ACCESS_CONFLICT` BEFORE forming an aliasing `&mut E` or
///    clobbering `*propagate`.
/// 3. `pod_clone_trampoline` safely handles identical and partially overlapping `(src, dst)` ranges.
/// 4. Bridged dense rows reject `current_len * host_size > isize::MAX` (`OUT_OF_RANGE`) and
///    non-power-of-two `host_align` (`TYPE_MISMATCH`).
#[test]
fn s10_restore_state_observer_mut_reentrancy_pod_clone_overlap_and_bridged_bounds() {
    use bamabi_guest_sys::{
        GuestInstance, guest_restore_state_trampoline, guest_save_state_trampoline,
        pod_clone_trampoline, register_observer_mut_closure,
    };
    use bamabi_sys::{BamInstance, BamObserverDesc, BamObserverMutFn, FfiBool};
    use core::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    // 1. `guest_restore_state_trampoline` & `guest_save_state_trampoline` bounds/alignment checks
    let api = make_mock_api();
    let mut dummy_ctx = 0u64;
    // SAFETY: Stack pointers valid for test.
    let host = unsafe { Host::from_raw(&api, (&raw const dummy_ctx).cast()) }.unwrap();
    let mut inst = Box::new(GuestInstance::new(host, "s10_mod"));
    let called = Arc::new(AtomicBool::new(false));
    let called_clone = Arc::clone(&called);
    inst.set_restore_state_hook(move |_bytes| {
        called_clone.store(true, Ordering::SeqCst);
        FfiResult::OK
    });
    let raw_inst = Box::into_raw(inst).cast::<BamInstance>();
    let byte = 0x5Au8;
    assert_eq!(
        // SAFETY: `len > isize::MAX` must fail closed with `OUT_OF_RANGE` before forming a slice!
        unsafe {
            guest_restore_state_trampoline(raw_inst, &raw const byte, (isize::MAX as usize) + 1)
        },
        FfiResult::OUT_OF_RANGE
    );
    assert!(!called.load(Ordering::SeqCst));
    // Misaligned non-null `instance` must fail closed without dereferencing.
    let mut align_buf = [0u64; 4];
    // SAFETY: Adding 1 byte produces a strictly misaligned pointer inside `align_buf`.
    let misaligned_inst =
        unsafe { align_buf.as_mut_ptr().cast::<u8>().add(1) }.cast::<BamInstance>();
    assert_eq!(
        // SAFETY: Alignment guard rejects `misaligned_inst` before dereferencing.
        unsafe { guest_restore_state_trampoline(misaligned_inst, &raw const byte, 1) },
        FfiResult::INVALID_ARGUMENT
    );
    unsafe extern "C" fn dummy_writer(_: *mut c_void, _: *const u8, _: usize) -> FfiResult {
        FfiResult::OK
    }
    assert_eq!(
        // SAFETY: Alignment guard rejects `misaligned_inst` before dereferencing.
        unsafe {
            guest_save_state_trampoline(misaligned_inst, Some(dummy_writer), core::ptr::null_mut())
        },
        FfiResult::INVALID_ARGUMENT
    );
    // Cleanly reclaim `raw_inst`.
    // SAFETY: Reclaiming the `GuestInstance` boxed above.
    unsafe { bamabi_guest_sys::shutdown_guest_instance(raw_inst) };

    // 2. Re-entrant mutable observer (`observer_mut_closure_trampoline`) Tree Borrows soundness!
    #[repr(C)]
    struct CaptureObsCtx {
        mut_cb: Option<BamObserverMutFn>,
        user_data: *mut c_void,
    }
    unsafe extern "C" fn capture_register_obs(
        load_ctx: *mut BamLoadCtx,
        desc: *const BamObserverDesc,
    ) -> FfiResult {
        // SAFETY: Test binds `load_ctx` to `CaptureObsCtx` and `desc` is valid.
        let cap = unsafe { &mut *load_ctx.cast::<CaptureObsCtx>() };
        // SAFETY: `desc` is valid.
        let d = unsafe { &*desc };
        cap.mut_cb = d.mut_callback;
        cap.user_data = d.user_data;
        FfiResult::OK
    }
    let mut obs_api = make_mock_api();
    obs_api.register_observer = Some(capture_register_obs);
    // SAFETY: Stack pointers valid for test.
    let obs_host = unsafe { Host::from_raw(&obs_api, (&raw const dummy_ctx).cast()) }.unwrap();
    let mut obs_inst = GuestInstance::new(obs_host, "s10_obs");
    let inner_rc_slot = Arc::new(Mutex::new(FfiResult::OK));
    let inner_rc_clone = Arc::clone(&inner_rc_slot);

    // Raw-pointer holder preserving full Miri strict provenance across the re-entrant call.
    struct ProvenanceBundle {
        cb: Option<BamObserverMutFn>,
        ud: *mut c_void,
        ev: *mut u8,
        prop: *mut FfiBool,
    }
    // SAFETY: Used strictly on the single test thread.
    unsafe impl Send for ProvenanceBundle {}
    let bundle = Arc::new(Mutex::new(ProvenanceBundle {
        cb: None,
        ud: core::ptr::null_mut(),
        ev: core::ptr::null_mut(),
        prop: core::ptr::null_mut(),
    }));
    let bundle_clone = Arc::clone(&bundle);
    let mut cap2 = CaptureObsCtx {
        mut_cb: None,
        user_data: core::ptr::null_mut(),
    };
    fn hrtb_mut_obs<
        F: FnMut(FfiEntity, &mut u64, &mut bool, &mut ValidatedCommands<'_>) -> FfiResult
            + Send
            + 'static,
    >(
        f: F,
    ) -> F {
        f
    }
    let mut_obs_closure = hrtb_mut_obs(move |target, ev, prop, _cmds| {
        *ev = 100;
        *prop = false;
        let (cb, ud, ev_ptr, prop_ptr) = {
            let g = bundle_clone.lock().unwrap();
            (g.cb.unwrap(), g.ud, g.ev, g.prop)
        };
        // Re-enter with the EXACT live `ev_ptr` and `prop_ptr` while `&mut *ev` is active!
        // Under `-Zmiri-tree-borrows`, forming `&mut *ev_ptr` before `try_with_mut` is UB!
        // SAFETY: Re-entrant call testing `ReentrantCell` mutual exclusion.
        let nested = unsafe { cb(target, ev_ptr, prop_ptr, core::ptr::null_mut(), ud) };
        *inner_rc_clone.lock().unwrap() = nested;
        // Must still be valid to mutate `*ev` after the rejected re-entry!
        *ev += 23;
        FfiResult::OK
    });
    // SAFETY: `cap2` acts as `BamLoadCtx`.
    let reg_rc2 = unsafe {
        register_observer_mut_closure::<u64, _>(
            &mut obs_inst,
            (&raw mut cap2).cast::<BamLoadCtx>(),
            "s10_mut_obs_reenter",
            BamComponent::from_raw(1),
            FfiEntity::NULL,
            0,
            BamComponent::INVALID,
            mut_obs_closure,
        )
    };
    assert_eq!(reg_rc2, FfiResult::OK);
    let mut event_val: u64 = 1;
    let mut prop_val = FfiBool::TRUE;
    {
        let mut g = bundle.lock().unwrap();
        g.cb = cap2.mut_cb;
        g.ud = cap2.user_data;
        g.ev = (&raw mut event_val).cast::<u8>();
        g.prop = &raw mut prop_val;
    }
    let cb = cap2.mut_cb.unwrap();
    // SAFETY: Pointers live on stack and `cap2.user_data` lives in `obs_inst`.
    let outer_rc = unsafe {
        cb(
            FfiEntity::from_bits(7),
            (&raw mut event_val).cast::<u8>(),
            &raw mut prop_val,
            core::ptr::null_mut(),
            cap2.user_data,
        )
    };
    assert_eq!(outer_rc, FfiResult::OK);
    assert_eq!(*inner_rc_slot.lock().unwrap(), FfiResult::ACCESS_CONFLICT);
    assert_eq!(event_val, 123);
    assert!(!prop_val.get());

    // 3. `pod_clone_trampoline` overlapping and identical buffers under Miri
    let mut overlap_buf: [u8; 12] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
    let overlap_base = overlap_buf.as_mut_ptr();
    assert_eq!(
        // SAFETY: Identical `src == dst` must be UB-free (`core::ptr::copy`).
        unsafe {
            pod_clone_trampoline::<u64>(
                overlap_base.cast(),
                overlap_base.cast(),
                core::ptr::null_mut(),
            )
        },
        FfiResult::OK
    );
    assert_eq!(
        // SAFETY: Partial overlap `[0..8)` -> `[4..12)` inside `overlap_buf[0..12]`.
        unsafe {
            pod_clone_trampoline::<u64>(
                overlap_base.cast(),
                overlap_base.add(4).cast(),
                core::ptr::null_mut(),
            )
        },
        FfiResult::OK
    );
    assert_eq!(&overlap_buf[4..12], &[1, 2, 3, 4, 5, 6, 7, 8]);

    // 4. Bridged row `current_len * host_size > isize::MAX` and non-power-of-two `host_align`
    unsafe extern "C" fn s10_wire_get(_: *const c_void, out: *mut u64) -> FfiResult {
        // SAFETY: Writable stack `out`.
        unsafe { *out = 9 };
        FfiResult::OK
    }
    static S10_HUGE_VT: MappedWireVTable<u64> = MappedWireVTable {
        header: VTableHeader::from_raw_parts(
            core::mem::size_of::<MappedWireVTable<u64>>() as u32,
            ComponentUuid::from_parts("s10::Huge", 8, 8, 0),
            (isize::MAX as usize) & !7,
            8,
        ),
        get: Some(s10_wire_get),
        set: None,
    };
    static S10_BAD_ALIGN_VT: MappedWireVTable<u64> = MappedWireVTable {
        header: VTableHeader::from_raw_parts(
            core::mem::size_of::<MappedWireVTable<u64>>() as u32,
            ComponentUuid::from_parts("s10::BadAlign", 8, 8, 0),
            9,
            3,
        ),
        get: Some(s10_wire_get),
        set: None,
    };
    unsafe extern "C" fn s10_get_proc(_: *const BamHostCtx, sym: *const c_char) -> *const c_void {
        // SAFETY: Valid CStr.
        match unsafe { CStr::from_ptr(sym) }.to_bytes() {
            b"bam.v0.s10::Huge.vtable" => (&raw const S10_HUGE_VT).cast(),
            b"bam.v0.s10::BadAlign.vtable" => (&raw const S10_BAD_ALIGN_VT).cast(),
            _ => core::ptr::null(),
        }
    }
    unsafe extern "C" fn s10_next(_: *mut BamIter, out_len: *mut usize) -> FfiBool {
        // SAFETY: Writable stack `out_len`.
        unsafe { *out_len = 2 };
        FfiBool::TRUE
    }
    unsafe extern "C" fn s10_info(
        _: *const BamIter,
        term: usize,
        out: *mut BamColumnInfo,
    ) -> FfiResult {
        let info = match term {
            0 => BamColumnInfo {
                component: BamComponent::from_raw(1),
                mode: AccessMode::READ,
                uuid: S10_HUGE_VT.header.uuid,
                size: S10_HUGE_VT.header.size,
                align: S10_HUGE_VT.header.align,
                storage_flags: 0,
                reserved0: 0,
            },
            1 => BamColumnInfo {
                component: BamComponent::from_raw(2),
                mode: AccessMode::READ,
                uuid: S10_BAD_ALIGN_VT.header.uuid,
                size: S10_BAD_ALIGN_VT.header.size,
                align: S10_BAD_ALIGN_VT.header.align,
                storage_flags: 0,
                reserved0: 0,
            },
            _ => return FfiResult::OUT_OF_RANGE,
        };
        // SAFETY: Writable stack `out`.
        unsafe { *out = info };
        FfiResult::OK
    }
    static S10_COL_BUF: [u64; 2] = [1, 2];
    unsafe extern "C" fn s10_col_ro(_: *const BamIter, _: usize) -> *const u8 {
        S10_COL_BUF.as_ptr().cast()
    }

    let mut b_api = make_mock_api();
    b_api.get_proc_address = Some(s10_get_proc);
    b_api.iter_next_chunk = Some(s10_next);
    b_api.iter_column_info = Some(s10_info);
    b_api.iter_column_readonly = Some(s10_col_ro);
    // SAFETY: Stack pointers valid for test.
    let b_host = unsafe { Host::from_raw(&b_api, (&raw const dummy_ctx).cast()) }.unwrap();
    let huge_vt = b_host
        .resolve_mapped_wire_vtable::<u64>(c"bam.v0.s10::Huge.vtable", S10_HUGE_VT.header.uuid)
        .unwrap();
    let bad_align_vt = b_host
        .resolve_mapped_wire_vtable::<u64>(
            c"bam.v0.s10::BadAlign.vtable",
            S10_BAD_ALIGN_VT.header.uuid,
        )
        .unwrap();
    // SAFETY: `dummy_ctx` acts as `BamIter`.
    let mut b_iter = unsafe { ValidatedIter::from_raw((&raw mut dummy_ctx).cast(), &b_host) };
    assert_eq!(b_iter.next_chunk(), Some(2));
    assert_eq!(
        b_iter.read_mapped_wire_row(0, 1, &huge_vt),
        Err(FfiResult::OUT_OF_RANGE)
    );
    assert_eq!(
        b_iter.read_mapped_wire_row(1, 0, &bad_align_vt),
        Err(FfiResult::TYPE_MISMATCH)
    );
}

// ============================================================================
// S11: `BatchColCache`, `advance_batch_into`, and `ChunkColumnDispenser`
//      scratch/epoch/cross-iter soundness under Miri
// ============================================================================

/// Verifies under Miri (Tree & Stacked Borrows) that:
/// 1. `advance_batch_into(&mut cache, ...)` followed by `ChunkColumnDispenser::new(&mut iter)`
///    does NOT read uninitialized or stale `QuerySlotScratch::batch_col_ptrs` and falls back cleanly
///    to `iter_column_readonly` / `iter_column_mutable`.
/// 2. Passing a stale `BatchColCache` (from an earlier chunk, before `next_chunk`/`reset`, or from
///    a different `ValidatedIter`) to `ChunkColumnDispenser::new_with_batch` falls back cleanly
///    without reading dangling or mismatched-length pointers.
#[test]
fn s11_batch_col_cache_and_dispenser_epoch_and_scratch_soundness() {
    struct MultiChunkBatchIter {
        chunk_idx: usize,
        chunk0_ro: Vec<u32>,
        chunk0_rw: Vec<u32>,
        chunk1_ro: Vec<u32>,
        chunk1_rw: Vec<u32>,
        legacy_ro_calls: usize,
        legacy_rw_calls: usize,
    }

    impl MultiChunkBatchIter {
        fn cur_len(&self) -> usize {
            match self.chunk_idx {
                1 => self.chunk0_ro.len(),
                2 => self.chunk1_ro.len(),
                _ => 0,
            }
        }
    }

    unsafe extern "C" fn s11_col_info(
        _: *const BamIter,
        term_index: usize,
        out: *mut BamColumnInfo,
    ) -> FfiResult {
        let info = match term_index {
            0 => BamColumnInfo {
                component: BamComponent::from_raw(1),
                uuid: ComponentUuid::from_parts("s11::ReadU32", 4, 4, 0),
                size: 4,
                align: 4,
                mode: AccessMode::READ,
                storage_flags: 0,
                reserved0: 0,
            },
            1 => BamColumnInfo {
                component: BamComponent::from_raw(2),
                uuid: ComponentUuid::from_parts("s11::WriteU32", 4, 4, 0),
                size: 4,
                align: 4,
                mode: AccessMode::WRITE,
                storage_flags: 0,
                reserved0: 0,
            },
            _ => return FfiResult::OUT_OF_RANGE,
        };
        // SAFETY: `out` is writable stack storage.
        unsafe { *out = info };
        FfiResult::OK
    }

    unsafe extern "C" fn s11_next_chunk(iter: *mut BamIter, out_len: *mut usize) -> FfiBool {
        // SAFETY: `iter` points to `MultiChunkBatchIter`.
        let st = unsafe { &mut *iter.cast::<MultiChunkBatchIter>() };
        st.chunk_idx += 1;
        let len = st.cur_len();
        if len == 0 {
            FfiBool::FALSE
        } else {
            // SAFETY: `out_len` is writable stack storage.
            unsafe { *out_len = len };
            FfiBool::TRUE
        }
    }

    unsafe extern "C" fn s11_next_chunk_batch(
        iter: *mut BamIter,
        out_len: *mut usize,
        _out_entities: *mut *const FfiEntity,
        out_col_ptrs: *mut *mut u8,
        col_count: usize,
        _mark_changed_mask: u32,
    ) -> FfiBool {
        // SAFETY: `iter` points to `MultiChunkBatchIter`.
        let st = unsafe { &mut *iter.cast::<MultiChunkBatchIter>() };
        st.chunk_idx += 1;
        let len = st.cur_len();
        if len == 0 {
            return FfiBool::FALSE;
        }
        // SAFETY: `out_len` is writable stack storage.
        unsafe { *out_len = len };
        let (ro_ptr, rw_ptr) = match st.chunk_idx {
            1 => (
                st.chunk0_ro.as_mut_ptr().cast::<u8>(),
                st.chunk0_rw.as_mut_ptr().cast::<u8>(),
            ),
            2 => (
                st.chunk1_ro.as_mut_ptr().cast::<u8>(),
                st.chunk1_rw.as_mut_ptr().cast::<u8>(),
            ),
            _ => return FfiBool::FALSE,
        };
        // SAFETY: Caller initializes `col_count` writable `*mut u8` slots at `out_col_ptrs`.
        unsafe {
            if col_count > 0 {
                *out_col_ptrs.add(0) = ro_ptr;
            }
            if col_count > 1 {
                *out_col_ptrs.add(1) = rw_ptr;
            }
        }
        FfiBool::TRUE
    }

    unsafe extern "C" fn s11_col_ro(iter: *const BamIter, term_index: usize) -> *const u8 {
        // SAFETY: `iter` points to `MultiChunkBatchIter`.
        let st = unsafe { &mut *iter.cast::<MultiChunkBatchIter>().cast_mut() };
        st.legacy_ro_calls += 1;
        if term_index != 0 {
            return core::ptr::null();
        }
        match st.chunk_idx {
            1 => st.chunk0_ro.as_ptr().cast::<u8>(),
            2 => st.chunk1_ro.as_ptr().cast::<u8>(),
            _ => core::ptr::null(),
        }
    }

    unsafe extern "C" fn s11_col_rw(iter: *mut BamIter, term_index: usize) -> *mut u8 {
        // SAFETY: `iter` points to `MultiChunkBatchIter`.
        let st = unsafe { &mut *iter.cast::<MultiChunkBatchIter>() };
        st.legacy_rw_calls += 1;
        if term_index != 1 {
            return core::ptr::null_mut();
        }
        match st.chunk_idx {
            1 => st.chunk0_rw.as_mut_ptr().cast::<u8>(),
            2 => st.chunk1_rw.as_mut_ptr().cast::<u8>(),
            _ => core::ptr::null_mut(),
        }
    }

    unsafe extern "C" fn s11_reset(iter: *mut BamIter) -> FfiResult {
        // SAFETY: `iter` points to `MultiChunkBatchIter`.
        let st = unsafe { &mut *iter.cast::<MultiChunkBatchIter>() };
        st.chunk_idx = 0;
        FfiResult::OK
    }

    let mut api = make_mock_api();
    api.iter_column_info = Some(s11_col_info);
    api.iter_next_chunk = Some(s11_next_chunk);
    api.iter_next_chunk_batch = Some(s11_next_chunk_batch);
    api.iter_column_readonly = Some(s11_col_ro);
    api.iter_column_mutable = Some(s11_col_rw);
    api.iter_reset = Some(s11_reset);

    let dummy_ctx = 0u64;
    // SAFETY: Stack pointers remain valid for test.
    let host = unsafe { Host::from_raw(&api, (&raw const dummy_ctx).cast()) }.unwrap();

    // Case (a1): `advance_batch_into` on an iter with uninitialized `QuerySlotScratch`, followed
    // by `ChunkColumnDispenser::new(&mut iter)` must NOT read uninit `slot_scratch.batch_col_ptrs`.
    {
        let mut st = MultiChunkBatchIter {
            chunk_idx: 0,
            chunk0_ro: vec![11],
            chunk0_rw: vec![100],
            chunk1_ro: vec![21, 22, 23, 24],
            chunk1_rw: vec![200, 201, 202, 203],
            legacy_ro_calls: 0,
            legacy_rw_calls: 0,
        };
        let mut scratch = QuerySlotScratch::new();
        // SAFETY: `st` and `scratch` outlive `iter`.
        let mut iter = unsafe {
            ValidatedIter::from_raw_with_scratch(
                (&raw mut st).cast::<BamIter>(),
                &host,
                &mut scratch,
            )
        };
        let mut cache = BatchColCache::new();
        assert_eq!(iter.advance_batch_into(&mut cache, 2, false, 0), Some(1));
        {
            let mut disp = ChunkColumnDispenser::new(&mut iter);
            assert_eq!(disp.next_read::<u32>(AccessMode::READ, None), Ok(&[11][..]));
            let rw = disp.next_write::<u32>(None).unwrap();
            assert_eq!(rw, &[100]);
            rw[0] = 101;
        }
        assert_eq!(st.legacy_ro_calls, 1);
        assert_eq!(st.legacy_rw_calls, 1);
        assert_eq!(st.chunk0_rw, vec![101]);

        // Case (a2): `advance_batch` populates `slot_scratch` on chunk 0 (len=1) -- wait: we already
        // consumed chunk 0, so reset to chunk 0 first!
        assert_eq!(iter.reset(), FfiResult::OK);
        assert_eq!(iter.advance_batch(2, false, 0), Some(1));
        // Now advance chunk 1 (len=4) via `advance_batch_into`: `slot_scratch` still holds chunk 0's
        // 1-element pointers! `ChunkColumnDispenser::new(&mut iter)` MUST fall back cleanly rather than
        // pairing chunk 0's 1-element pointers with chunk 1's `len = 4`!
        assert_eq!(iter.advance_batch_into(&mut cache, 2, false, 0), Some(4));
        {
            let mut disp = ChunkColumnDispenser::new(&mut iter);
            assert_eq!(
                disp.next_read::<u32>(AccessMode::READ, None),
                Ok(&[21, 22, 23, 24][..])
            );
            assert_eq!(
                disp.next_write::<u32>(None),
                Ok(&mut [200, 201, 202, 203][..])
            );
        }
        assert_eq!(st.legacy_ro_calls, 2);
        assert_eq!(st.legacy_rw_calls, 2);
    }

    // Case (b1): Adversarial Challenger 2 repro — stale chunk-0 `BatchColCache` (len=1) passed to
    // `ChunkColumnDispenser::new_with_batch` while active chunk is chunk 1 (len=4).
    {
        let mut st = MultiChunkBatchIter {
            chunk_idx: 0,
            chunk0_ro: vec![11],
            chunk0_rw: vec![100],
            chunk1_ro: vec![21, 22, 23, 24],
            chunk1_rw: vec![200, 201, 202, 203],
            legacy_ro_calls: 0,
            legacy_rw_calls: 0,
        };
        // SAFETY: `st` outlives `iter`.
        let mut iter = unsafe { ValidatedIter::from_raw((&raw mut st).cast::<BamIter>(), &host) };
        let mut cache0 = BatchColCache::new();
        let mut cache1 = BatchColCache::new();
        assert_eq!(iter.advance_batch_into(&mut cache0, 2, false, 0), Some(1));
        assert_eq!(iter.advance_batch_into(&mut cache1, 2, false, 0), Some(4));

        // Passing stale `cache0` MUST fall back cleanly to legacy columns (zero UB under Miri!).
        {
            let mut disp = ChunkColumnDispenser::new_with_batch(&mut iter, &cache0);
            assert_eq!(
                disp.next_read::<u32>(AccessMode::READ, None),
                Ok(&[21, 22, 23, 24][..])
            );
            assert_eq!(
                disp.next_write::<u32>(None),
                Ok(&mut [200, 201, 202, 203][..])
            );
        }
        assert_eq!(st.legacy_ro_calls, 1);
        assert_eq!(st.legacy_rw_calls, 1);

        // Passing matching `cache1` STILL uses slot-68 batch pointers with 0 extra legacy calls!
        {
            let mut disp = ChunkColumnDispenser::new_with_batch(&mut iter, &cache1);
            assert_eq!(
                disp.next_read::<u32>(AccessMode::READ, None),
                Ok(&[21, 22, 23, 24][..])
            );
            assert_eq!(
                disp.next_write::<u32>(None),
                Ok(&mut [200, 201, 202, 203][..])
            );
        }
        assert_eq!(st.legacy_ro_calls, 1);
        assert_eq!(st.legacy_rw_calls, 1);

        // After `reset()` and `next_chunk()` to chunk 0, `cache0` (from an earlier epoch) MUST also
        // fall back cleanly rather than reusing the old epoch's pointers!
        assert_eq!(iter.reset(), FfiResult::OK);
        assert_eq!(iter.next_chunk(), Some(1));
        {
            let mut disp = ChunkColumnDispenser::new_with_batch(&mut iter, &cache0);
            assert_eq!(disp.next_read::<u32>(AccessMode::READ, None), Ok(&[11][..]));
        }
        assert_eq!(st.legacy_ro_calls, 2);

        // After `next_chunk_batch` advances to chunk 1, `cache1` (from an earlier epoch) MUST also
        // fall back cleanly rather than reusing stale batch pointers!
        let mut raw_cols = [core::ptr::null_mut(); 2];
        assert_eq!(iter.next_chunk_batch(None, &mut raw_cols, 0), Ok(Some(4)));
        {
            let mut disp = ChunkColumnDispenser::new_with_batch(&mut iter, &cache1);
            assert_eq!(
                disp.next_read::<u32>(AccessMode::READ, None),
                Ok(&[21, 22, 23, 24][..])
            );
        }
        assert_eq!(st.legacy_ro_calls, 3);
    }

    // Case (b2): Cross-iterator mismatch — `BatchColCache` populated on `iter_a` (len=1) passed
    // to `ChunkColumnDispenser::new_with_batch(&mut iter_b, &cache_a)` (len=4) falls back cleanly.
    {
        let mut st_a = MultiChunkBatchIter {
            chunk_idx: 0,
            chunk0_ro: vec![99],
            chunk0_rw: vec![999],
            chunk1_ro: vec![],
            chunk1_rw: vec![],
            legacy_ro_calls: 0,
            legacy_rw_calls: 0,
        };
        let mut st_b = MultiChunkBatchIter {
            chunk_idx: 1, // so first advance yields chunk 1 (len=4)
            chunk0_ro: vec![1],
            chunk0_rw: vec![2],
            chunk1_ro: vec![31, 32, 33, 34],
            chunk1_rw: vec![300, 301, 302, 303],
            legacy_ro_calls: 0,
            legacy_rw_calls: 0,
        };
        // SAFETY: `st_a` outlives `iter_a`.
        let mut iter_a =
            unsafe { ValidatedIter::from_raw((&raw mut st_a).cast::<BamIter>(), &host) };
        // SAFETY: `st_b` outlives `iter_b`.
        let mut iter_b =
            unsafe { ValidatedIter::from_raw((&raw mut st_b).cast::<BamIter>(), &host) };
        let mut cache_a = BatchColCache::new();
        let mut cache_b = BatchColCache::new();
        assert_eq!(
            iter_a.advance_batch_into(&mut cache_a, 2, false, 0),
            Some(1)
        );
        assert_eq!(
            iter_b.advance_batch_into(&mut cache_b, 2, false, 0),
            Some(4)
        );

        // Both have `batch_epoch == 2`, so only `iter_id` distinguishes them!
        {
            let mut disp = ChunkColumnDispenser::new_with_batch(&mut iter_b, &cache_a);
            assert_eq!(
                disp.next_read::<u32>(AccessMode::READ, None),
                Ok(&[31, 32, 33, 34][..])
            );
            assert_eq!(
                disp.next_write::<u32>(None),
                Ok(&mut [300, 301, 302, 303][..])
            );
        }
        assert_eq!(st_b.legacy_ro_calls, 1);
        assert_eq!(st_b.legacy_rw_calls, 1);
    }
}

// ============================================================================
// S12: `ValidatedIter::from_raw_with_scratch` resets `col_info_mask` on reuse
// ============================================================================

/// Verifies that reusing the same `&mut QuerySlotScratch` across `iter_a` (writable `u32` at
/// term 0) and `iter_b` (read-only `u64` / `u32` at term 0) resets `col_info_mask` in
/// `ValidatedIter::from_raw_with_scratch`, preventing `iter_b` from observing `iter_a`'s cached
/// `BamColumnInfo`.
#[test]
fn s12_query_slot_scratch_reuse_resets_column_info_mask() {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum IterKind {
        WritableU32,
        ReadOnlyU64,
        ReadOnlyU32,
    }

    struct ScratchIterState {
        kind: IterKind,
        advanced: bool,
        u32_buf: [u32; 2],
        u64_buf: [u64; 2],
        col_info_calls: usize,
    }

    let write_u32_uuid = ComponentUuid::from_parts("s12::WriteU32", 4, 4, 0);
    let read_u64_uuid = ComponentUuid::from_parts("s12::ReadU64", 8, 8, 0);
    let read_u32_uuid = ComponentUuid::from_parts("s12::ReadU32", 4, 4, 0);

    unsafe extern "C" fn s12_col_info(
        iter: *const BamIter,
        term_index: usize,
        out: *mut BamColumnInfo,
    ) -> FfiResult {
        if term_index != 0 {
            return FfiResult::OUT_OF_RANGE;
        }
        // SAFETY: `iter` points to a valid `ScratchIterState` for the test duration.
        let st = unsafe { &mut *iter.cast::<ScratchIterState>().cast_mut() };
        st.col_info_calls += 1;
        let info = match st.kind {
            IterKind::WritableU32 => BamColumnInfo {
                component: BamComponent::from_raw(10),
                uuid: ComponentUuid::from_parts("s12::WriteU32", 4, 4, 0),
                size: 4,
                align: 4,
                mode: AccessMode::WRITE,
                storage_flags: 0,
                reserved0: 0,
            },
            IterKind::ReadOnlyU64 => BamColumnInfo {
                component: BamComponent::from_raw(11),
                uuid: ComponentUuid::from_parts("s12::ReadU64", 8, 8, 0),
                size: 8,
                align: 8,
                mode: AccessMode::READ,
                storage_flags: 0,
                reserved0: 0,
            },
            IterKind::ReadOnlyU32 => BamColumnInfo {
                component: BamComponent::from_raw(12),
                uuid: ComponentUuid::from_parts("s12::ReadU32", 4, 4, 0),
                size: 4,
                align: 4,
                mode: AccessMode::READ,
                storage_flags: 0,
                reserved0: 0,
            },
        };
        // SAFETY: `out` is writable stack storage.
        unsafe { *out = info };
        FfiResult::OK
    }

    unsafe extern "C" fn s12_next_chunk(iter: *mut BamIter, out_len: *mut usize) -> FfiBool {
        // SAFETY: `iter` points to `ScratchIterState`.
        let st = unsafe { &mut *iter.cast::<ScratchIterState>() };
        if st.advanced {
            return FfiBool::FALSE;
        }
        st.advanced = true;
        // SAFETY: `out_len` is writable stack storage.
        unsafe { *out_len = 2 };
        FfiBool::TRUE
    }

    unsafe extern "C" fn s12_col_ro(iter: *const BamIter, term_index: usize) -> *const u8 {
        if term_index != 0 {
            return core::ptr::null();
        }
        // SAFETY: `iter` points to `ScratchIterState`.
        let st = unsafe { &*iter.cast::<ScratchIterState>() };
        match st.kind {
            IterKind::WritableU32 | IterKind::ReadOnlyU32 => st.u32_buf.as_ptr().cast::<u8>(),
            IterKind::ReadOnlyU64 => st.u64_buf.as_ptr().cast::<u8>(),
        }
    }

    unsafe extern "C" fn s12_col_rw(iter: *mut BamIter, term_index: usize) -> *mut u8 {
        if term_index != 0 {
            return core::ptr::null_mut();
        }
        // SAFETY: `iter` points to `ScratchIterState`.
        let st = unsafe { &mut *iter.cast::<ScratchIterState>() };
        match st.kind {
            IterKind::WritableU32 => st.u32_buf.as_mut_ptr().cast::<u8>(),
            // Read-only iterators must never expose mutable pointers.
            IterKind::ReadOnlyU64 | IterKind::ReadOnlyU32 => core::ptr::null_mut(),
        }
    }

    let mut api = make_mock_api();
    api.iter_column_info = Some(s12_col_info);
    api.iter_next_chunk = Some(s12_next_chunk);
    api.iter_column_readonly = Some(s12_col_ro);
    api.iter_column_mutable = Some(s12_col_rw);

    let dummy_ctx = 0u64;
    // SAFETY: Stack pointers remain valid for test.
    let host = unsafe { Host::from_raw(&api, (&raw const dummy_ctx).cast()) }.unwrap();
    let mut scratch = QuerySlotScratch::new();

    // 1. First iterator populates `scratch` at term 0 with `WRITE` `u32`.
    let mut st_a = ScratchIterState {
        kind: IterKind::WritableU32,
        advanced: false,
        u32_buf: [10, 20],
        u64_buf: [0; 2],
        col_info_calls: 0,
    };
    {
        // SAFETY: `st_a` and `scratch` outlive `iter_a`.
        let mut iter_a = unsafe {
            ValidatedIter::from_raw_with_scratch(
                (&raw mut st_a).cast::<BamIter>(),
                &host,
                &mut scratch,
            )
        };
        assert_eq!(iter_a.next_chunk(), Some(2));
        let info_a = iter_a.column_info(0).unwrap();
        assert_eq!(info_a.mode, AccessMode::WRITE);
        assert_eq!(info_a.uuid, write_u32_uuid);
        let rw = iter_a
            .column_mut_checked::<u32>(0, Some(UuidFilter::exact(write_u32_uuid)))
            .unwrap();
        rw[0] = 11;
        assert_eq!(st_a.col_info_calls, 1);
    }

    // 2. Reusing the same `scratch` for `iter_b` (`READ` `u64` at term 0) must reset
    // `col_info_mask` and fetch `iter_b`'s metadata rather than leaking `iter_a`'s `WRITE u32`.
    let mut st_b = ScratchIterState {
        kind: IterKind::ReadOnlyU64,
        advanced: false,
        u32_buf: [0; 2],
        u64_buf: [100, 200],
        col_info_calls: 0,
    };
    {
        // SAFETY: `st_b` and `scratch` outlive `iter_b`.
        let mut iter_b = unsafe {
            ValidatedIter::from_raw_with_scratch(
                (&raw mut st_b).cast::<BamIter>(),
                &host,
                &mut scratch,
            )
        };
        assert_eq!(iter_b.next_chunk(), Some(2));
        let info_b = iter_b.column_info(0).unwrap();
        assert_eq!(st_b.col_info_calls, 1);
        assert_eq!(info_b.mode, AccessMode::READ);
        assert_eq!(info_b.uuid, read_u64_uuid);
        assert_eq!(info_b.size, 8);
        assert_eq!(info_b.align, 8);

        assert!(iter_b.column_mut_checked::<u32>(0, None).is_none());
        assert!(iter_b.column_checked::<u32>(0, None).is_none());
        {
            let mut disp = ChunkColumnDispenser::new(&mut iter_b);
            assert_eq!(disp.next_write::<u32>(None), Err(FfiResult::TYPE_MISMATCH));
        }
        assert_eq!(
            iter_b.column_checked::<u64>(0, Some(UuidFilter::exact(read_u64_uuid))),
            Some(&[100u64, 200u64][..])
        );
    }

    // 3. Reusing the same `scratch` again for `iter_c` (`READ` `u32` — same size/align as `iter_a`,
    //    but read-only and distinct UUID) also rejects mutable access and reads `iter_c`'s slice.
    let mut st_c = ScratchIterState {
        kind: IterKind::ReadOnlyU32,
        advanced: false,
        u32_buf: [42, 43],
        u64_buf: [0; 2],
        col_info_calls: 0,
    };
    {
        // SAFETY: `st_c` and `scratch` outlive `iter_c`.
        let mut iter_c = unsafe {
            ValidatedIter::from_raw_with_scratch(
                (&raw mut st_c).cast::<BamIter>(),
                &host,
                &mut scratch,
            )
        };
        assert_eq!(iter_c.next_chunk(), Some(2));
        let info_c = iter_c.column_info(0).unwrap();
        assert_eq!(st_c.col_info_calls, 1);
        assert_eq!(info_c.mode, AccessMode::READ);
        assert_eq!(info_c.uuid, read_u32_uuid);
        assert!(iter_c.column_mut_checked::<u32>(0, None).is_none());
        assert_eq!(
            iter_c.column_checked::<u32>(0, Some(UuidFilter::exact(read_u32_uuid))),
            Some(&[42u32, 43u32][..])
        );
    }
}

// ============================================================================
// S13: Hot-reload column migration & `from_raw_with_scratch` 16 B -> 8 B shrink
// ============================================================================

/// Verifies the per-system `BamColumnInfo` invariance and hot-reload cache contract (`S13`):
/// (a) Simulated hot-reload (`GuestInstance` v1 `[f32; 4]` 16 B shut down -> host migrates the
///     same `BamComponent` handle to v2 `[f32; 2]` 8 B -> fresh `GuestInstance` v2 initialized)
///     starts with fresh `SlotScratchesCell` caches (1 `iter_column_info` call on Tick 1, 0 on
///     Tick 2) and reads `8 B` v2 slices cleanly, while an un-migrated v1 registration against the
///     migrated `8 B` column fails closed on Tick 1 via `iter_column_info` without over-reading.
/// (b) Reusing caller-owned `&mut QuerySlotScratch` via `ValidatedIter::from_raw_with_scratch`
///     across a `16 B -> 8 B` schema shrink (`S12` shrink variant) calls `reset_all()` and rejects
///     an out-of-date `next_read::<[f32; 4]>` with `TYPE_MISMATCH` before `slice::from_raw_parts`
///     can over-read the 16-byte v2 allocation under Miri.
#[test]
fn s13_hot_reload_column_migration_and_scratch_shrink_soundness() {
    use std::cell::Cell;

    struct S13HostState {
        is_v2: Cell<bool>,
        col_info_calls: AtomicUsize,
        v1_uuid: ComponentUuid,
        v2_uuid: ComponentUuid,
        v1_data: Box<[[f32; 4]; 2]>,
        v2_data: Box<[[f32; 2]; 2]>,
        registered_fn: Cell<Option<bamabi_sys::BamSystemFn>>,
        registered_ud: Cell<*mut c_void>,
    }

    struct S13IterState {
        host: *const S13HostState,
        advanced: Cell<bool>,
    }

    unsafe extern "C" fn s13_register_system(
        load_ctx: *mut BamLoadCtx,
        desc: *const BamSystemDesc,
    ) -> FfiResult {
        if load_ctx.is_null() || desc.is_null() {
            return FfiResult::NULL_POINTER;
        }
        // SAFETY: `desc` is a valid `BamSystemDesc` from `ValidatedLoadCtx`.
        let status = unsafe { (*desc).validate() };
        if !status.is_ok() {
            return status;
        }
        // SAFETY: `load_ctx` points to `S13HostState` during `init_guest_instance`.
        let host = unsafe { &*load_ctx.cast::<S13HostState>() };
        // SAFETY: `desc` is readable.
        host.registered_fn.set(unsafe { (*desc).callback });
        // SAFETY: `desc` is readable.
        host.registered_ud.set(unsafe { (*desc).user_data });
        FfiResult::OK
    }

    unsafe extern "C" fn s13_sys_query(
        sys_ctx: *mut BamSystemCtx,
        query_index: usize,
        out_iter: *mut *mut BamIter,
    ) -> FfiResult {
        if sys_ctx.is_null() || out_iter.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if query_index != 0 {
            return FfiResult::OUT_OF_RANGE;
        }
        // SAFETY: `sys_ctx` points to `S13IterState` for this tick.
        let iter_state = unsafe { &*sys_ctx.cast::<S13IterState>() };
        iter_state.advanced.set(false);
        // SAFETY: `out_iter` is writable stack storage.
        unsafe { *out_iter = sys_ctx.cast::<BamIter>() };
        FfiResult::OK
    }

    unsafe extern "C" fn s13_next_chunk(iter: *mut BamIter, out_len: *mut usize) -> FfiBool {
        if iter.is_null() || out_len.is_null() {
            return FfiBool::FALSE;
        }
        // SAFETY: `iter` points to `S13IterState`.
        let st = unsafe { &*iter.cast::<S13IterState>() };
        if st.advanced.get() {
            return FfiBool::FALSE;
        }
        st.advanced.set(true);
        // SAFETY: `out_len` is writable stack storage.
        unsafe { *out_len = 2 };
        FfiBool::TRUE
    }

    unsafe extern "C" fn s13_col_info(
        iter: *const BamIter,
        term_index: usize,
        out: *mut BamColumnInfo,
    ) -> FfiResult {
        if iter.is_null() || out.is_null() {
            return FfiResult::NULL_POINTER;
        }
        if term_index != 0 {
            return FfiResult::OUT_OF_RANGE;
        }
        // SAFETY: `iter` points to `S13IterState` and `host` stays immutable during ticks.
        let host = unsafe { &*(*iter.cast::<S13IterState>()).host };
        host.col_info_calls.fetch_add(1, Ordering::Relaxed);
        let info = if host.is_v2.get() {
            BamColumnInfo {
                component: BamComponent::from_raw(1),
                mode: AccessMode::READ,
                uuid: host.v2_uuid,
                size: 8,
                align: 4,
                storage_flags: 0,
                reserved0: 0,
            }
        } else {
            BamColumnInfo {
                component: BamComponent::from_raw(1),
                mode: AccessMode::READ,
                uuid: host.v1_uuid,
                size: 16,
                align: 4,
                storage_flags: 0,
                reserved0: 0,
            }
        };
        // SAFETY: `out` is writable stack storage.
        unsafe { *out = info };
        FfiResult::OK
    }

    unsafe extern "C" fn s13_col_ro(iter: *const BamIter, term_index: usize) -> *const u8 {
        if iter.is_null() || term_index != 0 {
            return core::ptr::null();
        }
        // SAFETY: `iter` points to `S13IterState` and `host` stays immutable during ticks.
        let host = unsafe { &*(*iter.cast::<S13IterState>()).host };
        if host.is_v2.get() {
            host.v2_data.as_ptr().cast::<u8>()
        } else {
            host.v1_data.as_ptr().cast::<u8>()
        }
    }

    let v1_uuid = ComponentUuid::from_parts("s13::Position", 16, 4, 1);
    let v2_uuid = ComponentUuid::from_parts("s13::Position", 8, 4, 2);

    let mut api = make_mock_api();
    api.register_system = Some(s13_register_system);
    api.sys_query = Some(s13_sys_query);
    api.iter_next_chunk = Some(s13_next_chunk);
    api.iter_column_info = Some(s13_col_info);
    api.iter_column_readonly = Some(s13_col_ro);

    let mut host_state = S13HostState {
        is_v2: Cell::new(false),
        col_info_calls: AtomicUsize::new(0),
        v1_uuid,
        v2_uuid,
        v1_data: Box::new([[1.0, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0]]),
        // Strictly 16-byte heap allocation (2 * 8 B): any accidental 32-byte `[f32; 4]` slice
        // read triggers immediate UB under Miri.
        v2_data: Box::new([[10.0, 20.0], [30.0, 40.0]]),
        registered_fn: Cell::new(None),
        registered_ud: Cell::new(core::ptr::null_mut()),
    };

    let manifest_v1 = BamManifest::new(c"s13_mod".as_ptr(), c"0.1.0".as_ptr());
    let manifest_v2 = BamManifest::new(c"s13_mod".as_ptr(), c"0.2.0".as_ptr());

    // ------------------------------------------------------------------------
    // Part (a): Simulated hot-reload across `16 B -> 8 B` migration
    // ------------------------------------------------------------------------
    let mut inst_v1: *mut BamInstance = core::ptr::null_mut();
    let v1_terms = [BamTermDesc::read(BamComponent::from_raw(1))];
    // SAFETY: `api` and `host_state` remain live across `init_guest_instance`.
    let init_v1_res = unsafe {
        init_guest_instance(
            &raw const api,
            (&raw mut host_state).cast::<BamLoadCtx>(),
            &raw mut inst_v1,
            &manifest_v1,
            move |inst, load| {
                let queries = [&v1_terms[..]];
                let cfg = SystemRegistrationConfig {
                    name: c"s13_v1_read",
                    user_queries: &queries,
                    ..Default::default()
                };
                load.register_system_slice::<1, _>(inst, &cfg, None, move |inv| {
                    let mut iter = inv.take_raw_query_slot(0).unwrap();
                    assert_eq!(iter.next_chunk(), Some(2));
                    let mut disp = ChunkColumnDispenser::new(&mut iter);
                    let pos = match disp
                        .next_read::<[f32; 4]>(AccessMode::READ, Some(UuidFilter::exact(v1_uuid)))
                    {
                        Ok(s) => s,
                        Err(e) => return e,
                    };
                    assert_eq!(pos, &[[1.0, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0]]);
                    FfiResult::OK
                })
            },
        )
    };
    assert_eq!(init_v1_res, FfiResult::OK);
    let v1_fn = host_state.registered_fn.take().unwrap();
    let v1_ud = host_state.registered_ud.get();

    let mut iter_ctx = S13IterState {
        host: &raw const host_state,
        advanced: Cell::new(false),
    };
    // Tick 1 populates `SlotScratchesCell` (1 `iter_column_info` call); Tick 2 hits cache (still 1).
    // SAFETY: `inst_v1` and `iter_ctx` are active.
    let v1_t1 = unsafe { v1_fn((&raw mut iter_ctx).cast::<BamSystemCtx>(), v1_ud) };
    assert_eq!(v1_t1, FfiResult::OK);
    assert_eq!(host_state.col_info_calls.load(Ordering::Relaxed), 1);
    // SAFETY: `inst_v1` and `iter_ctx` are active.
    let v1_t2 = unsafe { v1_fn((&raw mut iter_ctx).cast::<BamSystemCtx>(), v1_ud) };
    assert_eq!(v1_t2, FfiResult::OK);
    assert_eq!(host_state.col_info_calls.load(Ordering::Relaxed), 1);

    // Tear down v1 guest instance and migrate the host's `BamComponent(1)` column from 16 B -> 8 B.
    // SAFETY: `inst_v1` was initialized by `init_guest_instance` and is shut down once.
    unsafe { shutdown_guest_instance(inst_v1) };
    host_state.is_v2.set(true);
    host_state.col_info_calls.store(0, Ordering::Relaxed);
    host_state.registered_ud.set(core::ptr::null_mut());

    // Fresh v2 guest instance gets a brand-new `SlotScratchesCell` and reads `[f32; 2]` (8 B).
    let mut inst_v2: *mut BamInstance = core::ptr::null_mut();
    let v2_terms = [BamTermDesc::read(BamComponent::from_raw(1))];
    // SAFETY: `api` and `host_state` remain live across `init_guest_instance`.
    let init_v2_res = unsafe {
        init_guest_instance(
            &raw const api,
            (&raw mut host_state).cast::<BamLoadCtx>(),
            &raw mut inst_v2,
            &manifest_v2,
            move |inst, load| {
                let queries = [&v2_terms[..]];
                let cfg = SystemRegistrationConfig {
                    name: c"s13_v2_read",
                    user_queries: &queries,
                    ..Default::default()
                };
                load.register_system_slice::<1, _>(inst, &cfg, None, move |inv| {
                    let mut iter = inv.take_raw_query_slot(0).unwrap();
                    assert_eq!(iter.next_chunk(), Some(2));
                    let mut disp = ChunkColumnDispenser::new(&mut iter);
                    let pos = match disp
                        .next_read::<[f32; 2]>(AccessMode::READ, Some(UuidFilter::exact(v2_uuid)))
                    {
                        Ok(s) => s,
                        Err(e) => return e,
                    };
                    assert_eq!(pos, &[[10.0, 20.0], [30.0, 40.0]]);
                    FfiResult::OK
                })
            },
        )
    };
    assert_eq!(init_v2_res, FfiResult::OK);
    let v2_fn = host_state.registered_fn.take().unwrap();
    let v2_ud = host_state.registered_ud.get();

    // SAFETY: `inst_v2` and `iter_ctx` are active (`host_state` immutable across ticks).
    let v2_t1 = unsafe { v2_fn((&raw mut iter_ctx).cast::<BamSystemCtx>(), v2_ud) };
    assert_eq!(v2_t1, FfiResult::OK);
    assert_eq!(host_state.col_info_calls.load(Ordering::Relaxed), 1);
    // SAFETY: `inst_v2` and `iter_ctx` are active.
    let v2_t2 = unsafe { v2_fn((&raw mut iter_ctx).cast::<BamSystemCtx>(), v2_ud) };
    assert_eq!(v2_t2, FfiResult::OK);
    assert_eq!(host_state.col_info_calls.load(Ordering::Relaxed), 1);
    // SAFETY: `inst_v2` was initialized by `init_guest_instance`.
    unsafe { shutdown_guest_instance(inst_v2) };

    // Un-migrated v1 guest code loaded against the migrated 8 B host fails closed on Tick 1 via
    // `iter_column_info` (both with `v1_uuid` filter and with `None` UUID filter).
    host_state.col_info_calls.store(0, Ordering::Relaxed);
    host_state.registered_ud.set(core::ptr::null_mut());
    let mut inst_stale: *mut BamInstance = core::ptr::null_mut();
    let stale_terms = [BamTermDesc::read(BamComponent::from_raw(1))];
    // SAFETY: `api` and `host_state` remain live across `init_guest_instance`.
    let init_stale_res = unsafe {
        init_guest_instance(
            &raw const api,
            (&raw mut host_state).cast::<BamLoadCtx>(),
            &raw mut inst_stale,
            &manifest_v1,
            move |inst, load| {
                let queries = [&stale_terms[..]];
                let cfg = SystemRegistrationConfig {
                    name: c"s13_stale_v1",
                    user_queries: &queries,
                    ..Default::default()
                };
                load.register_system_slice::<1, _>(inst, &cfg, None, move |inv| {
                    let mut iter = inv.take_raw_query_slot(0).unwrap();
                    assert_eq!(iter.next_chunk(), Some(2));
                    assert!(iter.column_checked::<[f32; 4]>(0, None).is_none());
                    let mut disp = ChunkColumnDispenser::new(&mut iter);
                    match disp
                        .next_read::<[f32; 4]>(AccessMode::READ, Some(UuidFilter::exact(v1_uuid)))
                    {
                        Ok(_) => FfiResult::OK,
                        Err(e) => e,
                    }
                })
            },
        )
    };
    assert_eq!(init_stale_res, FfiResult::OK);
    let stale_fn = host_state.registered_fn.take().unwrap();
    let stale_ud = host_state.registered_ud.get();
    // SAFETY: `inst_stale` and `iter_ctx` are active; fails closed with `TYPE_MISMATCH` before
    // `slice::from_raw_parts` can over-read the 16-byte v2 allocation.
    let stale_res = unsafe { stale_fn((&raw mut iter_ctx).cast::<BamSystemCtx>(), stale_ud) };
    assert_eq!(stale_res, FfiResult::TYPE_MISMATCH);
    assert_eq!(host_state.col_info_calls.load(Ordering::Relaxed), 1);
    // SAFETY: `inst_stale` was initialized by `init_guest_instance`.
    unsafe { shutdown_guest_instance(inst_stale) };

    // ------------------------------------------------------------------------
    // Part (b): Caller-owned `&mut QuerySlotScratch` reuse across 16 B -> 8 B shrink
    // ------------------------------------------------------------------------
    // SAFETY: `api` and `host_state` stay valid for `scratch_host`.
    let scratch_host =
        unsafe { Host::from_raw(&raw const api, (&raw const host_state).cast()) }.unwrap();
    let mut scratch = QuerySlotScratch::new();

    // Bind `iter_v1` (`16 B`) into `scratch`, populating both `col_info_mask` and `term_cache_mask`.
    host_state.is_v2.set(false);
    host_state.col_info_calls.store(0, Ordering::Relaxed);
    let mut raw_iter_v1 = S13IterState {
        host: &raw const host_state,
        advanced: Cell::new(false),
    };
    {
        // SAFETY: `raw_iter_v1` and `scratch` outlive `iter_v1`.
        let mut iter_v1 = unsafe {
            ValidatedIter::from_raw_with_scratch(
                (&raw mut raw_iter_v1).cast::<BamIter>(),
                &scratch_host,
                &mut scratch,
            )
        };
        assert_eq!(iter_v1.next_chunk(), Some(2));
        let mut disp = ChunkColumnDispenser::new(&mut iter_v1);
        let pos_v1 = disp
            .next_read::<[f32; 4]>(AccessMode::READ, None)
            .expect("v1 16B read must succeed");
        assert_eq!(pos_v1, &[[1.0, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0]]);
        assert_eq!(host_state.col_info_calls.load(Ordering::Relaxed), 1);
    }

    // Reuse the exact same `&mut scratch` for `iter_v2` (`8 B`, strictly 16-byte heap buffer).
    // `ValidatedIter::from_raw_with_scratch` must `reset_all()` so an outdated `next_read::<[f32; 4]>`
    // fails closed with `TYPE_MISMATCH` rather than reading stale cached metadata or 32 bytes.
    host_state.is_v2.set(true);
    host_state.col_info_calls.store(0, Ordering::Relaxed);
    let mut raw_iter_v2 = S13IterState {
        host: &raw const host_state,
        advanced: Cell::new(false),
    };
    {
        // SAFETY: `raw_iter_v2` and `scratch` outlive `iter_v2`.
        let mut iter_v2 = unsafe {
            ValidatedIter::from_raw_with_scratch(
                (&raw mut raw_iter_v2).cast::<BamIter>(),
                &scratch_host,
                &mut scratch,
            )
        };
        assert_eq!(iter_v2.next_chunk(), Some(2));
        let mut disp_bad = ChunkColumnDispenser::new(&mut iter_v2);
        assert_eq!(
            disp_bad.next_read::<[f32; 4]>(AccessMode::READ, None),
            Err(FfiResult::TYPE_MISMATCH)
        );
        assert_eq!(host_state.col_info_calls.load(Ordering::Relaxed), 1);

        let mut disp_ok = ChunkColumnDispenser::new(&mut iter_v2);
        let pos_v2 = disp_ok
            .next_read::<[f32; 2]>(AccessMode::READ, None)
            .expect("v2 8B read must succeed on fresh dispenser");
        assert_eq!(pos_v2, &[[10.0, 20.0], [30.0, 40.0]]);
        assert_eq!(host_state.col_info_calls.load(Ordering::Relaxed), 1);
    }
}
