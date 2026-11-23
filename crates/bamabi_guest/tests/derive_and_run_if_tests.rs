//! Integration tests for:
//! - `#[derive(GuestComponent)]`, `#[mod_component(..., guest)]`, `summary_tick`, and bit-for-bit
//!   UUID parity against host `#[derive(ModComponent)]` (under `#[cfg(feature = "derive")]`).
//! - Chainable AND-composed `ObserverBuilder::run_if` / `run_if_fn` (both `build` and
//!   `build_stoppable`, plus panic containment).
//! - Fixed chainable AND-composed `SystemBuilder::run_if` / `run_if_fn` (no overwrite/leak of
//!   earlier predicates).

#![allow(unsafe_code, clippy::undocumented_unsafe_blocks)]

mod common;

use core::ffi::c_void;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use bamabi_guest::prelude::*;
use bamabi_guest::raw::{
    BAM_STORAGE_FLAG_RESOURCE, BamCommands, BamLoadCtx, BamObserverCtxFn, BamObserverDesc,
    BamObserverFn, BamObserverMutFn, BamTermDesc,
};
use common::MockHostWorld;

/// Snapshot of a host-registered `BamObserverDesc` captured by [`stub_register_observer`].
#[derive(Clone)]
struct CapturedObserver {
    name: Option<String>,
    event_kind: BamEventKind,
    flags: u32,
    watched_components: Vec<BamComponent>,
    queries: Vec<Vec<BamTermDesc>>,
    query_count: usize,
    param_set_groups: Vec<u32>,
    callback: Option<BamObserverFn>,
    mut_callback: Option<BamObserverMutFn>,
    ctx_callback: Option<BamObserverCtxFn>,
    user_data: *mut c_void,
}

// SAFETY: Test-only storage of host-captured observer descriptors on the test thread.
unsafe impl Send for CapturedObserver {}

static CAPTURED_OBSERVERS: OnceLock<Mutex<Vec<CapturedObserver>>> = OnceLock::new();
static OBSERVER_CAPTURE_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Returns the global mutex-protected list of observers captured during observer tests.
fn captured_observers() -> &'static Mutex<Vec<CapturedObserver>> {
    CAPTURED_OBSERVERS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Mock `BamHostApi::register_observer` callback recording registered observer descriptors.
unsafe extern "C" fn stub_register_observer(
    load_ctx: *mut BamLoadCtx,
    desc: *const BamObserverDesc,
) -> FfiResult {
    if load_ctx.is_null() || desc.is_null() {
        return FfiResult::NULL_POINTER;
    }
    let d = unsafe { *desc };
    let val = d.validate();
    if !val.is_ok() {
        return val;
    }
    let name = if d.name.is_null() {
        None
    } else {
        Some(
            unsafe { core::ffi::CStr::from_ptr(d.name) }
                .to_str()
                .unwrap()
                .to_owned(),
        )
    };
    let watched_components = if d.watched_component_count == 0 {
        Vec::new()
    } else {
        unsafe { core::slice::from_raw_parts(d.watched_components, d.watched_component_count) }
            .to_vec()
    };
    let mut queries = Vec::with_capacity(d.query_count);
    let mut param_set_groups = Vec::with_capacity(d.query_count);
    if d.query_count > 0 {
        let qs = unsafe { core::slice::from_raw_parts(d.queries, d.query_count) };
        for q in qs {
            param_set_groups.push(q.param_set_group);
            let terms = if q.term_count == 0 {
                Vec::new()
            } else {
                unsafe { core::slice::from_raw_parts(q.terms, q.term_count) }.to_vec()
            };
            queries.push(terms);
        }
    }
    captured_observers()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(CapturedObserver {
            name,
            event_kind: d.event_kind,
            flags: d.flags,
            watched_components,
            queries,
            query_count: d.query_count,
            param_set_groups,
            callback: d.callback,
            mut_callback: d.mut_callback,
            ctx_callback: d.ctx_callback,
            user_data: d.user_data,
        });
    FfiResult::OK
}

