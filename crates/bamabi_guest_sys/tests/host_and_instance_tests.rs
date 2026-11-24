//! Host table, `GuestInstance` isolation, trampoline re-entrancy, sparse rows, and dynamic
//! component lifecycle tests (moved out of `src/lib.rs`).

mod common;

use core::{
    ffi::{CStr, c_void},
    sync::atomic::{AtomicPtr, AtomicUsize, Ordering},
};

use bamabi_guest_sys::*;
use bamabi_sys::{
    AccessMode, BAM_STORAGE_FLAG_SPARSE_SET, BamCommands, BamComponent, BamHostCtx, BamInstance,
    BamLoadCtx, BamManifest, BamRunIfFn, BamSystemCtx, BamSystemFn, ComponentUuid, FfiBool,
    FfiEntity, FfiResult, ModComponent, SymbolEntry, VTableHeader,
};
use common::{
    DummyVTable, MockWorldCtx, PaddedPod, VALID_VTABLE_UUID, make_mock_api,
    stub_component_layout_info_padded,
};

/// Verifies [`ReentrantCell`] re-entrancy rejection and panic recovery across both
/// `BamSystemFn` and `BamRunIfFn` trampolines.
///
/// Scenario:
/// 1. First system invocation calls itself re-entrantly and asserts `FfiResult::ACCESS_CONFLICT`.
/// 2. Second invocation panics inside the closure and asserts `FfiResult::PANIC_CAUGHT`.
/// 3. Third invocation confirms `ReleaseGuard` cleared `in_use` after unwind and returns `OK`.
/// 4. Re-entrant `run_if` predicate returns `FfiBool::FALSE` on nested entry and `TRUE` on outer.
#[test]
fn reentrant_trampoline_conflict() {
    let api = make_mock_api();
    let mut world = MockWorldCtx {
        error_msg: b"diagnostic ok",
        spawned: 0,
        last_inserted_val: 0,
    };
    let host_ctx = (&raw mut world).cast::<BamHostCtx>();
    // SAFETY: `api` and `host_ctx` are live stack objects valid for this test.
    let host = unsafe { Host::from_raw(&api, host_ctx) }.unwrap();
    let mut instance = GuestInstance::new(host, "reentrant_mod");

    /// `Send` pointer wrapper used to pass `user_data` into the re-entrant test closure.
    #[derive(Clone, Copy)]
    struct SendVoidPtr(*mut c_void);
    // SAFETY: Single-threaded test fixture preserving pointer provenance for Miri strict provenance.
    unsafe impl Send for SendVoidPtr {}

    /// Shared slot storing `(BamSystemFn, user_data)` so the closure can re-enter itself.
    #[derive(Clone, Copy)]
    struct SysCallbackSlot {
        func: Option<BamSystemFn>,
        data: SendVoidPtr,
    }
    let slot = std::sync::Arc::new(std::sync::Mutex::new(SysCallbackSlot {
        func: None,
        data: SendVoidPtr(core::ptr::null_mut()),
    }));
    let slot_in_sys = std::sync::Arc::clone(&slot);
    let mut outer_runs = 0usize;

    let (sys_fn, sys_data) = allocate_system_trampoline(&mut instance, 0, 0, None, move |inv| {
        outer_runs += 1;
        if outer_runs == 1 {
            let copy = *slot_in_sys.lock().unwrap();
            let nested_fn = copy.func.unwrap();
            // SAFETY: Passing active `sys_ctx` and matching `user_data` to test re-entrancy guard.
            let nested_res = unsafe { nested_fn(inv.as_raw_sys_ctx(), copy.data.0) };
            assert_eq!(nested_res, FfiResult::ACCESS_CONFLICT);
            return FfiResult::OK;
        }
        if outer_runs == 2 {
            panic!("simulated guest panic to verify ReleaseGuard resets in_use");
        }
        FfiResult::OK
    });
    {
        let mut guard = slot.lock().unwrap();
        guard.func = Some(sys_fn);
        guard.data = SendVoidPtr(sys_data);
    }

    let sys_ctx = (&raw mut world).cast::<BamSystemCtx>();
    // SAFETY: Valid `sys_ctx` and `sys_data`.
    assert_eq!(unsafe { sys_fn(sys_ctx, sys_data) }, FfiResult::OK);
    // SAFETY: Valid `sys_ctx` and `sys_data`.
    let panic_res = unsafe { sys_fn(sys_ctx, sys_data) };
    assert_eq!(panic_res, FfiResult::PANIC_CAUGHT);
    // SAFETY: Valid `sys_ctx` and `sys_data`.
    assert_eq!(unsafe { sys_fn(sys_ctx, sys_data) }, FfiResult::OK);

    /// Shared slot storing `(BamRunIfFn, user_data)` so the predicate can re-enter itself.
    #[derive(Clone, Copy)]
    struct RunIfSlot {
        func: Option<BamRunIfFn>,
        data: SendVoidPtr,
    }
    let pred_slot = std::sync::Arc::new(std::sync::Mutex::new(RunIfSlot {
        func: None,
        data: SendVoidPtr(core::ptr::null_mut()),
    }));
    let pred_slot_inner = std::sync::Arc::clone(&pred_slot);
    let (run_if_fn, run_if_data) = allocate_run_if_trampoline(&mut instance, move |_host| {
        let copy = *pred_slot_inner.lock().unwrap();
        let nested = copy.func.unwrap();
        // SAFETY: Invoking `run_if_fn` re-entrantly with its own `user_data`.
        let nested_bool = unsafe { nested(copy.data.0) };
        assert_eq!(nested_bool, FfiBool::FALSE);
        true
    });
    {
        let mut guard = pred_slot.lock().unwrap();
        guard.func = Some(run_if_fn);
        guard.data = SendVoidPtr(run_if_data);
    }
    // SAFETY: Valid `run_if_data`.
    assert_eq!(unsafe { run_if_fn(run_if_data) }, FfiBool::TRUE);
}

/// Verifies `Host::last_error`, `"bam.v0.*"` namespace enforcement, custom function pointer
/// resolution, and [`VTableHeader`] validation under Miri.
///
/// Confirms that non-`"bam.v0."` symbols return `INVALID_ARGUMENT`, valid VTables resolve and
/// execute, wrong UUIDs return `TYPE_MISMATCH`, and truncated or ABI-0.1 VTables return
/// `INVALID_VERSION` before forming `&DummyVTable`.
#[test]
fn vtable_header_and_custom_symbol_validation_under_miri() {
    let api = make_mock_api();
    let mut world = MockWorldCtx {
        error_msg: b"sample host error",
        spawned: 0,
        last_inserted_val: 0,
    };
    let host_ctx = (&raw mut world).cast::<BamHostCtx>();
    // SAFETY: Valid stack `api` and `host_ctx`.
    let host = unsafe { Host::from_raw(&api, host_ctx) }.unwrap();
    assert_eq!(host.last_error(), "sample host error");

    assert_eq!(
        host.resolve_custom_symbol(c"bamabi_iter_reset"),
        Err(FfiResult::INVALID_ARGUMENT)
    );
    // SAFETY: `custom_math_fn` has signature `unsafe extern "C" fn(u32) -> u32`.
    let add10: unsafe extern "C" fn(u32) -> u32 =
        unsafe { host.resolve_custom_fn(c"bam.v0.math.add10") }.unwrap();
    // SAFETY: Pure integer addition.
    assert_eq!(unsafe { add10(32) }, 42);

    // SAFETY: `VALID_VTABLE` is a static `DummyVTable` prefixed by `VTableHeader`.
    let vt = unsafe {
        host.resolve_vtable::<DummyVTable>(c"bam.v0.mock::Dummy.vtable", VALID_VTABLE_UUID)
    }
    .unwrap();
    // SAFETY: `dummy_getter` ignores its pointer argument.
    assert_eq!(unsafe { (vt.getter.unwrap())(core::ptr::null()) }, 42);

    let wrong_uuid = ComponentUuid::from_u128(0xDEAD_BEEF);
    // SAFETY: Testing header rejection path.
    let wrong_uuid_res =
        unsafe { host.resolve_vtable::<DummyVTable>(c"bam.v0.mock::Dummy.vtable", wrong_uuid) };
    assert_eq!(wrong_uuid_res.err(), Some(FfiResult::TYPE_MISMATCH));

    // SAFETY: `read_descriptor::<VTableHeader>` reads only `VTableHeader` and rejects before `&DummyVTable`.
    let truncated_res = unsafe {
        host.resolve_vtable::<DummyVTable>(c"bam.v0.mock::Truncated.vtable", VALID_VTABLE_UUID)
    };
    assert_eq!(truncated_res.err(), Some(FfiResult::INVALID_VERSION));

    // SAFETY: Valid `DummyVTable` allocation with 0.1 version header.
    let wrong_ver_res = unsafe {
        host.resolve_vtable::<DummyVTable>(c"bam.v0.mock::WrongVer.vtable", VALID_VTABLE_UUID)
    };
    assert_eq!(wrong_ver_res.err(), Some(FfiResult::INVALID_VERSION));
}

/// Verifies Miri soundness of [`SparseRows`]/[`SparseRowsMut`] row iteration and
/// [`ValidatedCommands::spawn`]/[`ValidatedCommands::insert`] on `PaddedPod` structs containing
/// uninitialized alignment padding bytes.
#[test]
fn sparse_rows_and_padded_commands_sound_under_miri() {
    let mut r0 = 10u32;
    let mut r1 = 20u32;
    let mut r2 = 30u32;
    let ro_ptrs: [*const u8; 3] = [
        (&raw const r0).cast::<u8>(),
        (&raw const r1).cast::<u8>(),
        (&raw const r2).cast::<u8>(),
    ];
    // SAFETY: `ro_ptrs` points to 3 live `u32` values.
    let rows = unsafe { SparseRows::<u32>::from_raw_parts(ro_ptrs.as_ptr(), 3, false) }.unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows.get(0).copied(), Some(10));
    assert_eq!(rows.get(2).copied(), Some(30));
    let collected: Vec<u32> = rows.iter().copied().collect();
    assert_eq!(collected, vec![10, 20, 30]);

    let mut_ptrs: [*mut u8; 3] = [
        (&raw mut r0).cast::<u8>(),
        (&raw mut r1).cast::<u8>(),
        (&raw mut r2).cast::<u8>(),
    ];
    // SAFETY: `mut_ptrs` points to 3 disjoint mutable `u32` values.
    let mut rows_mut =
        unsafe { SparseRowsMut::<u32>::from_raw_parts(mut_ptrs.as_ptr(), 3, false) }.unwrap();
    rows_mut.for_each_mut(|idx, val| {
        *val += (idx as u32) + 1;
    });
    assert_eq!([r0, r1, r2], [11, 22, 33]);

    // Verify `PaddedPod` insert & batched spawn preserve uninitialized padding without UB.
    let mut api = make_mock_api();
    api.component_layout_info = Some(stub_component_layout_info_padded);
    let mut world = MockWorldCtx {
        error_msg: b"",
        spawned: 0,
        last_inserted_val: 0,
    };
    let host_ctx = (&raw mut world).cast::<BamHostCtx>();
    // SAFETY: Valid `api` and `host_ctx`.
    let host = unsafe { Host::from_raw(&api, host_ctx) }.unwrap();
    let raw_cmds = (&raw mut world).cast::<BamCommands>();
    // SAFETY: `raw_cmds` points to `world` for mock command callbacks.
    let mut cmds = unsafe { ValidatedCommands::from_raw(raw_cmds, &host) };
    let padded = PaddedPod { tag: 7, val: 999 };
    let comp = BamComponent::from_raw(1);
    let e = cmds.spawn(&[SpawnEntry::new(comp, &padded)]).unwrap();
    assert_eq!(e.to_bits(), 1);
    assert_eq!(world.last_inserted_val, 999);

    let padded2 = PaddedPod { tag: 3, val: 1234 };
    assert_eq!(cmds.insert(e, comp, &padded2), FfiResult::OK);
    assert_eq!(world.last_inserted_val, 1234);
}