/// Drop-tracking guard verifying that chained `.run_if(...)` closures are freed on shutdown.
struct DropCounter(Arc<AtomicUsize>);
impl Drop for DropCounter {
    /// Increments the shared drop counter when the captured predicate is destroyed.
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// Scenario: Chains three `.run_if` / `.run_if_fn` predicates on a `SystemBuilder`, verifying
/// short-circuit logical AND evaluation order across all truth-table transitions and asserting
/// that all three closures are dropped cleanly on `unload_instance` with zero leaks.
#[test]
fn system_builder_chained_run_if_composes_and_drops_cleanly() {
    let world = MockHostWorld::new();

    let p1_flag = Arc::new(AtomicBool::new(false));
    let p2_flag = Arc::new(AtomicBool::new(true));
    let p3_flag = Arc::new(AtomicBool::new(true));
    let p1_calls = Arc::new(AtomicUsize::new(0));
    let p2_calls = Arc::new(AtomicUsize::new(0));
    let p3_calls = Arc::new(AtomicUsize::new(0));
    let sys_calls = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));

    let f1 = Arc::clone(&p1_flag);
    let f2 = Arc::clone(&p2_flag);
    let f3 = Arc::clone(&p3_flag);
    let c1 = Arc::clone(&p1_calls);
    let c2 = Arc::clone(&p2_calls);
    let c3 = Arc::clone(&p3_calls);
    let sc = Arc::clone(&sys_calls);
    let d1 = DropCounter(Arc::clone(&drops));
    let d2 = DropCounter(Arc::clone(&drops));
    let d3 = DropCounter(Arc::clone(&drops));

    let instance = world
        .load_fn(c"run_if_chain_mod", move |app| {
            app.system(c"chained_sys")
                .run_if(move |ctx| {
                    let _keep = &d1;
                    assert!(ctx.host_api().struct_size > 0);
                    c1.fetch_add(1, Ordering::SeqCst);
                    f1.load(Ordering::SeqCst)
                })
                .run_if(move |ctx| {
                    let _keep = &d2;
                    assert!(ctx.host_api().struct_size > 0);
                    c2.fetch_add(1, Ordering::SeqCst);
                    f2.load(Ordering::SeqCst)
                })
                .run_if_fn(move || {
                    let _keep = &d3;
                    c3.fetch_add(1, Ordering::SeqCst);
                    f3.load(Ordering::SeqCst)
                })
                .build(move |_ctx| {
                    sc.fetch_add(1, Ordering::SeqCst);
                })
        })
        .expect("init chained_sys");

    // Zero predicates dropped prematurely during registration.
    assert_eq!(drops.load(Ordering::SeqCst), 0);

    // 1) First predicate false -> short-circuits; p2/p3/system NOT invoked.
    assert_eq!(world.run_system(0), FfiResult::OK);
    assert_eq!(p1_calls.load(Ordering::SeqCst), 1);
    assert_eq!(p2_calls.load(Ordering::SeqCst), 0);
    assert_eq!(p3_calls.load(Ordering::SeqCst), 0);
    assert_eq!(sys_calls.load(Ordering::SeqCst), 0);

    // 2) p1 true, p2 false, p3 true -> p1 & p2 invoked; p3 & system skipped.
    p1_flag.store(true, Ordering::SeqCst);
    p2_flag.store(false, Ordering::SeqCst);
    assert_eq!(world.run_system(0), FfiResult::OK);
    assert_eq!(p1_calls.load(Ordering::SeqCst), 2);
    assert_eq!(p2_calls.load(Ordering::SeqCst), 1);
    assert_eq!(p3_calls.load(Ordering::SeqCst), 0);
    assert_eq!(sys_calls.load(Ordering::SeqCst), 0);

    // 3) p1 true, p2 true, p3 false -> all 3 evaluated; system skipped.
    p2_flag.store(true, Ordering::SeqCst);
    p3_flag.store(false, Ordering::SeqCst);
    assert_eq!(world.run_system(0), FfiResult::OK);
    assert_eq!(p1_calls.load(Ordering::SeqCst), 3);
    assert_eq!(p2_calls.load(Ordering::SeqCst), 2);
    assert_eq!(p3_calls.load(Ordering::SeqCst), 1);
    assert_eq!(sys_calls.load(Ordering::SeqCst), 0);

    // 4) All 3 true -> system runs!
    p3_flag.store(true, Ordering::SeqCst);
    assert_eq!(world.run_system(0), FfiResult::OK);
    assert_eq!(p1_calls.load(Ordering::SeqCst), 4);
    assert_eq!(p2_calls.load(Ordering::SeqCst), 3);
    assert_eq!(p3_calls.load(Ordering::SeqCst), 2);
    assert_eq!(sys_calls.load(Ordering::SeqCst), 1);

    // All 3 chained predicates dropped cleanly on shutdown (zero leak).
    world.unload_instance(instance);
    assert_eq!(drops.load(Ordering::SeqCst), 3);
}

/// Scenario: Registers three `Ready` observers via `ObserverBuilder` verifying (1) short-circuit
/// AND evaluation of chained `.run_if` / `.run_if_fn` predicates, (2) that a gated-off
/// `build_stoppable` observer leaves `*propagate == TRUE` untouched while an active one stops
/// propagation, and (3) that a panicking predicate is contained as `FfiResult::PANIC_CAUGHT`.
#[test]
fn observer_builder_run_if_chains_controls_propagation_and_catches_panics() {
    let _guard = OBSERVER_CAPTURE_TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    captured_observers()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clear();

    let mut world = MockHostWorld::new();
    world.expose_component::<FfiEntity>(
        "bevy_scene::Ready",
        ComponentUuid::from_canonical_name("bevy_scene::Ready"),
        0,
        false,
    );
    world.api.register_observer = Some(stub_register_observer);

    let gate_a = Arc::new(AtomicBool::new(false));
    let gate_b = Arc::new(AtomicBool::new(true));
    let b_evals = Arc::new(AtomicUsize::new(0));
    let lifecycle_runs = Arc::new(AtomicUsize::new(0));
    let stoppable_runs = Arc::new(AtomicUsize::new(0));

    let ga1 = Arc::clone(&gate_a);
    let gb1 = Arc::clone(&gate_b);
    let be1 = Arc::clone(&b_evals);
    let lr = Arc::clone(&lifecycle_runs);

    let ga2 = Arc::clone(&gate_a);
    let sr = Arc::clone(&stoppable_runs);

    let instance = world
        .load_fn(c"obs_run_if_mod", move |app| {
            // Observer 0: lifecycle `Ready` observer with chained `.run_if` + `.run_if_fn`.
            let res = ObserverBuilder::ready(app, "gated_ready")
                .run_if(move |ctx| {
                    assert!(ctx.host_api().struct_size > 0);
                    ga1.load(Ordering::SeqCst)
                })
                .run_if_fn(move || {
                    be1.fetch_add(1, Ordering::SeqCst);
                    gb1.load(Ordering::SeqCst)
                })
                .build(move |_tgt, _cmds| {
                    lr.fetch_add(1, Ordering::SeqCst);
                });
            if let Err(err) = res {
                return err;
            }

            // Observer 1: stoppable `Ready` observer verifying `*propagate` stays untouched when gated off.
            let res = ObserverBuilder::ready(app, "gated_stoppable")
                .with_propagate(true)
                .run_if_fn(move || ga2.load(Ordering::SeqCst))
                .build_stoppable(move |_tgt, propagate, _cmds| {
                    sr.fetch_add(1, Ordering::SeqCst);
                    *propagate = false;
                });
            if let Err(err) = res {
                return err;
            }

            // Observer 2: panicking predicate inside `ObserverBuilder::run_if`.
            let res = ObserverBuilder::ready(app, "panicking_pred")
                .run_if_fn(|| panic!("predicate boom"))
                .build(|_tgt, _cmds| {});
            match res {
                Ok(_) => FfiResult::OK,
                Err(err) => err,
            }
        })
        .expect("init observers");

    let observers: Vec<CapturedObserver> = captured_observers()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    assert_eq!(observers.len(), 3);
    assert_eq!(observers[0].event_kind, BamEventKind::READY);
    assert_eq!(observers[1].flags, BAM_OBSERVER_FLAG_PROPAGATE);

    let mut dummy_cmds = 0u64;
    let cmds_ptr = (&raw mut dummy_cmds).cast::<BamCommands>();
    let target = FfiEntity::from_bits(42);

    // 1) Observer 0: gate_a = false -> short-circuits before gate_b, skips callback, returns OK.
    let cb0 = observers[0].callback.expect("lifecycle callback");
    let st = unsafe { cb0(target, core::ptr::null(), cmds_ptr, observers[0].user_data) };
    assert_eq!(st, FfiResult::OK);
    assert_eq!(b_evals.load(Ordering::SeqCst), 0);
    assert_eq!(lifecycle_runs.load(Ordering::SeqCst), 0);

    // 2) Observer 0: gate_a = true, gate_b = false -> evaluates both, skips callback.
    gate_a.store(true, Ordering::SeqCst);
    gate_b.store(false, Ordering::SeqCst);
    let st = unsafe { cb0(target, core::ptr::null(), cmds_ptr, observers[0].user_data) };
    assert_eq!(st, FfiResult::OK);
    assert_eq!(b_evals.load(Ordering::SeqCst), 1);
    assert_eq!(lifecycle_runs.load(Ordering::SeqCst), 0);

    // 3) Observer 0: both true -> invokes callback.
    gate_b.store(true, Ordering::SeqCst);
    let st = unsafe { cb0(target, core::ptr::null(), cmds_ptr, observers[0].user_data) };
    assert_eq!(st, FfiResult::OK);
    assert_eq!(b_evals.load(Ordering::SeqCst), 2);
    assert_eq!(lifecycle_runs.load(Ordering::SeqCst), 1);

    // 4) Observer 1 (`build_stoppable`): gate_a = false -> skips callback AND leaves `prop == TRUE`!
    let cb1 = observers[1].mut_callback.expect("mut callback");
    let mut payload = target;
    let mut prop = FfiBool::TRUE;
    gate_a.store(false, Ordering::SeqCst);
    let st = unsafe {
        cb1(
            target,
            (&raw mut payload).cast::<u8>(),
            &raw mut prop,
            cmds_ptr,
            observers[1].user_data,
        )
    };
    assert_eq!(st, FfiResult::OK);
    assert_eq!(stoppable_runs.load(Ordering::SeqCst), 0);
    assert!(
        prop.get(),
        "skipped stoppable observer must not stop propagation"
    );

    // 5) Observer 1 (`build_stoppable`): gate_a = true -> runs callback and sets `prop = FALSE`!
    gate_a.store(true, Ordering::SeqCst);
    let st = unsafe {
        cb1(
            target,
            (&raw mut payload).cast::<u8>(),
            &raw mut prop,
            cmds_ptr,
            observers[1].user_data,
        )
    };
    assert_eq!(st, FfiResult::OK);
    assert_eq!(stoppable_runs.load(Ordering::SeqCst), 1);
    assert!(
        !prop.get(),
        "active stoppable observer should stop propagation"
    );

    // 6) Observer 2: panicking predicate caught cleanly as `PANIC_CAUGHT`.
    let cb2 = observers[2].callback.expect("panicking callback");
    let st = unsafe { cb2(target, core::ptr::null(), cmds_ptr, observers[2].user_data) };
    assert_eq!(st, FfiResult::PANIC_CAUGHT);

    world.unload_instance(instance);
}