/// Verifies per-World [`GuestInstance`] isolation, LIFO shutdown hook execution, null-shutdown
/// no-op behavior, and transactional rollback on failed `bamabi_init`.
///
/// Scenario: initializes two independent instances (World A and World B), shuts down World A and
/// verifies World B's allocations remain alive, then triggers a failing `init_guest_instance` and
/// verifies `*instance_out` is nulled, staged allocations drop, and unrun shutdown hooks do not fire.
#[test]
fn multi_world_guest_instance_isolation_and_rollback() {
    static DROPS_A: AtomicUsize = AtomicUsize::new(0);
    static DROPS_B: AtomicUsize = AtomicUsize::new(0);
    static HOOKS_A: AtomicUsize = AtomicUsize::new(0);

    /// Drop probe incrementing an atomic counter when reclaimed by [`GuestInstance`].
    struct DropCounter(&'static AtomicUsize);
    impl Drop for DropCounter {
        /// Records one drop event.
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let api = make_mock_api();
    let mut world_a = MockWorldCtx {
        error_msg: b"",
        spawned: 0,
        last_inserted_val: 0,
    };
    let mut world_b = MockWorldCtx {
        error_msg: b"",
        spawned: 0,
        last_inserted_val: 0,
    };
    let manifest = BamManifest::new(c"multi_mod".as_ptr(), c"0.2.0".as_ptr());

    let mut inst_a: *mut BamInstance = core::ptr::null_mut();
    let mut inst_b: *mut BamInstance = core::ptr::null_mut();

    // SAFETY: Valid mock pointers for World A.
    let status_a = unsafe {
        init_guest_instance(
            &api,
            (&raw mut world_a).cast::<BamLoadCtx>(),
            &mut inst_a,
            &manifest,
            |inst, _load| {
                let _ = inst.track_allocation(Box::new(DropCounter(&DROPS_A)));
                inst.add_shutdown_hook(|| {
                    HOOKS_A.fetch_add(1, Ordering::SeqCst);
                });
                FfiResult::OK
            },
        )
    };
    assert_eq!(status_a, FfiResult::OK);
    assert!(!inst_a.is_null());

    // SAFETY: Valid mock pointers for World B.
    let status_b = unsafe {
        init_guest_instance(
            &api,
            (&raw mut world_b).cast::<BamLoadCtx>(),
            &mut inst_b,
            &manifest,
            |inst, _load| {
                let _ = inst.track_allocation(Box::new(DropCounter(&DROPS_B)));
                FfiResult::OK
            },
        )
    };
    assert_eq!(status_b, FfiResult::OK);
    assert!(!inst_b.is_null());
    assert_ne!(inst_a, inst_b);

    // SAFETY: `inst_a` came from `init_guest_instance`.
    unsafe { shutdown_guest_instance(inst_a) };
    assert_eq!(HOOKS_A.load(Ordering::SeqCst), 1);
    assert_eq!(DROPS_A.load(Ordering::SeqCst), 1);
    assert_eq!(DROPS_B.load(Ordering::SeqCst), 0);

    // SAFETY: Null is explicitly allowed.
    unsafe { shutdown_guest_instance(core::ptr::null_mut()) };

    // SAFETY: `inst_b` came from `init_guest_instance`.
    unsafe { shutdown_guest_instance(inst_b) };
    assert_eq!(DROPS_B.load(Ordering::SeqCst), 1);

    static ROLLBACK_DROPS: AtomicUsize = AtomicUsize::new(0);
    static ROLLBACK_HOOKS: AtomicUsize = AtomicUsize::new(0);
    let mut failed_inst: *mut BamInstance = core::ptr::dangling_mut();
    // SAFETY: Valid mock pointers; `failed_inst` must be reset to null on error.
    let status = unsafe {
        init_guest_instance(
            &api,
            (&raw mut world_a).cast::<BamLoadCtx>(),
            &mut failed_inst,
            &manifest,
            |inst, _load| {
                let _ = inst.track_allocation(Box::new(DropCounter(&ROLLBACK_DROPS)));
                inst.add_shutdown_hook(|| {
                    ROLLBACK_HOOKS.fetch_add(1, Ordering::SeqCst);
                });
                FfiResult::PERMISSION_DENIED
            },
        )
    };
    assert_eq!(status, FfiResult::PERMISSION_DENIED);
    assert!(failed_inst.is_null());
    assert_eq!(ROLLBACK_DROPS.load(Ordering::SeqCst), 1);
    assert_eq!(ROLLBACK_HOOKS.load(Ordering::SeqCst), 0);

    let _ = BAM_STORAGE_FLAG_SPARSE_SET;
    let _ = AccessMode::READ;
}

/// Verifies runtime observer allocation tracking in [`RuntimeAllocTracker`] and graceful
/// `FfiResult::UNSUPPORTED` fallbacks when optional ABI `0.3` host slots (`asset_load`,
/// `sys_entity_children`) are unpopulated.
///
/// Confirms that a closure captured by `ValidatedCommands::observe` stays alive after
/// `init_guest_instance` returns and drops exactly once during `shutdown_guest_instance`.
#[test]
fn runtime_observer_alloc_tracking_and_unsupported_fallbacks() {
    static OBS_DROPS: AtomicUsize = AtomicUsize::new(0);
    /// Drop probe captured inside the runtime observer closure.
    struct ObsDropGuard;
    impl Drop for ObsDropGuard {
        /// Increments `OBS_DROPS` when the runtime observer closure is reclaimed.
        fn drop(&mut self) {
            OBS_DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }

    let mut api = make_mock_api();
    /// Mock `cmd_observe` validating the descriptor and returning entity `77`.
    unsafe extern "C" fn mock_cmd_observe(
        _cmds: *mut BamCommands,
        desc: *const bamabi_sys::BamObserverDesc,
        out_entity: *mut FfiEntity,
    ) -> FfiResult {
        if desc.is_null() || out_entity.is_null() {
            return FfiResult::NULL_POINTER;
        }
        // SAFETY: `desc` and `out_entity` are valid in this unit test.
        unsafe {
            let st = (*desc).validate();
            if !st.is_ok() {
                return st;
            }
            *out_entity = FfiEntity::from_bits(77);
        }
        FfiResult::OK
    }
    api.cmd_observe = Some(mock_cmd_observe);

    let mut world = MockWorldCtx {
        error_msg: b"",
        spawned: 0,
        last_inserted_val: 0,
    };
    let manifest = BamManifest::new(c"obs_mod".as_ptr(), c"0.2.0".as_ptr());
    let mut raw_inst: *mut BamInstance = core::ptr::null_mut();

    // SAFETY: Valid mock pointers.
    let status = unsafe {
        init_guest_instance(
            &api,
            (&raw mut world).cast::<BamLoadCtx>(),
            &mut raw_inst,
            &manifest,
            |inst, _load| {
                let host = *inst.host();
                assert_eq!(
                    host.asset_load(BamComponent::from_raw(1), "mods://a.png"),
                    Err(FfiResult::UNSUPPORTED)
                );
                assert_eq!(
                    host.asset_load(BamComponent::INVALID, "mods://untyped.bsn"),
                    Err(FfiResult::UNSUPPORTED)
                );
                let inv =
                    ValidatedSystemInvocation::from_raw((&raw mut world).cast(), &host, 0, 0, None);
                assert_eq!(
                    inv.system_ref()
                        .unwrap()
                        .entity_children(FfiEntity::from_bits(5), BamComponent::INVALID),
                    Err(FfiResult::UNSUPPORTED)
                );
                let mut cmds = ValidatedCommands::from_raw((&raw mut world).cast(), &host);
                let guard = ObsDropGuard;
                let obs_cfg = ObserverRegistrationConfig::custom(
                    Some("runtime_obs"),
                    BamComponent::from_raw(3),
                );
                let ent = cmds
                    .observe::<u32, _>(&obs_cfg, move |_e, _val, _c| {
                        let _ = &guard;
                        FfiResult::OK
                    })
                    .unwrap();
                assert_eq!(ent.to_bits(), 77);
                assert_eq!(inst.allocation_count(), 2);
                FfiResult::OK
            },
        )
    };
    assert_eq!(status, FfiResult::OK);
    assert_eq!(OBS_DROPS.load(Ordering::SeqCst), 0);

    // SAFETY: `raw_inst` was produced by `init_guest_instance`.
    unsafe { shutdown_guest_instance(raw_inst) };
    assert_eq!(OBS_DROPS.load(Ordering::SeqCst), 1);
}

/// Verifies that `system_closure_trampoline` refreshes the invocation's [`Host::host_ctx`] from
/// `BamHostApi::sys_host_ctx(sys_ctx)` rather than reusing a stale init-time context pointer.
#[test]
fn system_closure_trampoline_refreshes_host_ctx_via_sys_host_ctx() {
    static OBSERVED_ADDRS: AtomicUsize = AtomicUsize::new(0);
    static EXPECTED_RUNTIME_PTR: AtomicPtr<BamHostCtx> = AtomicPtr::new(core::ptr::null_mut());

    /// Mock `sys_host_ctx` returning the runtime world context pointer.
    unsafe extern "C" fn runtime_sys_host_ctx(_sys_ctx: *const BamSystemCtx) -> *const BamHostCtx {
        EXPECTED_RUNTIME_PTR.load(Ordering::SeqCst).cast_const()
    }

    let mut init_ctx_marker = 11u8;
    let mut runtime_ctx_marker = 22u8;
    let init_ctx_ptr = (&raw mut init_ctx_marker).cast::<BamHostCtx>().cast_const();
    let runtime_ctx_mut = (&raw mut runtime_ctx_marker).cast::<BamHostCtx>();
    EXPECTED_RUNTIME_PTR.store(runtime_ctx_mut, Ordering::SeqCst);

    let mut api = make_mock_api();
    api.sys_host_ctx = Some(runtime_sys_host_ctx);

    // SAFETY: `api` and `init_ctx_ptr` are valid stack addresses for the test duration.
    let init_host = unsafe { Host::from_raw(&raw const api, init_ctx_ptr) }.unwrap();
    let mut instance = GuestInstance::new(init_host, "ctx_refresh_mod");

    let (sys_fn, user_data) = allocate_system_trampoline(&mut instance, 0, 0, None, |inv| {
        let observed = inv.host().host_ctx().addr();
        OBSERVED_ADDRS.store(observed, Ordering::SeqCst);
        assert_eq!(inv.commands_mut().host().host_ctx().addr(), observed);
        FfiResult::OK
    });

    let mut dummy_sys_ctx = 99u8;
    // SAFETY: `dummy_sys_ctx` and `user_data` are valid for this synchronous system call.
    let rc = unsafe { sys_fn((&raw mut dummy_sys_ctx).cast::<BamSystemCtx>(), user_data) };
    assert_eq!(rc, FfiResult::OK);
    assert_eq!(
        OBSERVED_ADDRS.load(Ordering::SeqCst),
        runtime_ctx_mut.addr()
    );
    assert_ne!(OBSERVED_ADDRS.load(Ordering::SeqCst), init_ctx_ptr.addr());
}

/// Verifies `ValidatedLoadCtx::register_dynamic_mod_component` symbol registration (canonical +
/// manifest-qualified VTable and `init_default` trampoline) and confirms that failed
/// `insert_moved`/`spawn_moved` calls drop the guest-owned non-POD value cleanly without leaking.
#[test]
fn register_dynamic_mod_component_symbols_and_moved_command_error_drops_cleanly() {
    static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

    /// Non-POD test component with heap `String` and drop counter.
    #[derive(Default)]
    struct DynMockComp {
        label: String,
    }

    impl Drop for DynMockComp {
        /// Records each destructor invocation on the guest heap.
        fn drop(&mut self) {
            let _ = self.label.len();
            DROP_COUNT.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Header-only VTable fixture for `DynMockComp`.
    #[repr(C)]
    #[derive(Copy, Clone)]
    struct DynMockVTable {
        header: VTableHeader,
    }

    static DYN_MOCK_UUID: ComponentUuid = ComponentUuid::from_canonical_name("DynMockComp");
    static DYN_MOCK_VT: DynMockVTable = DynMockVTable {
        header: VTableHeader::from_raw_parts(
            core::mem::size_of::<DynMockVTable>() as u32,
            DYN_MOCK_UUID,
            core::mem::size_of::<DynMockComp>(),
            core::mem::align_of::<DynMockComp>(),
        ),
    };
    static DYN_MOCK_SYMS: [SymbolEntry; 1] = [SymbolEntry {
        name: "bam.v0.DynMockComp.vtable",
        ptr: (&raw const DYN_MOCK_VT).cast::<c_void>(),
    }];

    impl ModComponent for DynMockComp {
        const CANONICAL_NAME: &'static str = "DynMockComp";
        const COMPONENT_CSTR: &'static CStr = c"DynMockComp";
        const UUID: ComponentUuid = DYN_MOCK_UUID;
        type VTable = DynMockVTable;
        /// Returns the static `DynMockVTable`.
        fn vtable() -> &'static Self::VTable {
            &DYN_MOCK_VT
        }
        /// Returns the exported VTable symbol table.
        fn symbols() -> &'static [SymbolEntry] {
            &DYN_MOCK_SYMS
        }
    }

    DROP_COUNT.store(0, Ordering::SeqCst);
    let mut dummy_world = 0u64;
    let api = make_mock_api();
    // SAFETY: `api` and `dummy_world` stack pointers remain valid for this test.
    let host = unsafe { Host::from_raw(&raw const api, (&raw const dummy_world).cast()) }.unwrap();
    let mut instance = GuestInstance::new(host, "dyn_mod");
    let host_copy = *instance.host();
    // SAFETY: `dummy_world` acts as an opaque non-null mock load context.
    let load_ctx =
        unsafe { ValidatedLoadCtx::from_raw((&raw mut dummy_world).cast(), &host_copy) }.unwrap();

    let handle = load_ctx
        .register_dynamic_mod_component::<DynMockComp>(&mut instance, 0)
        .expect("register_dynamic_mod_component succeeds");
    assert!(handle.is_valid());

    let inst_ptr = (&raw const instance).cast::<BamInstance>();
    // SAFETY: `inst_ptr` points to live `instance` and symbol names are NUL-terminated.
    let vt_canon =
        unsafe { resolve_guest_instance_symbol(inst_ptr, c"bam.v0.DynMockComp.vtable".as_ptr()) };
    // SAFETY: Same as above.
    let vt_qual = unsafe {
        resolve_guest_instance_symbol(inst_ptr, c"bam.v0.dyn_mod::DynMockComp.vtable".as_ptr())
    };
    assert!(core::ptr::eq(vt_canon, DynMockComp::erased_vtable_ptr()));
    assert!(core::ptr::eq(vt_qual, DynMockComp::erased_vtable_ptr()));

    // SAFETY: Same as above.
    let init_ptr = unsafe {
        resolve_guest_instance_symbol(
            inst_ptr,
            c"bam.v0.dyn_mod::DynMockComp.init_default".as_ptr(),
        )
    };
    assert!(!init_ptr.is_null());
    // SAFETY: `init_default` symbol has signature `unsafe extern "C" fn(*mut c_void) -> FfiResult`.
    let init_fn: unsafe extern "C" fn(*mut c_void) -> FfiResult =
        unsafe { core::mem::transmute(init_ptr) };
    let mut uninit_slot = core::mem::MaybeUninit::<DynMockComp>::uninit();
    // SAFETY: `uninit_slot` has `DynMockComp` layout and alignment.
    let init_status = unsafe { init_fn(uninit_slot.as_mut_ptr().cast::<c_void>()) };
    assert_eq!(init_status, FfiResult::OK);
    // SAFETY: `init_fn` returned `FfiResult::OK`, initializing `uninit_slot`.
    unsafe { uninit_slot.assume_init_drop() };
    assert_eq!(DROP_COUNT.load(Ordering::SeqCst), 1);

    // Failed `insert_moved` / `spawn_moved` on an empty/null command buffer must drop `T` cleanly!
    let mut empty_cmds = ValidatedCommands::empty(instance.host());
    let rc = empty_cmds.insert_moved(
        FfiEntity::from_bits(1),
        handle,
        DynMockComp {
            label: String::from("dropped_on_insert_err"),
        },
    );
    assert_eq!(rc, FfiResult::NULL_POINTER);
    assert_eq!(DROP_COUNT.load(Ordering::SeqCst), 2);

    let spawn_res = empty_cmds.spawn_moved(
        handle,
        DynMockComp {
            label: String::from("dropped_on_spawn_err"),
        },
        &[],
        false,
    );
    assert_eq!(spawn_res, Err(FfiResult::NULL_POINTER));
    assert_eq!(DROP_COUNT.load(Ordering::SeqCst), 3);
}

/// Verifies that `system_closure_trampoline` defers both `sys_host_ctx` and `sys_commands` until
/// actually requested, caches `sys_host_ctx` at most once per tick (reusing the init-time `Host`
/// when `sys_host_ctx` returns null), and enforces single-take exclusivity on `take_lazy_commands`.
#[test]
fn system_closure_trampoline_defers_sys_host_ctx_and_sys_commands_until_accessed() {
    static HOST_CTX_CALLS: AtomicUsize = AtomicUsize::new(0);
    static COMMANDS_CALLS: AtomicUsize = AtomicUsize::new(0);
    static RUNTIME_CTX_PTR: AtomicPtr<BamHostCtx> = AtomicPtr::new(core::ptr::null_mut());
    static DUMMY_CMDS_PTR: AtomicPtr<BamCommands> = AtomicPtr::new(core::ptr::null_mut());

    /// Counting mock `sys_host_ctx` returning `RUNTIME_CTX_PTR`.
    unsafe extern "C" fn counting_sys_host_ctx(_sys_ctx: *const BamSystemCtx) -> *const BamHostCtx {
        HOST_CTX_CALLS.fetch_add(1, Ordering::SeqCst);
        RUNTIME_CTX_PTR.load(Ordering::SeqCst).cast_const()
    }

    /// Counting mock `sys_commands` returning `DUMMY_CMDS_PTR`.
    unsafe extern "C" fn counting_sys_commands(
        _sys_ctx: *mut BamSystemCtx,
        out_cmds: *mut *mut BamCommands,
    ) -> FfiResult {
        COMMANDS_CALLS.fetch_add(1, Ordering::SeqCst);
        // SAFETY: `out_cmds` is non-null writable stack storage passed by `ValidatedSystemInvocation`.
        unsafe { *out_cmds = DUMMY_CMDS_PTR.load(Ordering::SeqCst) };
        FfiResult::OK
    }

    HOST_CTX_CALLS.store(0, Ordering::SeqCst);
    COMMANDS_CALLS.store(0, Ordering::SeqCst);

    let mut init_marker = 1u8;
    let mut runtime_marker = 2u8;
    let mut cmds_marker = 3u8;
    let init_ctx = (&raw mut init_marker).cast::<BamHostCtx>().cast_const();
    let runtime_ctx = (&raw mut runtime_marker).cast::<BamHostCtx>();
    let cmds_ptr = (&raw mut cmds_marker).cast::<BamCommands>();
    RUNTIME_CTX_PTR.store(runtime_ctx, Ordering::SeqCst);
    DUMMY_CMDS_PTR.store(cmds_ptr, Ordering::SeqCst);

    let mut api = make_mock_api();
    api.sys_host_ctx = Some(counting_sys_host_ctx);
    api.sys_commands = Some(counting_sys_commands);

    // SAFETY: `api` and `init_ctx` are valid stack objects for this test.
    let init_host = unsafe { Host::from_raw(&raw const api, init_ctx) }.unwrap();
    let mut instance = GuestInstance::new(init_host, "lazy_mod");

    // 1. Noop system never calls `sys_host_ctx` or `sys_commands`.
    let (noop_fn, noop_ud) =
        allocate_system_trampoline(&mut instance, 0, 0, None, |_inv| FfiResult::OK);
    let mut dummy_sys = 0u64;
    let sys_ctx = (&raw mut dummy_sys).cast::<BamSystemCtx>();
    // SAFETY: `sys_ctx` and `noop_ud` are valid for this synchronous call.
    assert_eq!(unsafe { noop_fn(sys_ctx, noop_ud) }, FfiResult::OK);
    assert_eq!(HOST_CTX_CALLS.load(Ordering::SeqCst), 0);
    assert_eq!(COMMANDS_CALLS.load(Ordering::SeqCst), 0);

    // 2. Calling `inv.host()` repeatedly resolves `sys_host_ctx` once per tick, and
    //    `take_lazy_commands()` defers `sys_commands` until `.resolve()` while enforcing
    //    single-take exclusivity (S2).
    let expected_runtime_addr = runtime_ctx.addr();
    let (lazy_fn, lazy_ud) = allocate_system_trampoline(&mut instance, 0, 0, None, move |inv| {
        let lazy_cmds = inv.take_lazy_commands();
        // Taking the lazy token must NOT have called `sys_commands` or `sys_host_ctx` yet!
        assert_eq!(HOST_CTX_CALLS.load(Ordering::SeqCst), 0);
        assert_eq!(COMMANDS_CALLS.load(Ordering::SeqCst), 0);

        // Single-take exclusivity: second take and `commands_mut()` must return disabled buffers.
        assert!(!inv.take_lazy_commands().resolve().is_valid());
        assert!(!inv.take_commands().is_valid());
        assert!(!inv.commands_mut().is_valid());
        assert_eq!(COMMANDS_CALLS.load(Ordering::SeqCst), 0);

        // Repeated `inv.host()` calls resolve `sys_host_ctx` at most once per tick.
        assert_eq!(inv.host().host_ctx().addr(), expected_runtime_addr);
        assert_eq!(inv.host().host_ctx().addr(), expected_runtime_addr);
        assert_eq!(HOST_CTX_CALLS.load(Ordering::SeqCst), 1);

        // Resolving `lazy_cmds` calls `sys_commands` once and reuses the cached refreshed `Host`.
        let cmds = lazy_cmds.resolve();
        assert!(cmds.is_valid());
        assert_eq!(cmds.host().host_ctx().addr(), expected_runtime_addr);
        assert_eq!(HOST_CTX_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(COMMANDS_CALLS.load(Ordering::SeqCst), 1);
        FfiResult::OK
    });
    // SAFETY: `sys_ctx` and `lazy_ud` are valid for this synchronous call.
    assert_eq!(unsafe { lazy_fn(sys_ctx, lazy_ud) }, FfiResult::OK);

    // 3. When `sys_host_ctx` returns null, `inv.host()` falls back to the init-time `host_ctx`.
    RUNTIME_CTX_PTR.store(core::ptr::null_mut(), Ordering::SeqCst);
    let expected_init_addr = init_ctx.addr();
    let (fallback_fn, fallback_ud) =
        allocate_system_trampoline(&mut instance, 0, 0, None, move |inv| {
            assert_eq!(inv.host().host_ctx().addr(), expected_init_addr);
            FfiResult::OK
        });
    // SAFETY: `sys_ctx` and `fallback_ud` are valid for this synchronous call.
    assert_eq!(unsafe { fallback_fn(sys_ctx, fallback_ud) }, FfiResult::OK);
}

/// Verifies that [`ValidatedIter::column_info`] caches successful `BamColumnInfo` lookups for
/// inline-cached terms (`0..8`) across repeated chunk/resource accesses while still forwarding
/// higher term indices (`>= 8`) and never caching error results.
#[test]
fn validated_iter_caches_column_info_per_term_across_repeated_lookups() {
    static INFO_CALLS: AtomicUsize = AtomicUsize::new(0);

    /// Counting mock `iter_column_info` returning `OK` for `term_index < 10` and `OUT_OF_RANGE` otherwise.
    unsafe extern "C" fn counting_column_info(
        _iter: *const bamabi_sys::BamIter,
        term_index: usize,
        out_info: *mut bamabi_sys::BamColumnInfo,
    ) -> FfiResult {
        INFO_CALLS.fetch_add(1, Ordering::SeqCst);
        if term_index >= 10 {
            return FfiResult::OUT_OF_RANGE;
        }
        // SAFETY: `out_info` is writable stack storage supplied by `ValidatedIter::column_info`.
        unsafe {
            *out_info = bamabi_sys::BamColumnInfo {
                component: BamComponent::from_raw((term_index + 1) as u32),
                mode: AccessMode::READ,
                uuid: ComponentUuid::ZERO,
                size: 4,
                align: 4,
                storage_flags: 0,
                reserved0: 0,
            };
        }
        FfiResult::OK
    }

    INFO_CALLS.store(0, Ordering::SeqCst);
    let mut api = make_mock_api();
    api.iter_column_info = Some(counting_column_info);

    let mut dummy_ctx = 1u8;
    let mut dummy_iter = 2u8;
    // SAFETY: Stack pointers stay valid for the duration of this test.
    let host = unsafe { Host::from_raw(&raw const api, (&raw mut dummy_ctx).cast()) }.unwrap();
    let mut scratch = bamabi_guest_sys::QuerySlotScratch::new();
    // SAFETY: `dummy_iter` acts as an opaque non-null `BamIter` for the mock callback.
    let iter = unsafe {
        ValidatedIter::from_raw_with_scratch((&raw mut dummy_iter).cast(), &host, &mut scratch)
    };

    // Repeated lookups on inline-cached terms `0` and `1` invoke FFI only once each.
    for _ in 0..8 {
        assert_eq!(iter.column_info(0).unwrap().component.0, 1);
        assert_eq!(iter.column_info(1).unwrap().component.0, 2);
        assert_eq!(iter.column_info(7).unwrap().component.0, 8);
    }
    assert_eq!(INFO_CALLS.load(Ordering::SeqCst), 3);

    // Term index `9` (beyond the 8-slot inline cache) still queries FFI cleanly.
    assert_eq!(iter.column_info(9).unwrap().component.0, 10);
    assert_eq!(iter.column_info(9).unwrap().component.0, 10);
    assert_eq!(INFO_CALLS.load(Ordering::SeqCst), 5);

    // Error results (`OUT_OF_RANGE`) are never cached.
    assert_eq!(iter.column_info(11), Err(FfiResult::OUT_OF_RANGE));
    assert_eq!(iter.column_info(11), Err(FfiResult::OUT_OF_RANGE));
    assert_eq!(INFO_CALLS.load(Ordering::SeqCst), 7);

    // Bare `ValidatedIter::from_raw` without scratch queries FFI cleanly without caching.
    // SAFETY: `dummy_iter` acts as an opaque non-null `BamIter` for the mock callback.
    let uncached_iter = unsafe { ValidatedIter::from_raw((&raw mut dummy_iter).cast(), &host) };
    assert_eq!(uncached_iter.column_info(0).unwrap().component.0, 1);
    assert_eq!(uncached_iter.column_info(0).unwrap().component.0, 1);
    assert_eq!(INFO_CALLS.load(Ordering::SeqCst), 9);
}

/// Verifies that `BamHostApi::iter_next_chunk_batch` (slot 68) fetches chunk length, entities,
/// and all dense/optional/`HAS` column pointers in a single FFI call without invoking
/// `iter_next_chunk` / `iter_entities` / `iter_column_readonly` / `iter_column_mutable`, still
/// enforces overlap rejection on hostile batch pointers, and falls back cleanly on older
/// `MIN_V0_3_SIZE` hosts where slot 68 is uncovered.
#[test]
fn validated_iter_advance_batch_uses_slot_68_and_falls_back_on_v03_prefix() {
    use bamabi_guest_sys::ChunkColumnDispenser;

    struct MockBatchState {
        chunks_left: usize,
        entities: [FfiEntity; 2],
        col0_write: [u32; 2],
        col1_read: [u32; 2],
        has_flag: FfiBool,
        hostile_overlap: bool,
        batch_calls: usize,
        legacy_next_calls: usize,
        legacy_ent_calls: usize,
        legacy_ro_calls: usize,
        legacy_rw_calls: usize,
        last_mark_mask: u32,
    }

    unsafe extern "C" fn mock_col_info(
        _iter: *const bamabi_sys::BamIter,
        term_index: usize,
        out_info: *mut bamabi_sys::BamColumnInfo,
    ) -> FfiResult {
        let mode = match term_index {
            0 => AccessMode::WRITE,
            1 => AccessMode::READ,
            2 => AccessMode::OPTIONAL_READ,
            3 => AccessMode::HAS,
            _ => return FfiResult::OUT_OF_RANGE,
        };
        let (size, align) = if term_index == 3 { (1, 1) } else { (4, 4) };
        // SAFETY: `out_info` is writable stack storage.
        unsafe {
            *out_info = bamabi_sys::BamColumnInfo {
                component: BamComponent::from_raw((term_index + 1) as u32),
                mode,
                uuid: ComponentUuid::ZERO,
                size,
                align,
                storage_flags: 0,
                reserved0: 0,
            };
        }
        FfiResult::OK
    }

    unsafe extern "C" fn mock_next_chunk_batch(
        iter: *mut bamabi_sys::BamIter,
        out_len: *mut usize,
        out_entities: *mut *const FfiEntity,
        out_col_ptrs: *mut *mut u8,
        col_count: usize,
        mark_changed_mask: u32,
    ) -> FfiBool {
        // SAFETY: `iter` points to `MockBatchState`.
        let st = unsafe { &mut *iter.cast::<MockBatchState>() };
        st.batch_calls += 1;
        st.last_mark_mask = mark_changed_mask;
        if st.chunks_left == 0 {
            // SAFETY: `out_len` is writable.
            unsafe { *out_len = 0 };
            return FfiBool::FALSE;
        }
        st.chunks_left -= 1;
        // SAFETY: Output pointers are valid per `BamIterNextChunkBatchFn` contract.
        unsafe {
            *out_len = st.entities.len();
            if !out_entities.is_null() {
                *out_entities = st.entities.as_ptr();
            }
            let col0_ptr = (&raw mut st.col0_write).cast::<u8>();
            if col_count >= 1 && !out_col_ptrs.is_null() {
                *out_col_ptrs.add(0) = col0_ptr;
            }
            if col_count >= 2 && !out_col_ptrs.is_null() {
                *out_col_ptrs.add(1) = if st.hostile_overlap {
                    col0_ptr
                } else {
                    (&raw const st.col1_read).cast::<u8>().cast_mut()
                };
            }
            if col_count >= 3 && !out_col_ptrs.is_null() {
                *out_col_ptrs.add(2) = core::ptr::null_mut();
            }
            if col_count >= 4 && !out_col_ptrs.is_null() {
                *out_col_ptrs.add(3) = (&raw mut st.has_flag).cast::<u8>();
            }
        }
        FfiBool::TRUE
    }

    unsafe extern "C" fn mock_legacy_next(
        iter: *mut bamabi_sys::BamIter,
        out_len: *mut usize,
    ) -> FfiBool {
        // SAFETY: `iter` points to `MockBatchState`.
        let st = unsafe { &mut *iter.cast::<MockBatchState>() };
        st.legacy_next_calls += 1;
        if st.chunks_left == 0 {
            // SAFETY: `out_len` is writable.
            unsafe { *out_len = 0 };
            return FfiBool::FALSE;
        }
        st.chunks_left -= 1;
        // SAFETY: `out_len` is writable.
        unsafe { *out_len = st.entities.len() };
        FfiBool::TRUE
    }

    unsafe extern "C" fn mock_legacy_entities(
        iter: *const bamabi_sys::BamIter,
    ) -> *const FfiEntity {
        // SAFETY: `iter` points to `MockBatchState`.
        let st = unsafe { &mut *iter.cast::<MockBatchState>().cast_mut() };
        st.legacy_ent_calls += 1;
        st.entities.as_ptr()
    }

    unsafe extern "C" fn mock_legacy_ro(
        iter: *const bamabi_sys::BamIter,
        term_index: usize,
    ) -> *const u8 {
        // SAFETY: `iter` points to `MockBatchState`.
        let st = unsafe { &mut *iter.cast::<MockBatchState>().cast_mut() };
        st.legacy_ro_calls += 1;
        match term_index {
            1 => st.col1_read.as_ptr().cast::<u8>(),
            3 => (&raw const st.has_flag).cast::<u8>(),
            _ => core::ptr::null(),
        }
    }

    unsafe extern "C" fn mock_legacy_rw(
        iter: *mut bamabi_sys::BamIter,
        term_index: usize,
    ) -> *mut u8 {
        // SAFETY: `iter` points to `MockBatchState`.
        let st = unsafe { &mut *iter.cast::<MockBatchState>() };
        st.legacy_rw_calls += 1;
        if term_index == 0 {
            st.col0_write.as_mut_ptr().cast::<u8>()
        } else {
            core::ptr::null_mut()
        }
    }

    // 1. Batched fast path: slot 68 covered and populated -> 0 legacy calls.
    let mut api = make_mock_api();
    api.iter_column_info = Some(mock_col_info);
    api.iter_next_chunk = Some(mock_legacy_next);
    api.iter_entities = Some(mock_legacy_entities);
    api.iter_column_readonly = Some(mock_legacy_ro);
    api.iter_column_mutable = Some(mock_legacy_rw);
    api.iter_next_chunk_batch = Some(mock_next_chunk_batch);

    let mut dummy_ctx = 1u8;
    // SAFETY: Stack pointers remain valid for the duration of this test.
    let host = unsafe { Host::from_raw(&raw const api, (&raw mut dummy_ctx).cast()) }.unwrap();
    let mut state = MockBatchState {
        chunks_left: 2,
        entities: [FfiEntity::from_bits(10), FfiEntity::from_bits(20)],
        col0_write: [1, 2],
        col1_read: [30, 40],
        has_flag: FfiBool::TRUE,
        hostile_overlap: false,
        batch_calls: 0,
        legacy_next_calls: 0,
        legacy_ent_calls: 0,
        legacy_ro_calls: 0,
        legacy_rw_calls: 0,
        last_mark_mask: 0,
    };

    {
        let mut scratch = bamabi_guest_sys::QuerySlotScratch::new();
        // SAFETY: `state` outlives `iter`.
        let mut iter = unsafe {
            ValidatedIter::from_raw_with_scratch(
                (&raw mut state).cast::<bamabi_sys::BamIter>(),
                &host,
                &mut scratch,
            )
        };
        let mut chunks_seen = 0usize;
        while let Some(len) = iter.advance_batch(4, true, 0b0001) {
            assert_eq!(len, 2);
            let mut dispenser = ChunkColumnDispenser::new(&mut iter);
            let ents = dispenser.entities().unwrap();
            let c0 = dispenser.next_write::<u32>(None).unwrap();
            let c1 = dispenser.next_read::<u32>(AccessMode::READ, None).unwrap();
            let c2 = dispenser.next_optional_read::<u32>(None).unwrap();
            let c3 = dispenser.next_has::<u8>(None).unwrap();
            assert_eq!(ents, &[FfiEntity::from_bits(10), FfiEntity::from_bits(20)]);
            c0[0] += c1[0];
            c0[1] += c1[1];
            assert!(c2.is_none());
            assert!(c3);
            chunks_seen += 1;
        }
        assert_eq!(chunks_seen, 2);
    }
    assert_eq!(state.col0_write, [61, 82]);
    assert_eq!(state.batch_calls, 3); // 2 chunks + 1 exhaustion call
    assert_eq!(state.last_mark_mask, 0b0001);
    assert_eq!(state.legacy_next_calls, 0);
    assert_eq!(state.legacy_ent_calls, 0);
    assert_eq!(state.legacy_ro_calls, 0);
    assert_eq!(state.legacy_rw_calls, 0);

    // 1b. Explicit `BatchColCache` + `advance_batch_into` + `ChunkColumnDispenser::new_with_batch`
    //     works on bare `ValidatedIter::from_raw` with 0 legacy calls.
    state.chunks_left = 1;
    {
        // SAFETY: `state` outlives `iter`.
        let mut iter = unsafe {
            ValidatedIter::from_raw((&raw mut state).cast::<bamabi_sys::BamIter>(), &host)
        };
        let mut cache = bamabi_guest_sys::BatchColCache::new();
        assert_eq!(
            iter.advance_batch_into(&mut cache, 4, true, 0b0001),
            Some(2)
        );
        let mut dispenser = ChunkColumnDispenser::new_with_batch(&mut iter, &cache);
        let ents = dispenser.entities().unwrap();
        let c0 = dispenser.next_write::<u32>(None).unwrap();
        let c1 = dispenser.next_read::<u32>(AccessMode::READ, None).unwrap();
        let c2 = dispenser.next_optional_read::<u32>(None).unwrap();
        let c3 = dispenser.next_has::<u8>(None).unwrap();
        assert_eq!(ents, &[FfiEntity::from_bits(10), FfiEntity::from_bits(20)]);
        c0[0] += c1[0];
        assert!(c2.is_none());
        assert!(c3);
    }
    assert_eq!(state.col0_write, [91, 82]);
    assert_eq!(state.batch_calls, 4);
    assert_eq!(state.legacy_next_calls, 0);
    assert_eq!(state.legacy_ent_calls, 0);
    assert_eq!(state.legacy_ro_calls, 0);
    assert_eq!(state.legacy_rw_calls, 0);

    // 2. Hostile overlapping batch pointers are still rejected with ACCESS_CONFLICT.
    state.chunks_left = 1;
    state.hostile_overlap = true;
    {
        let mut scratch = bamabi_guest_sys::QuerySlotScratch::new();
        // SAFETY: `state` outlives `iter`.
        let mut iter = unsafe {
            ValidatedIter::from_raw_with_scratch(
                (&raw mut state).cast::<bamabi_sys::BamIter>(),
                &host,
                &mut scratch,
            )
        };
        assert_eq!(iter.advance_batch(2, false, 0b01), Some(2));
        let mut dispenser = ChunkColumnDispenser::new(&mut iter);
        let _c0 = dispenser.next_write::<u32>(None).unwrap();
        assert_eq!(
            dispenser.next_read::<u32>(AccessMode::READ, None),
            Err(FfiResult::ACCESS_CONFLICT)
        );
    }

    // 3. Older MIN_V0_3_SIZE host (slot 68 uncovered): next_chunk_batch returns UNSUPPORTED and
    //    advance_batch falls back to legacy per-column FFI calls cleanly.
    let mut v03_api = api;
    v03_api.struct_size = bamabi_sys::BamHostApi::MIN_V0_3_SIZE as u32;
    // SAFETY: Stack pointers remain valid for the duration of this test.
    let v03_host =
        unsafe { Host::from_raw(&raw const v03_api, (&raw mut dummy_ctx).cast()) }.unwrap();
    state.chunks_left = 1;
    state.hostile_overlap = false;
    let prev_batch_calls = state.batch_calls;
    {
        // SAFETY: `state` outlives `iter`.
        let mut iter = unsafe {
            ValidatedIter::from_raw((&raw mut state).cast::<bamabi_sys::BamIter>(), &v03_host)
        };
        let mut ptrs = [core::ptr::null_mut::<u8>(); 2];
        assert_eq!(
            iter.next_chunk_batch(None, &mut ptrs, 0b01),
            Err(FfiResult::UNSUPPORTED)
        );
        assert_eq!(iter.advance_batch(4, true, 0b0001), Some(2));
        let mut dispenser = ChunkColumnDispenser::new(&mut iter);
        assert_eq!(dispenser.entities().unwrap().len(), 2);
        assert_eq!(dispenser.next_write::<u32>(None).unwrap().len(), 2);
        assert_eq!(
            dispenser
                .next_read::<u32>(AccessMode::READ, None)
                .unwrap()
                .len(),
            2
        );
        assert!(dispenser.next_optional_read::<u32>(None).unwrap().is_none());
        assert!(dispenser.next_has::<u8>(None).unwrap());
    }
    assert_eq!(state.batch_calls, prev_batch_calls);
    assert_eq!(state.legacy_next_calls, 1);
    assert_eq!(state.legacy_ent_calls, 1);
    assert_eq!(state.legacy_rw_calls, 1);
    assert_eq!(state.legacy_ro_calls, 3);
}

#[test]
fn validated_iter_and_dispenser_column_ticks_validate_bounds_alignment_and_overlap() {
    struct MockTickState {
        len: usize,
        col: core::cell::UnsafeCell<[u32; 3]>,
        added: [u32; 3],
        changed: [u32; 3],
        last_run: u32,
        this_run: u32,
        mode: u8, // 0=ok, 1=null_added, 2=misaligned_changed, 3=overlap_col
    }

    unsafe extern "C" fn mock_next(iter: *mut bamabi_sys::BamIter, out_len: *mut usize) -> FfiBool {
        // SAFETY: `iter` points to `MockTickState`.
        let st = unsafe { &*iter.cast::<MockTickState>() };
        // SAFETY: `out_len` is writable stack storage.
        unsafe { *out_len = st.len };
        FfiBool::TRUE
    }

    unsafe extern "C" fn mock_col_info(
        _iter: *const bamabi_sys::BamIter,
        term_index: usize,
        out: *mut bamabi_sys::BamColumnInfo,
    ) -> FfiResult {
        if term_index != 0 {
            return FfiResult::OUT_OF_RANGE;
        }
        // SAFETY: `out` is writable stack storage.
        unsafe {
            *out = bamabi_sys::BamColumnInfo {
                component: BamComponent::from_raw(1),
                mode: AccessMode::WRITE,
                uuid: ComponentUuid::from_bytes([1; 16]),
                size: core::mem::size_of::<u32>(),
                align: core::mem::align_of::<u32>(),
                storage_flags: 0,
                reserved0: 0,
            };
        }
        FfiResult::OK
    }

    unsafe extern "C" fn mock_col_mut(iter: *mut bamabi_sys::BamIter, _term: usize) -> *mut u8 {
        // SAFETY: `iter` points to `MockTickState`.
        let st = unsafe { &*iter.cast::<MockTickState>() };
        st.col.get().cast::<u8>()
    }

    unsafe extern "C" fn mock_ticks(
        iter: *const bamabi_sys::BamIter,
        term_index: usize,
        out_added: *mut *const u32,
        out_changed: *mut *const u32,
        out_last_run: *mut u32,
        out_this_run: *mut u32,
    ) -> FfiResult {
        if term_index != 0 {
            return FfiResult::OUT_OF_RANGE;
        }
        // SAFETY: `iter` points to `MockTickState`.
        let st = unsafe { &*iter.cast::<MockTickState>() };
        // SAFETY: All four out-pointers point to writable stack slots.
        unsafe {
            *out_last_run = st.last_run;
            *out_this_run = st.this_run;
            match st.mode {
                0 => {
                    *out_added = st.added.as_ptr();
                    *out_changed = st.changed.as_ptr();
                }
                1 => {
                    *out_added = core::ptr::null();
                    *out_changed = st.changed.as_ptr();
                }
                2 => {
                    *out_added = st.added.as_ptr();
                    #[allow(clippy::cast_ptr_alignment)]
                    {
                        *out_changed = st.changed.as_ptr().cast::<u8>().add(1).cast::<u32>();
                    }
                }
                _ => {
                    *out_added = st.col.get().cast::<u32>().cast_const();
                    *out_changed = st.changed.as_ptr();
                }
            }
        }
        FfiResult::OK
    }

    let mut api = make_mock_api();
    api.iter_next_chunk = Some(mock_next);
    api.iter_column_info = Some(mock_col_info);
    api.iter_column_mutable = Some(mock_col_mut);
    api.iter_column_ticks = Some(mock_ticks);

    let mut dummy_ctx = 1u8;
    // SAFETY: Stack pointers stay valid across test.
    let host = unsafe { Host::from_raw(&raw const api, (&raw mut dummy_ctx).cast()) }.unwrap();

    let mut state = MockTickState {
        len: 3,
        col: core::cell::UnsafeCell::new([10, 20, 30]),
        added: [1, 2, 3],
        changed: [4, 5, 6],
        last_run: 2,
        this_run: 6,
        mode: 0,
    };

    // 1. Valid non-empty chunk via ValidatedIter and ChunkColumnDispenser.
    {
        // SAFETY: `state` outlives `iter`.
        let mut iter = unsafe {
            ValidatedIter::from_raw((&raw mut state).cast::<bamabi_sys::BamIter>(), &host)
        };
        assert_eq!(iter.next_chunk(), Some(3));
        assert_eq!(
            iter.column_ticks(0),
            Ok((&[1, 2, 3][..], &[4, 5, 6][..], 2, 6))
        );
        assert_eq!(iter.column_ticks(1), Err(FfiResult::OUT_OF_RANGE));

        let mut dispenser = ChunkColumnDispenser::new(&mut iter);
        let term_idx = dispenser.next_term_index();
        assert_eq!(term_idx, 0);
        let col = dispenser.next_write::<u32>(None).unwrap();
        let (added, changed, last_run, this_run) = dispenser.column_ticks(term_idx).unwrap();
        assert_eq!(col, &[10, 20, 30]);
        assert_eq!(added, &[1, 2, 3]);
        assert_eq!(changed, &[4, 5, 6]);
        assert_eq!((last_run, this_run), (2, 6));
    }

    // 2. Empty chunk still reports (last_run, this_run) with empty slices.
    state.len = 0;
    {
        // SAFETY: `state` outlives `iter`.
        let mut iter = unsafe {
            ValidatedIter::from_raw((&raw mut state).cast::<bamabi_sys::BamIter>(), &host)
        };
        assert_eq!(iter.next_chunk(), Some(0));
        assert_eq!(iter.column_ticks(0), Ok((&[][..], &[][..], 2, 6)));
    }

    // 3. Hostile null & misaligned pointers fail closed.
    state.len = 3;
    state.mode = 1;
    {
        // SAFETY: `state` outlives `iter`.
        let mut iter = unsafe {
            ValidatedIter::from_raw((&raw mut state).cast::<bamabi_sys::BamIter>(), &host)
        };
        assert_eq!(iter.next_chunk(), Some(3));
        assert_eq!(iter.column_ticks(0), Err(FfiResult::NULL_POINTER));
    }
    state.mode = 2;
    {
        // SAFETY: `state` outlives `iter`.
        let mut iter = unsafe {
            ValidatedIter::from_raw((&raw mut state).cast::<bamabi_sys::BamIter>(), &host)
        };
        assert_eq!(iter.next_chunk(), Some(3));
        assert_eq!(iter.column_ticks(0), Err(FfiResult::TYPE_MISMATCH));
    }

    // 4. Hostile tick pointer overlapping active mutable column borrow rejected in dispenser.
    state.mode = 3;
    {
        // SAFETY: `state` outlives `iter`.
        let mut iter = unsafe {
            ValidatedIter::from_raw((&raw mut state).cast::<bamabi_sys::BamIter>(), &host)
        };
        assert_eq!(iter.next_chunk(), Some(3));
        let mut dispenser = ChunkColumnDispenser::new(&mut iter);
        let _col = dispenser.next_write::<u32>(None).unwrap();
        assert_eq!(dispenser.column_ticks(0), Err(FfiResult::ACCESS_CONFLICT));
    }

    // 5. Older MIN_V0_3_SIZE host (slot 69 uncovered) returns UNSUPPORTED.
    let mut v03_api = api;
    v03_api.struct_size = bamabi_sys::BamHostApi::MIN_V0_3_SIZE as u32;
    // SAFETY: Stack pointers stay valid across test.
    let v03_host =
        unsafe { Host::from_raw(&raw const v03_api, (&raw mut dummy_ctx).cast()) }.unwrap();
    // SAFETY: `state` outlives `iter`.
    let iter = unsafe {
        ValidatedIter::from_raw((&raw mut state).cast::<bamabi_sys::BamIter>(), &v03_host)
    };
    assert_eq!(iter.column_ticks(0), Err(FfiResult::UNSUPPORTED));
}

#[test]
fn entity_get_many_mut_checked_enforces_distinctness_overlap_and_zero_len() {
    struct MockManyState {
        values: core::cell::UnsafeCell<[u32; 4]>,
        hostile_overlap: bool,
        hostile_misalign: bool,
    }

    unsafe extern "C" fn mock_col_info(
        _iter: *const bamabi_sys::BamIter,
        term_index: usize,
        out: *mut bamabi_sys::BamColumnInfo,
    ) -> FfiResult {
        if term_index != 0 {
            return FfiResult::OUT_OF_RANGE;
        }
        // SAFETY: `out` is writable stack storage.
        unsafe {
            *out = bamabi_sys::BamColumnInfo {
                component: BamComponent::from_raw(1),
                mode: AccessMode::WRITE,
                uuid: ComponentUuid::from_bytes([7; 16]),
                size: core::mem::size_of::<u32>(),
                align: core::mem::align_of::<u32>(),
                storage_flags: 0,
                reserved0: 0,
            };
        }
        FfiResult::OK
    }

    unsafe extern "C" fn mock_entity_get_mut(
        sys_ctx: *mut bamabi_sys::BamSystemCtx,
        _query_index: usize,
        entity: FfiEntity,
        _term_index: usize,
    ) -> *mut u8 {
        // SAFETY: `sys_ctx` points to `MockManyState`.
        let st = unsafe { &*sys_ctx.cast::<MockManyState>() };
        let base = st.values.get().cast::<u32>();
        if st.hostile_misalign {
            // SAFETY: Offset by 1 byte inside `values` for alignment rejection test.
            return unsafe { base.cast::<u8>().add(1) };
        }
        if st.hostile_overlap {
            // Return the same pointer for distinct entity IDs.
            return base.cast::<u8>();
        }
        let idx = entity.to_bits() as usize;
        if (1..=4).contains(&idx) {
            // SAFETY: `idx - 1 < 4`.
            unsafe { base.add(idx - 1).cast::<u8>() }
        } else {
            core::ptr::null_mut()
        }
    }

    let mut api = make_mock_api();
    api.iter_column_info = Some(mock_col_info);
    api.sys_entity_get_mut = Some(mock_entity_get_mut);

    let mut dummy_ctx = 1u8;
    // SAFETY: Stack pointers stay valid across test.
    let host = unsafe { Host::from_raw(&raw const api, (&raw mut dummy_ctx).cast()) }.unwrap();

    let mut state = MockManyState {
        values: core::cell::UnsafeCell::new([10, 20, 30, 40]),
        hostile_overlap: false,
        hostile_misalign: false,
    };

    let e1 = FfiEntity::from_bits(1);
    let e2 = FfiEntity::from_bits(2);
    let e3 = FfiEntity::from_bits(3);
    let e_missing = FfiEntity::from_bits(99);

    let mut dummy_iter_tok = 1u8;
    // SAFETY: `state` and `dummy_iter_tok` stay pinned across invocation.
    let mut inv = unsafe {
        ValidatedSystemInvocation::from_raw(
            (&raw mut state).cast::<bamabi_sys::BamSystemCtx>(),
            &host,
            1,
            1,
            None,
        )
    };
    let mut sys_ref = inv.take_system_ref().unwrap();
    // SAFETY: `dummy_iter_tok` is non-null and `mock_col_info` does not dereference it.
    let mut iter = unsafe {
        ValidatedIter::from_raw(
            (&raw mut dummy_iter_tok).cast::<bamabi_sys::BamIter>(),
            &host,
        )
    };

    // 1. N = 0 succeeds cleanly with empty array.
    let empty: [&mut u32; 0] = sys_ref
        .entity_get_many_mut_checked(&mut iter, 0, 0, [], None)
        .unwrap();
    assert!(empty.is_empty());

    // 2. Distinct entities succeed and allow simultaneous mutation.
    {
        let [a, b, c] = sys_ref
            .entity_get_many_mut_checked::<3, u32>(&mut iter, 0, 0, [e1, e2, e3], None)
            .unwrap();
        *a += 1;
        *b += 2;
        *c += 3;
    }

    // 3. Duplicate entities -> ACCESS_CONFLICT.
    assert_eq!(
        sys_ref.entity_get_many_mut_checked::<2, u32>(&mut iter, 0, 0, [e1, e1], None),
        Err(FfiResult::ACCESS_CONFLICT)
    );

    // 4. Null entity -> INVALID_ENTITY.
    assert_eq!(
        sys_ref.entity_get_many_mut_checked::<2, u32>(&mut iter, 0, 0, [e1, FfiEntity::NULL], None),
        Err(FfiResult::INVALID_ENTITY)
    );

    // 5. Missing entity -> NOT_FOUND.
    assert_eq!(
        sys_ref.entity_get_many_mut_checked::<2, u32>(&mut iter, 0, 0, [e1, e_missing], None),
        Err(FfiResult::NOT_FOUND)
    );

    // 6. Hostile overlapping pointers for distinct entity IDs -> ACCESS_CONFLICT.
    let _ = (sys_ref, inv);
    assert_eq!(*state.values.get_mut(), [11, 22, 33, 40]);

    state.hostile_overlap = true;
    // SAFETY: `state` stays pinned across invocation.
    let mut inv2 = unsafe {
        ValidatedSystemInvocation::from_raw(
            (&raw mut state).cast::<bamabi_sys::BamSystemCtx>(),
            &host,
            1,
            1,
            None,
        )
    };
    let mut sys_ref2 = inv2.take_system_ref().unwrap();
    assert_eq!(
        sys_ref2.entity_get_many_mut_checked::<2, u32>(&mut iter, 0, 0, [e1, e2], None),
        Err(FfiResult::ACCESS_CONFLICT)
    );

    // 7. Hostile misaligned pointer -> TYPE_MISMATCH.
    let _ = (sys_ref2, inv2);
    state.hostile_overlap = false;
    state.hostile_misalign = true;
    // SAFETY: `state` stays pinned across invocation.
    let mut inv3 = unsafe {
        ValidatedSystemInvocation::from_raw(
            (&raw mut state).cast::<bamabi_sys::BamSystemCtx>(),
            &host,
            1,
            1,
            None,
        )
    };
    let mut sys_ref3 = inv3.take_system_ref().unwrap();
    assert_eq!(
        sys_ref3.entity_get_many_mut_checked::<1, u32>(&mut iter, 0, 0, [e1], None),
        Err(FfiResult::TYPE_MISMATCH)
    );
}

#[test]
fn validated_commands_clone_entity_and_resource_commands_validate_slots_and_layout() {
    struct MockCmdState {
        layout_calls: core::cell::Cell<usize>,
        clone_calls: usize,
        last_clone: (FfiEntity, FfiEntity),
        insert_res_calls: usize,
        last_insert_val: u64,
        remove_res_calls: usize,
        last_removed: BamComponent,
    }

    unsafe extern "C" fn mock_layout_info(
        ctx: *const bamabi_sys::BamHostCtx,
        comp: BamComponent,
        out: *mut bamabi_sys::BamLayoutInfo,
    ) -> FfiResult {
        // SAFETY: `ctx` points to `MockCmdState`.
        let st = unsafe { &*ctx.cast::<MockCmdState>() };
        st.layout_calls.set(st.layout_calls.get() + 1);
        if comp == BamComponent::from_raw(10) {
            // SAFETY: `out` is writable stack storage.
            unsafe {
                *out = bamabi_sys::BamLayoutInfo {
                    size: core::mem::size_of::<u64>(),
                    align: core::mem::align_of::<u64>(),
                    uuid: ComponentUuid::from_bytes([0xAA; 16]),
                    ..bamabi_sys::BamLayoutInfo::empty()
                };
            }
            FfiResult::OK
        } else {
            FfiResult::NOT_FOUND
        }
    }

    unsafe extern "C" fn mock_clone_entity(
        cmds: *mut bamabi_sys::BamCommands,
        src: FfiEntity,
        dst: FfiEntity,
    ) -> FfiResult {
        // SAFETY: `cmds` points to `MockCmdState`.
        let st = unsafe { &mut *cmds.cast::<MockCmdState>() };
        st.clone_calls += 1;
        st.last_clone = (src, dst);
        FfiResult::OK
    }

    unsafe extern "C" fn mock_insert_resource(
        cmds: *mut bamabi_sys::BamCommands,
        _res: BamComponent,
        data_ptr: *const u8,
        data_len: usize,
    ) -> FfiResult {
        // SAFETY: `cmds` points to `MockCmdState`.
        let st = unsafe { &mut *cmds.cast::<MockCmdState>() };
        st.insert_res_calls += 1;
        if data_len == core::mem::size_of::<u64>() {
            // SAFETY: Caller passes valid `u64` bytes.
            st.last_insert_val = unsafe { core::ptr::read_unaligned(data_ptr.cast::<u64>()) };
        }
        FfiResult::OK
    }

    unsafe extern "C" fn mock_remove_resource(
        cmds: *mut bamabi_sys::BamCommands,
        res: BamComponent,
    ) -> FfiResult {
        // SAFETY: `cmds` points to `MockCmdState`.
        let st = unsafe { &mut *cmds.cast::<MockCmdState>() };
        st.remove_res_calls += 1;
        st.last_removed = res;
        FfiResult::OK
    }

    let mut api = make_mock_api();
    api.component_layout_info = Some(mock_layout_info);
    api.cmd_clone_entity = Some(mock_clone_entity);
    api.cmd_insert_resource = Some(mock_insert_resource);
    api.cmd_remove_resource = Some(mock_remove_resource);

    let mut state = MockCmdState {
        layout_calls: core::cell::Cell::new(0),
        clone_calls: 0,
        last_clone: (FfiEntity::NULL, FfiEntity::NULL),
        insert_res_calls: 0,
        last_insert_val: 0,
        remove_res_calls: 0,
        last_removed: BamComponent::INVALID,
    };

    let res_comp = BamComponent::from_raw(10);
    let e1 = FfiEntity::from_bits(1);
    let e2 = FfiEntity::from_bits(2);

    {
        let state_ptr = &raw mut state;
        // SAFETY: `api` and `state` stay pinned for `host` and `cmds`.
        let host =
            unsafe { Host::from_raw(&raw const api, state_ptr.cast_const().cast()) }.unwrap();
        let typed = bamabi_guest_sys::TypedComponent::<u64>::validate(&host, res_comp).unwrap();
        // SAFETY: `state` acts as `BamCommands`.
        let mut cmds = unsafe { ValidatedCommands::from_raw(state_ptr.cast(), &host) };

        // clone_entity validation & happy path
        assert_eq!(
            cmds.clone_entity(FfiEntity::NULL, e2),
            FfiResult::INVALID_ENTITY
        );
        assert_eq!(
            cmds.clone_entity(e1, FfiEntity::NULL),
            FfiResult::INVALID_ENTITY
        );
        assert_eq!(cmds.clone_entity(e1, e2), FfiResult::OK);

        // insert_resource validates layout (u32 fails TYPE_MISMATCH, u64 succeeds)
        assert_eq!(
            cmds.insert_resource::<u32>(res_comp, &42u32),
            FfiResult::TYPE_MISMATCH
        );
        assert_eq!(
            cmds.insert_resource::<u64>(BamComponent::INVALID, &99u64),
            FfiResult::INVALID_COMPONENT
        );
        assert_eq!(
            cmds.insert_resource::<u64>(res_comp, &0x1234_5678u64),
            FfiResult::OK
        );

        // insert_resource_typed skips component_layout_info
        assert_eq!(
            cmds.insert_resource_typed(typed, &0xDEAD_BEEFu64),
            FfiResult::OK
        );

        // remove_resource
        assert_eq!(
            cmds.remove_resource(BamComponent::INVALID),
            FfiResult::INVALID_COMPONENT
        );
        assert_eq!(cmds.remove_resource(res_comp), FfiResult::OK);
    }

    // 1 call in TypedComponent::validate + 1 for u32 mismatch + 1 for u64 insert_resource = 3
    // (insert_resource_typed skipped component_layout_info!).
    assert_eq!(state.layout_calls.get(), 3);
    assert_eq!(state.clone_calls, 1);
    assert_eq!(state.last_clone, (e1, e2));
    assert_eq!(state.insert_res_calls, 2);
    assert_eq!(state.last_insert_val, 0xDEAD_BEEFu64);
    assert_eq!(state.remove_res_calls, 1);
    assert_eq!(state.last_removed, res_comp);

    // Older MIN_V0_3_SIZE host (slots 70..72 uncovered) returns UNSUPPORTED.
    let mut v03_api = api;
    v03_api.struct_size = bamabi_sys::BamHostApi::MIN_V0_3_SIZE as u32;
    let state_ptr = &raw mut state;
    // SAFETY: Stack pointers stay valid across test.
    let v03_host =
        unsafe { Host::from_raw(&raw const v03_api, state_ptr.cast_const().cast()) }.unwrap();
    // SAFETY: `state` acts as `BamCommands`.
    let mut v03_cmds = unsafe { ValidatedCommands::from_raw(state_ptr.cast(), &v03_host) };
    assert_eq!(v03_cmds.clone_entity(e1, e2), FfiResult::UNSUPPORTED);
    assert_eq!(
        v03_cmds.insert_resource(res_comp, &1u64),
        FfiResult::UNSUPPORTED
    );
    assert_eq!(v03_cmds.remove_resource(res_comp), FfiResult::UNSUPPORTED);
}

#[test]
fn run_if_ctx_trampoline_and_registration_config_forward_to_system_desc() {
    use core::sync::atomic::{AtomicBool, Ordering};

    struct MockRegState {
        saw_run_if_ctx: bool,
        saw_query_terms: usize,
        captured_fn: Option<bamabi_sys::BamSystemFn>,
        captured_user_data: *mut c_void,
    }

    unsafe extern "C" fn mock_register_system(
        load_ctx: *mut bamabi_sys::BamLoadCtx,
        desc: *const bamabi_sys::BamSystemDesc,
    ) -> FfiResult {
        // SAFETY: `load_ctx` points to `MockRegState`.
        let st = unsafe { &mut *load_ctx.cast::<MockRegState>() };
        // SAFETY: `desc` points to a valid `BamSystemDesc`.
        let desc_ref = unsafe { &*desc };
        st.saw_run_if_ctx = desc_ref.run_if_ctx.is_some();
        st.captured_fn = desc_ref.run_if_ctx;
        st.captured_user_data = desc_ref.run_if_ctx_user_data;
        if !desc_ref.run_if_query.is_null() {
            // SAFETY: `run_if_query` points to a valid `BamQueryDesc`.
            st.saw_query_terms = unsafe { (*desc_ref.run_if_query).term_count };
        }
        FfiResult::OK
    }

    let mut api = make_mock_api();
    api.register_system = Some(mock_register_system);

    let mut reg_state = MockRegState {
        saw_run_if_ctx: false,
        saw_query_terms: 0,
        captured_fn: None,
        captured_user_data: core::ptr::null_mut(),
    };

    let manifest = bamabi_sys::BamManifest::new(c"run_if_ctx_mod".as_ptr(), c"0.1.0".as_ptr());
    let mut raw_inst: *mut bamabi_sys::BamInstance = core::ptr::null_mut();
    let allow_run = std::sync::Arc::new(AtomicBool::new(true));
    let allow_run_clone = std::sync::Arc::clone(&allow_run);

    // SAFETY: Stack pointers stay live across `init_guest_instance` and `shutdown_guest_instance`.
    let status = unsafe {
        bamabi_guest_sys::init_guest_instance(
            &raw const api,
            (&raw mut reg_state).cast::<bamabi_sys::BamLoadCtx>(),
            &raw mut raw_inst,
            &manifest,
            move |inst, load| {
                let run_if_terms = [bamabi_sys::BamTermDesc::new(
                    BamComponent::from_raw(5),
                    AccessMode::RESOURCE_READ,
                )];
                let query_ptr =
                    match bamabi_guest_sys::allocate_run_if_ctx_query(inst, &run_if_terms) {
                        Ok(ptr) => ptr,
                        Err(err) => return err,
                    };
                let (run_if_fn, run_if_ud) =
                    bamabi_guest_sys::allocate_run_if_ctx_trampoline(inst, 1, move |inv| {
                        assert_eq!(inv.query_count(), 1);
                        allow_run_clone.load(Ordering::Relaxed)
                    });
                let cfg = bamabi_guest_sys::SystemRegistrationConfig {
                    name: c"gated_sys",
                    run_if_ctx: Some(run_if_fn),
                    run_if_query: query_ptr,
                    run_if_ctx_user_data: run_if_ud,
                    ..Default::default()
                };
                load.register_system(inst, &cfg, None, |_| FfiResult::OK)
            },
        )
    };
    assert_eq!(status, FfiResult::OK);
    assert!(reg_state.saw_run_if_ctx);
    assert_eq!(reg_state.saw_query_terms, 1);

    let run_if_fn = reg_state.captured_fn.unwrap();
    let mut dummy_sys_ctx = 1u8;
    // Predicate returns true -> FfiResult::OK (run).
    // SAFETY: `raw_inst` is still alive and `dummy_sys_ctx` is non-null.
    let rc_true = unsafe {
        run_if_fn(
            (&raw mut dummy_sys_ctx).cast::<bamabi_sys::BamSystemCtx>(),
            reg_state.captured_user_data,
        )
    };
    assert_eq!(rc_true, FfiResult::OK);

    // Predicate returns false -> non-zero (skip).
    allow_run.store(false, Ordering::Relaxed);
    // SAFETY: `raw_inst` is still alive and `dummy_sys_ctx` is non-null.
    let rc_false = unsafe {
        run_if_fn(
            (&raw mut dummy_sys_ctx).cast::<bamabi_sys::BamSystemCtx>(),
            reg_state.captured_user_data,
        )
    };
    assert!(!rc_false.is_ok());

    // SAFETY: `raw_inst` was returned by `init_guest_instance`.
    unsafe { bamabi_guest_sys::shutdown_guest_instance(raw_inst) };
}

/// Deterministic mock-vtable counter test verifying that `command_burst`-style 0-query systems
/// (`spawn_empty`, `insert_typed`, `checkpoint`, `rollback`) make ZERO `sys_host_ctx` calls,
/// `ValidatedCommands::host()` resolves `sys_host_ctx` lazily at most once per tick, and
/// `ValidatedSystemRef::intern_symbol` uses `resolve_ctx` with 1 `sys_host_ctx` call and clean
/// null-context fallback.
#[test]
fn command_burst_commands_make_zero_sys_host_ctx_calls_and_intern_symbol_uses_ctx_only() {
    static HOST_CTX_CALLS: AtomicUsize = AtomicUsize::new(0);
    static COMMANDS_CALLS: AtomicUsize = AtomicUsize::new(0);
    static SPAWN_EMPTY_CALLS: AtomicUsize = AtomicUsize::new(0);
    static INSERT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static CHECKPOINT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static ROLLBACK_CALLS: AtomicUsize = AtomicUsize::new(0);
    static INTERN_CALLS: AtomicUsize = AtomicUsize::new(0);
    static LAST_INTERN_CTX_ADDR: AtomicUsize = AtomicUsize::new(0);
    static RUNTIME_CTX_PTR: AtomicPtr<BamHostCtx> = AtomicPtr::new(core::ptr::null_mut());
    static DUMMY_CMDS_PTR: AtomicPtr<bamabi_sys::BamCommands> =
        AtomicPtr::new(core::ptr::null_mut());

    unsafe extern "C" fn counting_sys_host_ctx(_sys_ctx: *const BamSystemCtx) -> *const BamHostCtx {
        HOST_CTX_CALLS.fetch_add(1, Ordering::SeqCst);
        RUNTIME_CTX_PTR.load(Ordering::SeqCst).cast_const()
    }

    unsafe extern "C" fn counting_sys_commands(
        _sys_ctx: *mut BamSystemCtx,
        out_cmds: *mut *mut bamabi_sys::BamCommands,
    ) -> FfiResult {
        COMMANDS_CALLS.fetch_add(1, Ordering::SeqCst);
        // SAFETY: `out_cmds` is valid writable stack storage.
        unsafe { *out_cmds = DUMMY_CMDS_PTR.load(Ordering::SeqCst) };
        FfiResult::OK
    }

    unsafe extern "C" fn counting_spawn_empty(
        _cmds: *mut bamabi_sys::BamCommands,
        out_entity: *mut FfiEntity,
    ) -> FfiResult {
        let n = SPAWN_EMPTY_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
        // SAFETY: `out_entity` is valid writable stack storage.
        unsafe { *out_entity = FfiEntity::from_bits(n as u64) };
        FfiResult::OK
    }

    unsafe extern "C" fn counting_insert(
        _cmds: *mut bamabi_sys::BamCommands,
        _entity: FfiEntity,
        _component: BamComponent,
        _data: *const u8,
        _len: usize,
    ) -> FfiResult {
        INSERT_CALLS.fetch_add(1, Ordering::SeqCst);
        FfiResult::OK
    }

    unsafe extern "C" fn counting_checkpoint(
        _cmds: *mut bamabi_sys::BamCommands,
        out_token: *mut usize,
    ) -> FfiResult {
        CHECKPOINT_CALLS.fetch_add(1, Ordering::SeqCst);
        // SAFETY: `out_token` is valid writable stack storage.
        unsafe { *out_token = 77 };
        FfiResult::OK
    }

    unsafe extern "C" fn counting_rollback(
        _cmds: *mut bamabi_sys::BamCommands,
        _token: usize,
    ) -> FfiResult {
        ROLLBACK_CALLS.fetch_add(1, Ordering::SeqCst);
        FfiResult::OK
    }

    unsafe extern "C" fn counting_intern_symbol(
        host_ctx: *const BamHostCtx,
        _sym: *const bamabi_sys::StrView,
        out_sym: *mut bamabi_sys::BamSymbol,
    ) -> FfiResult {
        INTERN_CALLS.fetch_add(1, Ordering::SeqCst);
        LAST_INTERN_CTX_ADDR.store(host_ctx.addr(), Ordering::SeqCst);
        // SAFETY: `out_sym` is valid writable stack storage.
        unsafe { *out_sym = bamabi_sys::BamSymbol(42) };
        FfiResult::OK
    }

    HOST_CTX_CALLS.store(0, Ordering::SeqCst);
    COMMANDS_CALLS.store(0, Ordering::SeqCst);
    SPAWN_EMPTY_CALLS.store(0, Ordering::SeqCst);
    INSERT_CALLS.store(0, Ordering::SeqCst);
    CHECKPOINT_CALLS.store(0, Ordering::SeqCst);
    ROLLBACK_CALLS.store(0, Ordering::SeqCst);
    INTERN_CALLS.store(0, Ordering::SeqCst);
    LAST_INTERN_CTX_ADDR.store(0, Ordering::SeqCst);

    let mut init_marker = 1u8;
    let mut runtime_marker = 2u8;
    let mut cmds_marker = 3u8;
    let init_ctx = (&raw mut init_marker).cast::<BamHostCtx>().cast_const();
    let runtime_ctx = (&raw mut runtime_marker).cast::<BamHostCtx>();
    let cmds_ptr = (&raw mut cmds_marker).cast::<bamabi_sys::BamCommands>();
    RUNTIME_CTX_PTR.store(runtime_ctx, Ordering::SeqCst);
    DUMMY_CMDS_PTR.store(cmds_ptr, Ordering::SeqCst);

    let mut api = make_mock_api();
    api.sys_host_ctx = Some(counting_sys_host_ctx);
    api.sys_commands = Some(counting_sys_commands);
    api.cmd_spawn_empty = Some(counting_spawn_empty);
    api.cmd_insert = Some(counting_insert);
    api.cmd_checkpoint = Some(counting_checkpoint);
    api.cmd_rollback = Some(counting_rollback);
    api.intern_symbol = Some(counting_intern_symbol);

    // SAFETY: `api` and `init_ctx` remain valid for the duration of this test.
    let init_host = unsafe { Host::from_raw(&raw const api, init_ctx) }.unwrap();
    let mut instance = GuestInstance::new(init_host, "burst_counter_mod");
    // SAFETY: Mock component layout matches `[f32; 2]` (`8` bytes, `4` align).
    let typed_pos = unsafe {
        bamabi_guest_sys::TypedComponent::<[f32; 2]>::from_raw_unchecked(BamComponent::from_raw(9))
    };

    let mut dummy_sys = 0u64;
    let sys_ctx = (&raw mut dummy_sys).cast::<BamSystemCtx>();

    // 1. `command_burst` shape (0 queries + spawn_empty + insert_typed + checkpoint + rollback):
    //    must make ZERO `sys_host_ctx` calls!
    let (burst_fn, burst_ud) = bamabi_guest_sys::allocate_system_slice_trampoline::<0, _>(
        &mut instance,
        0,
        0,
        None,
        move |inv| {
            let mut cmds = inv.take_lazy_commands().resolve();
            for i in 0..64 {
                let e = cmds.spawn_empty().unwrap();
                assert_eq!(
                    cmds.insert_typed(e, typed_pos, &[i as f32, 1.0]),
                    FfiResult::OK
                );
            }
            let cp = cmds.checkpoint().unwrap();
            let _ = cmds.spawn_empty().unwrap();
            assert_eq!(cmds.rollback(cp), FfiResult::OK);
            FfiResult::OK
        },
    );
    // SAFETY: `sys_ctx` and `burst_ud` are valid for this synchronous call.
    assert_eq!(unsafe { burst_fn(sys_ctx, burst_ud) }, FfiResult::OK);
    assert_eq!(HOST_CTX_CALLS.load(Ordering::SeqCst), 0);
    assert_eq!(COMMANDS_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(SPAWN_EMPTY_CALLS.load(Ordering::SeqCst), 65);
    assert_eq!(INSERT_CALLS.load(Ordering::SeqCst), 64);
    assert_eq!(CHECKPOINT_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(ROLLBACK_CALLS.load(Ordering::SeqCst), 1);

    // 2. Lazy `.host()` on `ValidatedCommands`: 0 `sys_host_ctx` calls before `.host()`, then 1
    //    call on first `.host()`, shared across `inv.host()` and subsequent `cmds.host()`.
    let expected_runtime_addr = runtime_ctx.addr();
    let (lazy_fn, lazy_ud) = bamabi_guest_sys::allocate_system_slice_trampoline::<0, _>(
        &mut instance,
        0,
        0,
        None,
        move |inv| {
            let mut cmds = inv.take_lazy_commands().resolve();
            let _ = cmds.spawn_empty().unwrap();
            assert_eq!(HOST_CTX_CALLS.load(Ordering::SeqCst), 0);
            assert_eq!(cmds.host().host_ctx().addr(), expected_runtime_addr);
            assert_eq!(HOST_CTX_CALLS.load(Ordering::SeqCst), 1);
            assert_eq!(inv.host().host_ctx().addr(), expected_runtime_addr);
            assert_eq!(cmds.host().host_ctx().addr(), expected_runtime_addr);
            assert_eq!(HOST_CTX_CALLS.load(Ordering::SeqCst), 1);
            FfiResult::OK
        },
    );
    // SAFETY: `sys_ctx` and `lazy_ud` are valid for this synchronous call.
    assert_eq!(unsafe { lazy_fn(sys_ctx, lazy_ud) }, FfiResult::OK);

    // 3. `ValidatedSystemRef::intern_symbol`: 64 calls -> 1 `sys_host_ctx` call, observing
    //    `runtime_ctx`, and `inv.host()` still reuses that cached `sys_host_ctx` resolution.
    HOST_CTX_CALLS.store(0, Ordering::SeqCst);
    let (intern_fn, intern_ud) = bamabi_guest_sys::allocate_system_slice_trampoline::<0, _>(
        &mut instance,
        0,
        0,
        None,
        move |inv| {
            let sys_ref = inv.take_system_ref().unwrap();
            for _ in 0..64 {
                assert_eq!(sys_ref.intern_symbol("bench.sym").unwrap().0, 42);
            }
            assert_eq!(HOST_CTX_CALLS.load(Ordering::SeqCst), 1);
            assert_eq!(inv.host().host_ctx().addr(), expected_runtime_addr);
            assert_eq!(HOST_CTX_CALLS.load(Ordering::SeqCst), 1);
            FfiResult::OK
        },
    );
    // SAFETY: `sys_ctx` and `intern_ud` are valid for this synchronous call.
    assert_eq!(unsafe { intern_fn(sys_ctx, intern_ud) }, FfiResult::OK);
    assert_eq!(INTERN_CALLS.load(Ordering::SeqCst), 64);
    assert_eq!(
        LAST_INTERN_CTX_ADDR.load(Ordering::SeqCst),
        expected_runtime_addr
    );

    // 4. Null `sys_host_ctx` fallback: `sys_ref.intern_symbol` and `cmds.host().host_ctx()`
    //    both observe `init_ctx` with `HOST_CTX_CALLS == 1`.
    HOST_CTX_CALLS.store(0, Ordering::SeqCst);
    RUNTIME_CTX_PTR.store(core::ptr::null_mut(), Ordering::SeqCst);
    let expected_init_addr = init_ctx.addr();
    let (fallback_fn, fallback_ud) = bamabi_guest_sys::allocate_system_slice_trampoline::<0, _>(
        &mut instance,
        0,
        0,
        None,
        move |inv| {
            let sys_ref = inv.take_system_ref().unwrap();
            let cmds = inv.take_lazy_commands().resolve();
            assert_eq!(sys_ref.intern_symbol("bench.sym").unwrap().0, 42);
            assert_eq!(cmds.host().host_ctx().addr(), expected_init_addr);
            assert_eq!(HOST_CTX_CALLS.load(Ordering::SeqCst), 1);
            FfiResult::OK
        },
    );
    // SAFETY: `sys_ctx` and `fallback_ud` are valid for this synchronous call.
    assert_eq!(unsafe { fallback_fn(sys_ctx, fallback_ud) }, FfiResult::OK);
    assert_eq!(
        LAST_INTERN_CTX_ADDR.load(Ordering::SeqCst),
        expected_init_addr
    );
}

/// Verifies persistent per-system `QuerySlotScratch` metadata caching across ticks for:
/// 1. `ChunkColumnDispenser` (`0` steady-state `iter_column_info` calls on tick >= 2, dynamic
///    branching `TypeId` / `AccessMode` / `UuidFilter` mismatch rejection without poisoning, and
///    `> INLINE_COL_INFO_CAP` fallback for 10+ terms).
/// 2. `ValidatedIter::resource_with_ticks_checked` (`0` steady-state `iter_column_info` calls
///    across 64-call loops on tick 1 calls 2..=64 and all of tick >= 2, plus type/UUID mismatch).
/// 3. `par_for_each_chunk` and `par_for_each_chunk_readonly` reading the frozen cache lock-free
///    with `0` `iter_column_info` calls across worker invocations.
#[test]
fn persistent_slot_cache_zero_steady_state_column_info_and_mismatch_guard() {
    #[repr(C)]
    struct MockIter {
        col_info_calls: *const AtomicUsize,
        res_ticks_calls: *const AtomicUsize,
        chunk_idx: AtomicUsize,
        chunk_count: AtomicUsize,
        col0_write: core::cell::UnsafeCell<[u32; 2]>,
        col1_read: [u32; 2],
        res_val: u32,
        is_resource: bool,
    }

    #[repr(C)]
    struct MockSys {
        entity_iter: MockIter,
        res_iter: MockIter,
    }

    let uuid_w = ComponentUuid::from_parts("m3::WriteU32", 4, 4, 0);
    let uuid_r = ComponentUuid::from_parts("m3::ReadU32", 4, 4, 0);
    let uuid_res = ComponentUuid::from_parts("m3::ResU32", 4, 4, 0);
    let uuid_wrong = ComponentUuid::from_parts("m3::Wrong", 4, 4, 0);

    unsafe extern "C" fn sys_query(
        sys_ctx: *mut BamSystemCtx,
        query_index: usize,
        out_iter: *mut *mut bamabi_sys::BamIter,
    ) -> FfiResult {
        // SAFETY: Test binds `sys_ctx` to `MockSys`.
        let sys = unsafe { &*sys_ctx.cast::<MockSys>() };
        let ptr = match query_index {
            0 => (&raw const sys.entity_iter)
                .cast_mut()
                .cast::<bamabi_sys::BamIter>(),
            1 => (&raw const sys.res_iter)
                .cast_mut()
                .cast::<bamabi_sys::BamIter>(),
            _ => return FfiResult::OUT_OF_RANGE,
        };
        // SAFETY: `out_iter` is writable stack storage.
        unsafe { *out_iter = ptr };
        FfiResult::OK
    }

    unsafe extern "C" fn iter_next_chunk(
        iter: *mut bamabi_sys::BamIter,
        out_len: *mut usize,
    ) -> FfiBool {
        // SAFETY: `iter` points to `MockIter`.
        let st = unsafe { &*iter.cast::<MockIter>() };
        let cur = st.chunk_idx.load(Ordering::SeqCst);
        if cur >= st.chunk_count.load(Ordering::SeqCst) {
            return FfiBool::FALSE;
        }
        st.chunk_idx.store(cur + 1, Ordering::SeqCst);
        // SAFETY: `out_len` is writable stack storage.
        unsafe { *out_len = 2 };
        FfiBool::TRUE
    }

    unsafe extern "C" fn iter_next_chunk_batch(
        iter: *mut bamabi_sys::BamIter,
        out_len: *mut usize,
        _out_entities: *mut *const FfiEntity,
        out_col_ptrs: *mut *mut u8,
        col_count: usize,
        _mark_changed_mask: u32,
    ) -> FfiBool {
        // SAFETY: `iter` points to `MockIter`.
        let st = unsafe { &*iter.cast::<MockIter>() };
        let cur = st.chunk_idx.load(Ordering::SeqCst);
        if cur >= st.chunk_count.load(Ordering::SeqCst) {
            return FfiBool::FALSE;
        }
        st.chunk_idx.store(cur + 1, Ordering::SeqCst);
        // SAFETY: `out_len` and `out_col_ptrs[0..col_count]` are valid writable slots.
        unsafe {
            *out_len = 2;
            if col_count > 0 {
                *out_col_ptrs.add(0) = st.col0_write.get().cast::<u8>();
            }
            if col_count > 1 {
                *out_col_ptrs.add(1) = st.col1_read.as_ptr().cast_mut().cast::<u8>();
            }
        }
        FfiBool::TRUE
    }

    unsafe extern "C" fn iter_col_ro(
        iter: *const bamabi_sys::BamIter,
        term_index: usize,
    ) -> *const u8 {
        // SAFETY: `iter` points to `MockIter`.
        let st = unsafe { &*iter.cast::<MockIter>() };
        if term_index >= 1 {
            st.col1_read.as_ptr().cast::<u8>()
        } else {
            st.col0_write.get().cast_const().cast::<u8>()
        }
    }

    unsafe extern "C" fn iter_col_mut(
        iter: *mut bamabi_sys::BamIter,
        term_index: usize,
    ) -> *mut u8 {
        // SAFETY: `iter` points to `MockIter`.
        let st = unsafe { &*iter.cast::<MockIter>() };
        if term_index == 0 {
            st.col0_write.get().cast::<u8>()
        } else {
            core::ptr::null_mut()
        }
    }

    unsafe extern "C" fn iter_col_info(
        iter: *const bamabi_sys::BamIter,
        term_index: usize,
        out_info: *mut bamabi_sys::BamColumnInfo,
    ) -> FfiResult {
        // SAFETY: `iter` points to `MockIter`.
        let st = unsafe { &*iter.cast::<MockIter>() };
        // SAFETY: `st.col_info_calls` points to test stack `AtomicUsize`.
        unsafe { (*st.col_info_calls).fetch_add(1, Ordering::SeqCst) };
        let info = if st.is_resource {
            if term_index != 0 {
                return FfiResult::OUT_OF_RANGE;
            }
            bamabi_sys::BamColumnInfo {
                component: BamComponent::from_raw(30),
                uuid: ComponentUuid::from_parts("m3::ResU32", 4, 4, 0),
                size: 4,
                align: 4,
                mode: AccessMode::RESOURCE_READ,
                storage_flags: bamabi_sys::BAM_STORAGE_FLAG_RESOURCE,
                reserved0: 0,
            }
        } else if term_index == 0 {
            bamabi_sys::BamColumnInfo {
                component: BamComponent::from_raw(10),
                uuid: ComponentUuid::from_parts("m3::WriteU32", 4, 4, 0),
                size: 4,
                align: 4,
                mode: AccessMode::WRITE,
                storage_flags: 0,
                reserved0: 0,
            }
        } else if term_index < 10 {
            bamabi_sys::BamColumnInfo {
                component: BamComponent::from_raw((11 + term_index) as u32),
                uuid: ComponentUuid::from_parts("m3::ReadU32", 4, 4, 0),
                size: 4,
                align: 4,
                mode: AccessMode::READ,
                storage_flags: 0,
                reserved0: 0,
            }
        } else {
            return FfiResult::OUT_OF_RANGE;
        };
        // SAFETY: `out_info` is writable stack storage.
        unsafe { *out_info = info };
        FfiResult::OK
    }

    unsafe extern "C" fn iter_res_ptr(
        iter: *const bamabi_sys::BamIter,
        term_index: usize,
    ) -> *const u8 {
        if term_index != 0 {
            return core::ptr::null();
        }
        // SAFETY: `iter` points to `MockIter`.
        let st = unsafe { &*iter.cast::<MockIter>() };
        (&raw const st.res_val).cast::<u8>()
    }

    unsafe extern "C" fn iter_res_ticks(
        iter: *const bamabi_sys::BamIter,
        term_index: usize,
        out_ticks: *mut bamabi_sys::BamChangeTicks,
    ) -> FfiResult {
        if term_index != 0 {
            return FfiResult::OUT_OF_RANGE;
        }
        // SAFETY: `iter` points to `MockIter`.
        let st = unsafe { &*iter.cast::<MockIter>() };
        // SAFETY: `st.res_ticks_calls` points to test stack `AtomicUsize`.
        unsafe { (*st.res_ticks_calls).fetch_add(1, Ordering::SeqCst) };
        // SAFETY: `out_ticks` is writable stack storage.
        unsafe {
            *out_ticks = bamabi_sys::BamChangeTicks {
                added: 1,
                changed: 2,
                last_run: 1,
                this_run: 2,
            };
        }
        FfiResult::OK
    }

    unsafe extern "C" fn par_for_each(
        iter: *mut bamabi_sys::BamIter,
        task_fn: Option<unsafe extern "C" fn(*mut bamabi_sys::BamIter, *mut c_void) -> FfiResult>,
        user_data: *mut c_void,
    ) -> FfiResult {
        let Some(func) = task_fn else {
            return FfiResult::NULL_POINTER;
        };
        // SAFETY: `iter` points to `MockIter`; invoke worker trampoline twice for two sub-chunks.
        let st = unsafe { &*iter.cast::<MockIter>() };
        st.chunk_idx.store(0, Ordering::SeqCst);
        for step in 0..2 {
            st.chunk_count.store(step + 1, Ordering::SeqCst);
            // SAFETY: `iter` and `user_data` are valid for this synchronous sub-chunk task call.
            let rc = unsafe { func(iter, user_data) };
            if !rc.is_ok() {
                return rc;
            }
        }
        FfiResult::OK
    }

    let entity_info_calls = AtomicUsize::new(0);
    let res_info_calls = AtomicUsize::new(0);
    let res_ticks_calls = AtomicUsize::new(0);

    let mut api = make_mock_api();
    api.sys_query = Some(sys_query);
    api.iter_next_chunk = Some(iter_next_chunk);
    api.iter_next_chunk_batch = Some(iter_next_chunk_batch);
    api.iter_column_readonly = Some(iter_col_ro);
    api.iter_column_mutable = Some(iter_col_mut);
    api.iter_column_info = Some(iter_col_info);
    api.res_ptr = Some(iter_res_ptr);
    api.res_ticks = Some(iter_res_ticks);
    api.par_for_each_chunk = Some(par_for_each);
    api.par_for_each_chunk_readonly = Some(par_for_each);

    let mut dummy_ctx = 1u8;
    // SAFETY: `api` and `dummy_ctx` outlive `host` and `instance`.
    let host = unsafe { Host::from_raw(&raw const api, (&raw mut dummy_ctx).cast()) }.unwrap();
    let mut instance = GuestInstance::new(host, "m3_cache_mod");

    let mut tick_phase = 0usize;
    let (sys_fn, sys_ud) =
        allocate_system_slice_trampoline::<4, _>(&mut instance, 2, 1, Some(1), move |inv| {
            tick_phase += 1;
            let mut entity_iter = inv.take_raw_query_slot(0).unwrap();
            let res_iter = inv.take_raw_query_slot(1).unwrap();
            let mut batch = BatchColCache::new();

            match tick_phase {
                1 | 2 => {
                    // Tick 1 warms cache (2 `iter_column_info` calls across 2 chunks);
                    // Tick 2 must make 0 `iter_column_info` calls!
                    while entity_iter
                        .advance_batch_into(&mut batch, 2, false, 1)
                        .is_some()
                    {
                        let mut disp =
                            ChunkColumnDispenser::new_with_batch(&mut entity_iter, &batch);
                        let w = disp
                            .next_write::<u32>(Some(UuidFilter::exact(uuid_w)))
                            .unwrap();
                        let r = disp
                            .next_read::<u32>(AccessMode::READ, Some(UuidFilter::exact(uuid_r)))
                            .unwrap();
                        w[0] += r[0];
                        w[1] += r[1];
                    }
                    // 64-call `res_with_ticks` loop: Tick 1 makes 1 info call; Tick 2 makes 0.
                    let mut sum = 0u32;
                    for _ in 0..64 {
                        let (val, ticks) = res_iter
                            .resource_with_ticks_checked::<u32>(
                                0,
                                Some(UuidFilter::exact(uuid_res)),
                            )
                            .unwrap();
                        sum += *val + ticks.this_run;
                    }
                    assert!(sum > 0);
                }
                3 => {
                    // Dynamic branching mismatch on same slot/term must be rejected without poisoning.
                    assert!(
                        entity_iter
                            .advance_batch_into(&mut batch, 2, false, 1)
                            .is_some()
                    );
                    {
                        let mut disp =
                            ChunkColumnDispenser::new_with_batch(&mut entity_iter, &batch);
                        assert_eq!(disp.next_write::<u64>(None), Err(FfiResult::TYPE_MISMATCH));
                    }
                    {
                        let mut disp =
                            ChunkColumnDispenser::new_with_batch(&mut entity_iter, &batch);
                        assert_eq!(
                            disp.next_write::<u32>(Some(UuidFilter::exact(uuid_wrong))),
                            Err(FfiResult::TYPE_MISMATCH)
                        );
                    }
                    {
                        let mut disp =
                            ChunkColumnDispenser::new_with_batch(&mut entity_iter, &batch);
                        assert_eq!(
                            disp.next_read::<u32>(
                                AccessMode::CHANGED,
                                Some(UuidFilter::exact(uuid_w))
                            ),
                            Err(FfiResult::TYPE_MISMATCH)
                        );
                    }
                    assert!(
                        res_iter
                            .resource_with_ticks_checked::<u64>(
                                0,
                                Some(UuidFilter::exact(uuid_res))
                            )
                            .is_none()
                    );
                    assert!(
                        res_iter
                            .resource_with_ticks_checked::<u32>(
                                0,
                                Some(UuidFilter::exact(uuid_wrong))
                            )
                            .is_none()
                    );
                    assert!(
                        res_iter
                            .resource_with_ticks_checked::<u32>(0, None)
                            .is_some()
                    );
                    assert!(
                        res_iter
                            .resource_with_ticks_checked::<u32>(
                                0,
                                Some(UuidFilter::exact(uuid_res))
                            )
                            .is_some()
                    );
                }
                4 => {
                    // Parallel frozen cache snapshot: 0 `iter_column_info` calls on worker threads!
                    let rc = entity_iter.par_for_each_chunk(|task_iter| {
                        let mut disp = ChunkColumnDispenser::new(task_iter);
                        let _w = disp
                            .next_write::<u32>(Some(UuidFilter::exact(uuid_w)))
                            .unwrap();
                        let _r = disp
                            .next_read::<u32>(AccessMode::READ, Some(UuidFilter::exact(uuid_r)))
                            .unwrap();
                        FfiResult::OK
                    });
                    assert_eq!(rc, FfiResult::OK);
                }
                5 => {
                    // `> INLINE_COL_INFO_CAP` (10 terms): terms 0..8 cached, terms 8..10 fall back cleanly.
                    assert!(entity_iter.next_chunk().is_some());
                    let mut disp = ChunkColumnDispenser::new(&mut entity_iter);
                    let _ = disp
                        .next_write::<u32>(Some(UuidFilter::exact(uuid_w)))
                        .unwrap();
                    for _ in 1..10 {
                        let _ = disp
                            .next_read::<u32>(AccessMode::READ, Some(UuidFilter::exact(uuid_r)))
                            .unwrap();
                    }
                }
                _ => unreachable!(),
            }
            FfiResult::OK
        });

    let sys = MockSys {
        entity_iter: MockIter {
            col_info_calls: &raw const entity_info_calls,
            res_ticks_calls: &raw const res_ticks_calls,
            chunk_idx: AtomicUsize::new(0),
            chunk_count: AtomicUsize::new(2),
            col0_write: core::cell::UnsafeCell::new([1, 2]),
            col1_read: [10, 20],
            res_val: 7,
            is_resource: false,
        },
        res_iter: MockIter {
            col_info_calls: &raw const res_info_calls,
            res_ticks_calls: &raw const res_ticks_calls,
            chunk_idx: AtomicUsize::new(0),
            chunk_count: AtomicUsize::new(0),
            col0_write: core::cell::UnsafeCell::new([0, 0]),
            col1_read: [0, 0],
            res_val: 77,
            is_resource: true,
        },
    };
    let sys_ctx = (&raw const sys).cast_mut().cast::<BamSystemCtx>();

    // Tick 1: 2 entity info calls (1 per term across 2 chunks) + 1 resource info call (across 64 calls).
    // SAFETY: `sys_ctx` and `sys_ud` are valid for this call.
    assert_eq!(unsafe { sys_fn(sys_ctx, sys_ud) }, FfiResult::OK);
    assert_eq!(entity_info_calls.load(Ordering::SeqCst), 2);
    assert_eq!(res_info_calls.load(Ordering::SeqCst), 1);
    assert_eq!(res_ticks_calls.load(Ordering::SeqCst), 64);

    // Tick 2: 0 steady-state `iter_column_info` calls for BOTH entity query AND 64-call resource loop!
    sys.entity_iter.chunk_idx.store(0, Ordering::SeqCst);
    // SAFETY: `sys_ctx` and `sys_ud` are valid for this call.
    assert_eq!(unsafe { sys_fn(sys_ctx, sys_ud) }, FfiResult::OK);
    assert_eq!(entity_info_calls.load(Ordering::SeqCst), 2);
    assert_eq!(res_info_calls.load(Ordering::SeqCst), 1);
    assert_eq!(res_ticks_calls.load(Ordering::SeqCst), 128);

    // Tick 3: mismatch paths hit cached `BamColumnInfo` (0 FFI calls) and still reject safely!
    sys.entity_iter.chunk_idx.store(0, Ordering::SeqCst);
    // SAFETY: `sys_ctx` and `sys_ud` are valid for this call.
    assert_eq!(unsafe { sys_fn(sys_ctx, sys_ud) }, FfiResult::OK);
    assert_eq!(entity_info_calls.load(Ordering::SeqCst), 2);
    assert_eq!(res_info_calls.load(Ordering::SeqCst), 1);

    // Tick 4: `par_for_each_chunk` reads frozen cache with 0 `iter_column_info` calls across workers!
    sys.entity_iter.chunk_idx.store(0, Ordering::SeqCst);
    // SAFETY: `sys_ctx` and `sys_ud` are valid for this call.
    assert_eq!(unsafe { sys_fn(sys_ctx, sys_ud) }, FfiResult::OK);
    assert_eq!(entity_info_calls.load(Ordering::SeqCst), 2);

    // Tick 5: terms 0..2 hit cache, terms 2..10 warm/fall back cleanly (8 new info calls: 2..10).
    sys.entity_iter.chunk_idx.store(0, Ordering::SeqCst);
    sys.entity_iter.chunk_count.store(1, Ordering::SeqCst);
    // SAFETY: `sys_ctx` and `sys_ud` are valid for this call.
    assert_eq!(unsafe { sys_fn(sys_ctx, sys_ud) }, FfiResult::OK);
    assert_eq!(entity_info_calls.load(Ordering::SeqCst), 10);
}