#[cfg(feature = "derive")]
mod derive_guest_component_tests {
    use super::*;

    /// Host-side `#[derive(ModComponent)]` named struct fixture for UUID/flag parity checks.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, ModComponent)]
    #[mod_component(name = "parity::Position", summary_tick)]
    struct HostPosition {
        x: f32,
        y: f32,
        z: f32,
    }

    /// Guest-side `#[derive(GuestComponent)]` counterpart to [`HostPosition`].
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, AbiPod, GuestComponent)]
    #[guest_component(name = "parity::Position", summary_tick)]
    struct GuestPosition {
        x: f32,
        y: f32,
        z: f32,
    }

    /// Host-side tuple struct fixture using `namespace = "parity"`.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, ModComponent)]
    #[mod_component(namespace = "parity", name = "Vel")]
    struct HostVel(f32, f32);

    /// Guest-side tuple struct counterpart to [`HostVel`].
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, AbiPod, GuestComponent)]
    #[guest_component(namespace = "parity", name = "Vel")]
    struct GuestVel(f32, f32);

    /// Host-side unit marker fixture with `sparse_set` storage flag.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, ModComponent)]
    #[mod_component(name = "parity::Marker", sparse_set)]
    struct HostMarker;

    /// Guest-side unit marker counterpart to [`HostMarker`].
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, AbiPod, GuestComponent)]
    #[guest_component(name = "parity::Marker", sparse_set)]
    struct GuestMarker;

    /// Host-side nested struct fixture containing another `AbiPod` struct and a fixed array.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, ModComponent)]
    #[mod_component(name = "parity::NestedPayload")]
    struct HostNested {
        vel: HostVel,
        weights: [f32; 4],
    }

    /// Guest-side nested struct counterpart to [`HostNested`].
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, AbiPod, GuestComponent)]
    #[guest_component(name = "parity::NestedPayload")]
    struct GuestNested {
        vel: HostVel,
        weights: [f32; 4],
    }

    /// Fixture deriving both `ModComponent` and `GuestComponent` via shared `#[mod_component]` attrs.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, ModComponent, GuestComponent)]
    #[mod_component(name = "parity::DualComp", summary_tick, immutable)]
    struct DualComp {
        id: u64,
        score: f32,
        _pad: u32,
    }

    /// Fixture using the `#[mod_component(..., guest)]` shorthand to emit `GuestComponent` too.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, ModComponent)]
    #[mod_component(name = "parity::ShorthandComp", summary_tick, guest)]
    struct ShorthandComp {
        hp: f32,
        max_hp: f32,
    }

    /// Fixture opting out of structural field hashing (`no_schema_hash`) so `FIELD_SCHEMA_HASH == 0`.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, AbiPod, GuestComponent)]
    #[guest_component(name = "parity::NoSchemaHash", no_schema_hash)]
    struct NoSchemaHash {
        a: u32,
        b: u32,
    }

    /// Fixture pinning an explicit 128-bit UUID override on a `#[repr(transparent)]` newtype.
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, PartialEq, AbiPod, GuestComponent)]
    #[guest_component(
        name = "parity::ExplicitUuid",
        uuid = "01234567-89ab-cdef-0123-456789abcdef"
    )]
    struct ExplicitUuid(u64);

    /// Fixture with an unqualified default name verifying manifest auto-prefixing on registration.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, AbiPod, GuestComponent)]
    struct UnqualifiedHealth {
        hp: f32,
    }

    /// Scenario: Verifies bit-for-bit `NAME`, `UUID`, and `FLAGS` parity between host
    /// `#[derive(ModComponent)]` and guest `#[derive(GuestComponent)]` across named, tuple, unit,
    /// nested, dual-derived, shorthand, `no_schema_hash`, explicit-UUID, and manifest-qualified
    /// dynamic components registered on a `MockHostWorld`.
    #[test]
    fn uuid_parity_flags_and_host_registration_match_mod_component() {
        // 1. Named struct UUID & flags parity.
        assert_eq!(
            <GuestPosition as GuestComponent>::NAME,
            <HostPosition as ModComponent>::COMPONENT_CSTR
        );
        assert_eq!(
            <GuestPosition as GuestComponent>::UUID,
            <HostPosition as ModComponent>::UUID
        );
        assert_eq!(
            <GuestPosition as GuestComponent>::FLAGS,
            BAM_COMPONENT_FLAG_SUMMARY_TICK
        );

        // 2. Tuple struct with `namespace = "..."`.
        assert_eq!(
            <GuestVel as GuestComponent>::NAME,
            <HostVel as ModComponent>::COMPONENT_CSTR
        );
        assert_eq!(
            <GuestVel as GuestComponent>::UUID,
            <HostVel as ModComponent>::UUID
        );

        // 3. Unit struct with `sparse_set`.
        assert_eq!(
            <GuestMarker as GuestComponent>::UUID,
            <HostMarker as ModComponent>::UUID
        );
        assert_eq!(
            <GuestMarker as GuestComponent>::FLAGS,
            BAM_COMPONENT_FLAG_SPARSE_SET
        );

        // 4. Nested AbiPod + array field parity.
        assert_eq!(
            <GuestNested as GuestComponent>::UUID,
            <HostNested as ModComponent>::UUID
        );

        // 5. `#[derive(ModComponent, GuestComponent)]` fallback to `#[mod_component(...)]`.
        assert_eq!(
            <DualComp as GuestComponent>::NAME,
            <DualComp as ModComponent>::COMPONENT_CSTR
        );
        assert_eq!(
            <DualComp as GuestComponent>::UUID,
            <DualComp as ModComponent>::UUID
        );
        assert_eq!(
            <DualComp as GuestComponent>::FLAGS,
            BAM_COMPONENT_FLAG_SUMMARY_TICK | BAM_COMPONENT_FLAG_IMMUTABLE
        );

        // 6. `#[mod_component(..., guest)]` shorthand.
        assert_eq!(
            <ShorthandComp as GuestComponent>::NAME,
            <ShorthandComp as ModComponent>::COMPONENT_CSTR
        );
        assert_eq!(
            <ShorthandComp as GuestComponent>::UUID,
            <ShorthandComp as ModComponent>::UUID
        );
        assert_eq!(
            <ShorthandComp as GuestComponent>::FLAGS,
            BAM_COMPONENT_FLAG_SUMMARY_TICK
        );

        // 7. `no_schema_hash` & explicit `uuid`.
        assert_eq!(<NoSchemaHash as GuestComponent>::FIELD_SCHEMA_HASH, 0);
        assert_eq!(
            <NoSchemaHash as GuestComponent>::UUID,
            ComponentUuid::from_parts("parity::NoSchemaHash", 8, 4, 0)
        );
        assert_eq!(
            <ExplicitUuid as GuestComponent>::UUID,
            ComponentUuid::parse_hex("01234567-89ab-cdef-0123-456789abcdef")
        );

        // 8. Unqualified default name (`c"UnqualifiedHealth"`) auto-prefixes manifest name during registration.
        assert_eq!(
            <UnqualifiedHealth as GuestComponent>::NAME,
            c"UnqualifiedHealth"
        );

        let world = MockHostWorld::new();
        let instance = world
            .load_fn(c"my_mod", |app| {
                let h_pos = match app.register_component::<GuestPosition>() {
                    Ok(h) => h,
                    Err(e) => return e,
                };
                let h_short = match app.register_component::<ShorthandComp>() {
                    Ok(h) => h,
                    Err(e) => return e,
                };
                let h_local = match app.register_component::<UnqualifiedHealth>() {
                    Ok(h) => h,
                    Err(e) => return e,
                };
                assert!(h_pos.is_valid());
                assert!(h_short.is_valid());
                assert!(h_local.is_valid());
                FfiResult::OK
            })
            .expect("register derived guest components");

        let pos_meta = world
            .components
            .iter()
            .find(|c| c.name == "parity::Position")
            .expect("registered parity::Position");
        assert_eq!(pos_meta.uuid, <HostPosition as ModComponent>::UUID);
        assert_eq!(pos_meta.flags, BAM_COMPONENT_FLAG_SUMMARY_TICK);

        let local_meta = world
            .components
            .iter()
            .find(|c| c.name == "my_mod::UnqualifiedHealth")
            .expect("manifest-qualified UnqualifiedHealth");
        let expected_local_uuid = ComponentUuid::from_parts(
            "my_mod::UnqualifiedHealth",
            core::mem::size_of::<UnqualifiedHealth>(),
            core::mem::align_of::<UnqualifiedHealth>(),
            <UnqualifiedHealth as GuestComponent>::FIELD_SCHEMA_HASH,
        );
        assert_eq!(local_meta.uuid, expected_local_uuid);

        world.unload_instance(instance);
    }
}

/// Scenario: Verifies `TypedObserverBuilder` context-aware observers (`.with_watched_handle`,
/// `.query`, `.param_set`, `.resource_read_handle`, `.run_if_fn`, `.build_with_ctx`), fail-closed
/// rejection when `.build` / `.build_mut` is called with declared queries or resources, and
/// `watched_components` forwarding through `.build(...)` and `.build_with_ctx(...)`.
#[test]
fn typed_observer_builder_ctx_queries_watched_and_validation_guards() {
    let _guard = OBSERVER_CAPTURE_TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    captured_observers()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clear();

    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct TestEvt(u64);
    // SAFETY: `TestEvt` is a `u64` with zero padding.
    unsafe impl RawInsertable for TestEvt {}
    // SAFETY: `TestEvt` has fixed C layout and `AbiPod` inner field.
    unsafe impl AbiPod for TestEvt {}

    let mut world = MockHostWorld::new();
    let comp_a = world.expose_component::<u32>(
        "obs_ctx::CompA",
        ComponentUuid::from_canonical_name("obs_ctx::CompA"),
        0,
        false,
    );
    let comp_b = world.expose_component::<u32>(
        "obs_ctx::CompB",
        ComponentUuid::from_canonical_name("obs_ctx::CompB"),
        0,
        false,
    );
    let res_h = world.expose_component::<u32>(
        "obs_ctx::Res",
        ComponentUuid::from_canonical_name("obs_ctx::Res"),
        BAM_STORAGE_FLAG_RESOURCE,
        false,
    );
    world.insert_resource(
        "obs_ctx::Res",
        ComponentUuid::from_canonical_name("obs_ctx::Res"),
        99u32,
    );
    let ev_h = world.expose_component::<TestEvt>(
        "obs_ctx::TestEvt",
        ComponentUuid::from_canonical_name("obs_ctx::TestEvt"),
        0,
        false,
    );
    world.api.register_observer = Some(stub_register_observer);

    let gate = Arc::new(AtomicBool::new(true));
    let gate_clone = Arc::clone(&gate);
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_clone = Arc::clone(&calls);

    let instance = world
        .load_fn(c"typed_obs_ctx_mod", move |app| {
            // 1. `.query(...)` + `.build(...)` must fail closed with `INVALID_ARGUMENT`.
            let err_ro = app
                .observe_event_handle::<TestEvt>("bad_ro", ev_h)
                .query(|q| q.read_handle(comp_a))
                .build(|_tgt, _ev, _cmds| {});
            assert_eq!(err_ro.err(), Some(FfiResult::INVALID_ARGUMENT));

            // 2. `.resource_read_handle(...)` + `.build_mut(...)` must fail closed with `INVALID_ARGUMENT`.
            let err_mut = app
                .observe_event_handle::<TestEvt>("bad_mut", ev_h)
                .resource_read_handle(res_h)
                .build_mut(|_tgt, _ev, _prop, _cmds| {});
            assert_eq!(err_mut.err(), Some(FfiResult::INVALID_ARGUMENT));

            // 3. `.with_watched_handle` on `.build(...)` forwards watched handle cleanly.
            if let Err(err) = app
                .observe_event_handle::<TestEvt>("ro_watched", ev_h)
                .with_watched_handle(comp_a)
                .build(|_tgt, _ev, _cmds| {})
            {
                return err;
            }

            // 4. Full `.build_with_ctx(...)` with watched handle, param_set (2 queries) + res (slot 2 -> bucket 4).
            let ctx_calls = Arc::clone(&calls_clone);
            let ctx_gate = Arc::clone(&gate_clone);
            if let Err(err) = app
                .observe_event_handle::<TestEvt>("ctx_watched_ps", ev_h)
                .with_watched_components(&[comp_a, comp_b, comp_a])
                .param_set(|ps| {
                    ps.query(|q| q.read_handle(comp_a))
                        .query(|q| q.write_handle(comp_a));
                })
                .resource_read_handle(res_h)
                .run_if_fn(move || ctx_gate.load(Ordering::SeqCst))
                .build_with_ctx(move |tgt, ev, ctx| {
                    assert_eq!(tgt, FfiEntity::from_bits(55));
                    assert_eq!(ctx.query_count(), 2);
                    assert_eq!(ctx.resource::<u32>(0), Some(&99u32));
                    ctx_calls.fetch_add(ev.0 as usize, Ordering::SeqCst);
                })
            {
                return err;
            }

            FfiResult::OK
        })
        .expect("register typed ctx observers");

    let observers = captured_observers()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    assert_eq!(observers.len(), 2);
    assert_eq!(observers[0].name.as_deref(), Some("ro_watched"));
    assert_eq!(observers[0].watched_components, vec![comp_a]);
    assert!(observers[0].callback.is_some());
    assert!(observers[0].ctx_callback.is_none());

    assert_eq!(observers[1].name.as_deref(), Some("ctx_watched_ps"));
    assert_eq!(observers[1].watched_components, vec![comp_a, comp_b]);
    assert!(observers[1].callback.is_none());
    assert!(observers[1].mut_callback.is_none());
    assert_eq!(observers[1].query_count, 3);
    assert_eq!(observers[1].param_set_groups, vec![1, 1, 0]);
    let ctx_cb = observers[1].ctx_callback.expect("ctx_callback populated");

    let ev = TestEvt(12);
    let rc = world.with_mock_system_frame(&observers[1].queries, |sys_ptr| unsafe {
        ctx_cb(
            FfiEntity::from_bits(55),
            (&raw const ev).cast::<c_void>(),
            sys_ptr,
            observers[1].user_data,
        )
    });
    assert_eq!(rc, FfiResult::OK);
    assert_eq!(calls.load(Ordering::SeqCst), 12);

    // Gate off -> run_if skips body without error.
    gate.store(false, Ordering::SeqCst);
    let rc2 = world.with_mock_system_frame(&observers[1].queries, |sys_ptr| unsafe {
        ctx_cb(
            FfiEntity::from_bits(55),
            (&raw const ev).cast::<c_void>(),
            sys_ptr,
            observers[1].user_data,
        )
    });
    assert_eq!(rc2, FfiResult::OK);
    assert_eq!(calls.load(Ordering::SeqCst), 12);

    world.unload_instance(instance);
}
