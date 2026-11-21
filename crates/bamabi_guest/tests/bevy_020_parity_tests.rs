//! End-to-end mock-host integration & edge-case tests verifying Bevy `0.20` feature parity in
//! `bamabi_guest` and `bamabi_guest_sys` (executed under both `--no-default-features` and
//! `--all-features`).

#![allow(
    dead_code,
    unsafe_code,
    clippy::undocumented_unsafe_blocks,
    clippy::vec_box
)]

use core::{
    ffi::{CStr, c_char, c_void},
    sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
};
use std::collections::{HashMap, HashSet};

use bamabi_guest::{
    build_guest_fn,
    prelude::*,
    raw::{
        BAM_STORAGE_FLAG_MESSAGE, BAM_STORAGE_FLAG_RESOURCE, BamCommands, BamHostApi, BamHostCtx,
        BamInstance, BamIter, BamLoadCtx, BamObserverDesc, BamObserverFn, BamObserverMutFn,
        BamRunIfFn, BamSystemCtx, BamSystemDesc, BamSystemFn, BamTermDesc, ComponentDesc,
        MappedWireVTable, VTableHeader, init_guest_instance, shutdown_guest_instance,
    },
};

// ============================================================================
// Pure-value & Edge-Case Tests (Zero Unsafe, No Default Features Required)
// ============================================================================

/// Scenario: Verifies that `BevySchedules::DEFAULT` maps all 16 Bevy 0.20 schedules cleanly
/// without colliding with reserved ABI 0.3.0 custom schedule ID ranges.
#[test]
fn bevy_schedules_maps_all_16_schedules_without_violating_abi_030_reserved_range() {
    let default = BevySchedules::DEFAULT;
    assert_eq!(default.update(), ScheduleId::UPDATE);
    assert_eq!(default.startup(), ScheduleId::STARTUP);
    assert_eq!(default.pre_update(), ScheduleId::PRE_UPDATE);
    assert_eq!(default.post_update(), ScheduleId::POST_UPDATE);
    assert_eq!(default.fixed_update(), ScheduleId::FIXED_UPDATE);
    assert_eq!(default.last(), ScheduleId::LAST);

    let all_16 = [
        default.pre_startup(),
        default.startup(),
        default.post_startup(),
        default.first(),
        default.pre_update(),
        default.state_transition(),
        default.run_fixed_main_loop(),
        default.fixed_first(),
        default.fixed_pre_update(),
        default.fixed_update(),
        default.fixed_post_update(),
        default.fixed_last(),
        default.update(),
        default.spawn_scene(),
        default.post_update(),
        default.last(),
    ];
    let unique: HashSet<ScheduleId> = all_16.into_iter().collect();
    assert_eq!(
        unique.len(),
        16,
        "all 16 Bevy 0.20 schedules must be distinct"
    );
    for s in all_16 {
        assert!(s.is_valid(), "schedule {s} must be valid under ABI 0.3.0");
    }

    // Reserved 6..1024 and overflow bases are rejected cleanly; maximum valid base `u32::MAX - 9`
    // (10 extended slots: `base..=base+9 == u32::MAX`) is accepted.
    assert!(BevySchedules::with_custom_base(ScheduleId(0)).is_none());
    assert!(BevySchedules::with_custom_base(ScheduleId(6)).is_none());
    assert!(BevySchedules::with_custom_base(ScheduleId(1023)).is_none());
    assert!(BevySchedules::with_custom_base(ScheduleId(u32::MAX)).is_none());
    assert!(BevySchedules::with_custom_base(ScheduleId::custom(u32::MAX - 8)).is_none());
    let max_valid = BevySchedules::with_custom_base(ScheduleId::custom(u32::MAX - 9))
        .expect("u32::MAX - 9 fits all 10 extended slots (base..=u32::MAX)");
    assert_eq!(max_valid.spawn_scene(), ScheduleId::custom(u32::MAX));

    let custom = BevySchedules::with_custom_base(ScheduleId::custom(4096)).unwrap();
    assert_eq!(custom.custom_base(), ScheduleId::custom(4096));
    assert_eq!(custom.state_transition(), ScheduleId::custom(4099));
    assert_eq!(custom.spawn_scene(), ScheduleId::custom(4105));
}

/// Scenario: Verifies that `mod_asset_path` formats valid `mod://<id>/<rel_path>` URIs and
/// `mod_overlay_path` formats `mod://overlay/<rel_path>` matching `ModAssetOverlayRegistry`, while
/// rejecting path traversal (`..`), reserved `"overlay"`, absolute paths, backslashes, and
/// malformed mod IDs.
#[test]
fn isolated_asset_path_validation_rejects_traversal_and_malformed_inputs() {
    assert_eq!(
        mod_asset_path("my_mod-1", "textures/hero.png").unwrap(),
        "mod://my_mod-1/textures/hero.png"
    );
    assert_eq!(
        mod_overlay_path("shaders/custom.wesl").unwrap(),
        "mod://overlay/shaders/custom.wesl"
    );
    assert_eq!(OVERLAY_NAMESPACE, "overlay");

    // Malformed namespaces + reserved `"overlay"` namespace
    for bad_ns in ["", "overlay", "bad/ns", "bad ns", "../evil", "a.b", "ns\0"] {
        assert_eq!(
            mod_asset_path(bad_ns, "ok.png"),
            Err(FfiResult::INVALID_ARGUMENT),
            "namespace `{bad_ns:?}` must be rejected"
        );
    }

    // Traversal, absolute, backslash, NUL, empty segment, scheme injection
    for bad_rel in [
        "",
        "/etc/passwd",
        "../secret.txt",
        "a/../b.png",
        "./a.png",
        "a//b.png",
        "a\\b.png",
        "a\0b.png",
        "file://etc/passwd",
    ] {
        assert_eq!(
            mod_asset_path("valid_mod", bad_rel),
            Err(FfiResult::INVALID_ARGUMENT),
            "path `{bad_rel:?}` must be rejected"
        );
        assert_eq!(
            mod_overlay_path(bad_rel),
            Err(FfiResult::INVALID_ARGUMENT),
            "overlay path `{bad_rel:?}` must be rejected"
        );
    }
}

/// Scenario: Exercises Bevy 0.20 wire-type math helpers (`FfiTransform`, quaternions, degenerate
/// `looking_to`/`looking_at` edge cases, color/UI/audio/camera/picking/telemetry types)
/// unconditionally without `feature = "derive"`.
#[test]
fn bevy_020_wire_types_math_and_edge_cases_work_unconditionally() {
    // 1. FfiTransform math + degenerate looking_to / looking_at edge cases
    let mut t = FfiTransform::from_xyz(1.0, 0.0, 0.0);
    t.rotate_y(core::f32::consts::FRAC_PI_2);
    let r = t.right();
    assert!(r[0].abs() < 1e-5);
    assert!((r[2] - (-1.0)).abs() < 1e-5);

    let mut orbit = FfiTransform::from_xyz(2.0, 0.0, 0.0);
    orbit.translate_around(
        [0.0, 0.0, 0.0],
        FfiTransform::from_rotation_y(core::f32::consts::FRAC_PI_2).rotation,
    );
    assert!(orbit.translation[0].abs() < 1e-5);
    assert!((orbit.translation[2] - (-2.0)).abs() < 1e-5);

    // Collinear up/direction and zero-length direction must not produce NaN.
    let col = FfiTransform::IDENTITY.looking_to([0.0, 1.0, 0.0], [0.0, 1.0, 0.0]);
    assert!(col.rotation.iter().all(|c| c.is_finite()));
    let zero = FfiTransform::IDENTITY.looking_to([0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
    assert!(zero.rotation.iter().all(|c| c.is_finite()));

    // Composition
    let parent = FfiTransform::from_xyz(10.0, 0.0, 0.0).with_scale([2.0, 2.0, 2.0]);
    let child = FfiTransform::from_xyz(1.0, 2.0, 3.0);
    let composed = parent.mul_transform(child);
    assert_eq!(composed.translation, [12.0, 4.0, 6.0]);

    // 2. Time negative/NaN duration safety
    let bad_time = FfiTimeVirtual {
        delta_secs_f64: -1.0,
        elapsed_secs_f64: f64::NAN,
        delta_secs: -1.0,
        elapsed_secs: f32::NAN,
        relative_speed: 1.0,
        paused: FfiBool::TRUE,
        reserved0: [0; 3],
    };
    assert_eq!(bad_time.delta(), core::time::Duration::ZERO);
    assert_eq!(bad_time.elapsed(), core::time::Duration::ZERO);
    assert!(bad_time.is_paused());

    // 3. Audio playback settings matching `bamabi_bridges::audio`
    let audio = FfiAudioPlaybackSettings::looped()
        .with_volume_linear(0.5)
        .with_spatial(true);
    assert_eq!(audio.mode, PLAYBACK_MODE_LOOP);
    assert_eq!(audio.volume_mode, VOLUME_LINEAR);
    assert!((audio.volume - 0.5).abs() < 1e-6);
    assert!(audio.spatial.get());
    let db_audio = FfiAudioPlaybackSettings::default().with_volume_decibels(-6.0);
    assert_eq!(db_audio.volume_mode, VOLUME_DECIBELS);
    assert!((db_audio.volume - (-6.0)).abs() < 1e-6);

    // 4. AppExit::error(0) clamps to non-zero error code
    assert!(FfiAppExit::SUCCESS.is_success());
    let err0 = FfiAppExit::error(0);
    assert!(!err0.is_success());
    assert_eq!(err0.code, 1);
    assert_eq!(FfiAppExit::error(42).code, 42);

    // 5. Real `bamabi_reflect::telemetry::SystemTelemetryDigest64` `"BAMT"` 64-byte LE frame
    let digest = SystemTelemetryDigest64 {
        magic: SYSTEM_TELEMETRY_MAGIC,
        version: SYSTEM_TELEMETRY_DIGEST_VERSION,
        flags: TELEMETRY_FLAG_SUMMARY_TICK_ACTIVE | TELEMETRY_FLAG_BUDGET_BREACHED,
        mod_id: 7,
        system_hash: fnv1a_64_hash_system("combat_mod", "tick"),
        frame_seq: 42,
        matched_entities: 1200,
        matched_tables: 4,
        skipped_summary_tick_tables: 3,
        chunks: 5,
        p50_us: 120,
        p99_us: 450,
        commands_queued: 16,
        reserved: 0,
    };
    let wire = digest.to_le_bytes();
    assert_eq!(wire.len(), SYSTEM_TELEMETRY_DIGEST_SIZE);
    assert_eq!(&wire[0..4], b"BAMT");
    let decoded = SystemTelemetryDigest64::from_le_bytes(&wire).expect("valid BAMT digest");
    assert_eq!(decoded, digest);
    assert_ne!(decoded.flags & TELEMETRY_FLAG_SUMMARY_TICK_ACTIVE, 0);
    assert_eq!(decoded.flags & TELEMETRY_FLAG_QUARANTINED, 0);
    // Reject bad magic and bad version cleanly
    let mut bad_magic = wire;
    bad_magic[0] = b'X';
    assert_eq!(
        SystemTelemetryDigest64::from_le_bytes(&bad_magic),
        Err(TelemetryFrameError::InvalidMagic(*b"XAMT"))
    );
    let mut bad_ver = wire;
    bad_ver[4] = 99;
    bad_ver[5] = 0;
    assert_eq!(
        SystemTelemetryDigest64::from_le_bytes(&bad_ver),
        Err(TelemetryFrameError::UnsupportedVersion(99))
    );

    // 6. State set name helper & transition event & ModStateSlab helpers
    assert_eq!(
        state_set_name("GameState", "Playing")
            .unwrap()
            .to_str()
            .unwrap(),
        "host::state.GameState.Playing"
    );
    assert_eq!(
        state_set_name("", "Playing"),
        Err(FfiResult::INVALID_ARGUMENT)
    );
    assert_eq!(
        state_set_name("GameState", ""),
        Err(FfiResult::INVALID_ARGUMENT)
    );
    let trans = FfiStateTransitionEvent {
        state_slot: 0,
        exited: 1,
        entered: 2,
        has_exited: FfiBool::TRUE,
        has_entered: FfiBool::TRUE,
        allow_same_state_transitions: FfiBool::FALSE,
        reserved0: 0,
    };
    assert_eq!(trans.exited(), Some(1));
    assert_eq!(trans.entered(), Some(2));
    let slab_entry = ModStateEntry {
        state_slot: 3,
        state_generation: 1,
        current_discriminant: 2,
        flags: MOD_STATE_FLAG_ACTIVE | MOD_STATE_FLAG_MUTABLE,
    };
    assert_eq!(slab_entry.state_handle(), ModStateHandle::new(3, 1));
    assert_eq!(slab_entry.active_discriminant(), Some(2));
    assert!(slab_entry.is_mutable());
    assert!(!slab_entry.is_computed());
    let absent_entry = ModStateEntry {
        state_slot: 4,
        state_generation: 1,
        current_discriminant: STATE_ABSENT_DISCRIMINANT,
        flags: MOD_STATE_FLAG_SUB_STATE,
    };
    assert_eq!(absent_entry.active_discriminant(), None);
    assert!(absent_entry.is_sub_state());

    // 7. UI Bevy 0.20 Val::Em / Val::Rem, FontSize::Rem (5, NOT FfiVal::UNIT_REM=8!), circular/elliptical BorderRadius
    let node = FfiUiNode {
        width: FfiVal::rem(12.0),
        height: FfiVal::em(2.5),
        padding: FfiUiRect::axes(FfiVal::px(8.0), FfiVal::rem(0.5)),
        ..Default::default()
    };
    assert_eq!(node.width.unit, FfiVal::UNIT_REM);
    assert_eq!(node.height.unit, FfiVal::UNIT_EM);
    assert_eq!(FfiFontSize::rem(1.25).unit, FfiFontSize::UNIT_REM);
    assert_eq!(FfiFontSize::UNIT_REM, 5);
    let circ = FfiCornerRadius::circular(FfiVal::px(6.0));
    assert_eq!(circ.x.value, 6.0);
    assert_eq!(circ.y.unit, FfiVal::UNIT_AUTO);
    let br = FfiBorderRadius {
        top_left: FfiCornerRadius::elliptical(FfiVal::px(10.0), FfiVal::px(4.0)),
        ..FfiBorderRadius::px(6.0)
    };
    assert_eq!(br.top_left.x.value, 10.0);
    assert_eq!(br.top_left.y.value, 4.0);
    assert_eq!(br.bottom_right.y.unit, FfiVal::UNIT_AUTO);

    // 8. Picking flattened events (exact host layout) & 72-byte Gamepad state helpers
    let press = FfiPointerPress {
        target: FfiEntity::from_bits(7),
        hit_entity: FfiEntity::from_bits(7),
        hit_camera: FfiEntity::from_bits(9),
        window: FfiEntity::from_bits(1),
        pointer_id: 0,
        pointer_id_hi: 0,
        position: [120.0, 240.0],
        hit_depth: 1.5,
        hit_position: [1.0, 2.0, 3.0],
        hit_normal: [0.0, 1.0, 0.0],
        pointer_kind: FfiPointerKind::MOUSE_RAW,
        button: FfiPointerButton::PRIMARY_RAW,
        count: 1,
        has_hit_position: FfiBool::TRUE,
        has_hit_normal: FfiBool::FALSE,
        reserved0: [0; 7],
    };
    assert_eq!(press.hit_position_opt(), Some([1.0, 2.0, 3.0]));
    assert_eq!(press.hit_normal_opt(), None);
    assert_eq!(FfiPointerButton::NONE_RAW, 0);
    assert_eq!(FfiPointerButton::PRIMARY_RAW, 1);
    assert_eq!(FfiPointerButton::SECONDARY_RAW, 2);
    assert_eq!(FfiPointerButton::MIDDLE_RAW, 3);
    assert_eq!(FfiEntityCursor::HELP.icon, 2);
    assert_eq!(FfiEntityCursor::POINTER.icon, 3);
    assert_eq!(FfiEntityCursor::ZOOM_OUT.icon, 33);

    let gp = FfiGamepadState {
        pressed: FfiGamepadButton::SOUTH.bit_mask(),
        just_pressed: FfiGamepadButton::SOUTH.bit_mask(),
        left_stick: [0.5, -0.25],
        right_stick: [0.75, 0.1],
        connected: FfiBool::TRUE,
        ..Default::default()
    };
    assert!(gp.pressed(FfiGamepadButton::SOUTH));
    assert!(gp.just_pressed(FfiGamepadButton::SOUTH));
    assert!(!gp.pressed(FfiGamepadButton::NORTH));
    assert_eq!(gp.left_stick, [0.5, -0.25]);
    assert_eq!(gp.right_stick, [0.75, 0.1]);
}

// ============================================================================
// Mock ABI 0.3.0 Host Harness for End-to-End Guest SDK Verification
// ============================================================================

/// Plain `#[repr(C)]` 2D position test struct used across parity harness fixtures.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
struct TestPos {
    x: f32,
    y: f32,
}
// Use a zero-derive GuestComponent via bamabi_guest::bridges / FfiTransform or simple u32 wrapper:
// Here we use built-in FfiChildOf, FfiDisabled, FfiPointerClick, FfiCamera, FfiGizmoLine, etc.,
// which are ALL unconditional GuestComponents without needing `feature = "derive"`!

/// Registered component or resource metadata tracked by [`MockHost`].
#[derive(Clone)]
struct MockComp {
    handle: BamComponent,
    name: String,
    uuid: ComponentUuid,
    size: usize,
    align: usize,
    flags: u32,
    raw_insert_allowed: bool,
    schema_fingerprint: u64,
    field_count: usize,
    required_init_count: usize,
    required_custom_bytes: Vec<Vec<u8>>,
}

/// Registered guest system descriptor and trampolines tracked by [`MockHost`].
struct MockSystem {
    name: String,
    queries: Vec<Vec<BamTermDesc>>,
    callback: BamSystemFn,
    user_data: *mut c_void,
    run_if: Option<BamRunIfFn>,
    run_if_user_data: *mut c_void,
}

/// Registered observer descriptor and callbacks tracked by [`MockHost`].
struct MockObserver {
    name: String,
    event: BamComponent,
    target: FfiEntity,
    flags: u32,
    callback: Option<BamObserverFn>,
    mut_callback: Option<BamObserverMutFn>,
    user_data: *mut c_void,
}

/// Recorded deferred ECS command emitted into [`MockSysCtxState`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum MockCmd {
    SpawnEmpty(FfiEntity),
    Despawn(FfiEntity),
    Insert(FfiEntity, BamComponent, Vec<u8>),
    InsertDefault(FfiEntity, BamComponent),
    Remove(FfiEntity, BamComponent),
    Trigger(BamComponent, FfiEntity, Vec<u8>, u32),
}

/// Mock message entry storing either encoded POD/witness bytes or a host pointer.
enum MockMsgItem {
    Bytes(Vec<u8>),
    Ptr(*const c_void),
}

/// Per-query simulated chunk iterator state passed as `*mut BamIter` in [`MockHost`].
struct MockIterState {
    host: *mut MockHost,
    terms: Vec<BamTermDesc>,
    entities: Vec<FfiEntity>,
    // Dense 8-byte-aligned column buffers per term_index
    columns: HashMap<usize, Vec<u64>>,
    // Removed entities per term_index
    removed: HashMap<usize, Vec<FfiEntity>>,
    // Singleton 8-byte-aligned resource buffers per term_index
    resources: HashMap<usize, Vec<u64>>,
    // Buffered message queues per term_index (preserves pointer provenance for non-POD messages)
    messages: HashMap<usize, Vec<MockMsgItem>>,
    // Chunk slicing (each element is range start..end into entities/columns)
    chunks: Vec<core::ops::Range<usize>>,
    cursor: usize,
    active_chunk: Option<core::ops::Range<usize>>,
}

/// Copies `bytes` into an 8-byte-aligned `Vec<u64>` backing buffer for mock column/resource storage.
fn aligned_buf_from_bytes(bytes: &[u8]) -> Vec<u64> {
    let mut words = vec![0u64; bytes.len().div_ceil(8).max(1)];
    if !bytes.is_empty() {
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                words.as_mut_ptr().cast::<u8>(),
                bytes.len(),
            );
        }
    }
    words
}

/// Per-system execution context frame passed as `*mut BamSystemCtx` during [`MockHost`] runs.
struct MockSysCtxState {
    host: *mut MockHost,
    iters: Vec<Box<MockIterState>>,
    cmds: Vec<MockCmd>,
    next_entity: u64,
    // Entity lookup map: (query_idx, term_idx, entity) -> 8-byte-aligned storage
    entity_data: HashMap<(usize, usize, FfiEntity), Vec<u64>>,
    // Children adjacency list: (target_handle, entity) -> Vec<FfiEntity>
    children: HashMap<(BamComponent, FfiEntity), Vec<FfiEntity>>,
}

/// Full-featured ABI `0.3.0` mock host harness used by the Bevy `0.20` parity test suite.
struct MockHost {
    next_handle: u32,
    comps: Vec<MockComp>,
    systems: Vec<MockSystem>,
    observers: Vec<MockObserver>,
    symbols: HashMap<String, *const c_void>,
}

impl MockHost {
    fn new() -> Box<Self> {
        let mut h = Box::new(Self {
            next_handle: 1,
            comps: Vec::new(),
            systems: Vec::new(),
            observers: Vec::new(),
            symbols: HashMap::new(),
        });
        // Register standard built-in Bevy components used across tests
        h.add_comp("bevy_ecs::hierarchy::ChildOf", FfiChildOf::UUID, 8, 8, 0);
        h.add_comp(
            "bevy_ecs::hierarchy::Children",
            ComponentUuid::from_canonical_name("bevy_ecs::hierarchy::Children"),
            0,
            1,
            0,
        );
        // Register Disabled matching real `StdTypesBridgePlugin`'s `DisabledBridge`:
        // UUID = `from_canonical_name(BAM_DISABLED_NAME)`, `raw_insert_allowed = false`,
        // `cmd_insert_default` enabled!
        let disabled_h = h.add_comp(
            BAM_DISABLED_NAME,
            ComponentUuid::from_canonical_name(BAM_DISABLED_NAME),
            0,
            1,
            0,
        );
        h.comps
            .iter_mut()
            .find(|c| c.handle == disabled_h)
            .unwrap()
            .raw_insert_allowed = false;
        assert_eq!(
            FfiDisabled::UUID,
            ComponentUuid::from_canonical_name(BAM_DISABLED_NAME)
        );

        h.add_comp(
            "bevy_picking::events::PointerClick",
            <FfiPointerClick as GuestComponent>::UUID,
            core::mem::size_of::<FfiPointerClick>(),
            core::mem::align_of::<FfiPointerClick>(),
            0,
        );
        h.add_comp(
            "bevy_picking::events::PointerDrag",
            <FfiPointerDrag as GuestComponent>::UUID,
            core::mem::size_of::<FfiPointerDrag>(),
            core::mem::align_of::<FfiPointerDrag>(),
            0,
        );
        h.add_comp(
            "bevy_gizmos::FfiGizmoLine",
            <FfiGizmoLine as GuestComponent>::UUID,
            core::mem::size_of::<FfiGizmoLine>(),
            core::mem::align_of::<FfiGizmoLine>(),
            0,
        );
        h.add_comp(
            "bamabi::camera_light::FfiCamera2d",
            <FfiCamera2d as GuestComponent>::UUID,
            core::mem::size_of::<FfiCamera2d>(),
            core::mem::align_of::<FfiCamera2d>(),
            0,
        );
        h
    }

    fn add_comp(
        &mut self,
        name: &str,
        uuid: ComponentUuid,
        size: usize,
        align: usize,
        flags: u32,
    ) -> BamComponent {
        let handle = BamComponent::from_raw(self.next_handle);
        self.next_handle += 1;
        self.comps.push(MockComp {
            handle,
            name: name.to_owned(),
            uuid,
            size,
            align,
            flags,
            raw_insert_allowed: true,
            schema_fingerprint: 0,
            field_count: 0,
            required_init_count: 0,
            required_custom_bytes: Vec::new(),
        });
        handle
    }

    fn find_comp(&self, handle: BamComponent) -> Option<&MockComp> {
        self.comps.iter().find(|c| c.handle == handle)
    }

    fn api() -> BamHostApi {
        BamHostApi {
            struct_size: core::mem::size_of::<BamHostApi>() as u32,
            abi_version: ABI_VERSION,
            get_proc_address: Some(mock_get_proc_address),
            last_error: Some(mock_last_error),
            load_host_ctx: Some(mock_load_host_ctx),
            register_component: Some(mock_register_component),
            resolve_component: Some(mock_resolve_component),
            register_system: Some(mock_register_system),
            sys_host_ctx: Some(mock_sys_host_ctx),
            sys_query: Some(mock_sys_query),
            sys_commands: Some(mock_sys_commands),
            iter_next_chunk: Some(mock_iter_next_chunk),
            iter_reset: Some(mock_iter_reset),
            iter_entities: Some(mock_iter_entities),
            iter_column_readonly: Some(mock_iter_column_readonly),
            iter_column_mutable: Some(mock_iter_column_mutable),
            iter_column_mutable_untracked: Some(mock_iter_column_mutable),
            iter_mark_changed_range: Some(mock_iter_mark_changed_range),
            iter_column_rows_readonly: Some(mock_iter_column_rows_readonly),
            iter_column_rows_mutable: Some(mock_iter_column_rows_mutable),
            iter_column_info: Some(mock_iter_column_info),
            res_ptr: Some(mock_res_ptr),
            res_mut_ptr: Some(mock_res_mut_ptr),
            cmd_spawn_empty: Some(mock_cmd_spawn_empty),
            cmd_spawn: Some(mock_cmd_spawn),
            cmd_despawn: Some(mock_cmd_despawn),
            cmd_insert: Some(mock_cmd_insert),
            cmd_insert_default: Some(mock_cmd_insert_default),
            cmd_remove: Some(mock_cmd_remove),
            par_for_each_chunk: None,
            sys_entity_get: Some(mock_sys_entity_get),
            sys_entity_get_mut: Some(mock_sys_entity_get_mut),
            msg_read_batch: Some(mock_msg_read_batch),
            msg_send: Some(mock_msg_send),
            msg_write_batch: Some(mock_msg_write_batch),
            res_ticks: None,
            sys_res_ptr: None,
            sys_res_mut_ptr: None,
            register_observer: Some(mock_register_observer),
            cmd_trigger: Some(mock_cmd_trigger),
            cmd_spawn_persistent: Some(mock_cmd_spawn),
            cmd_spawn_batch: None,
            cmd_despawn_all: None,
            cmd_insert_if_new: None,
            cmd_trigger_ex: Some(mock_cmd_trigger_ex),
            cmd_observe: Some(mock_cmd_observe),
            iter_read_removed: Some(mock_iter_read_removed),
            sys_entity_children: Some(mock_sys_entity_children),
            asset_load: None,
            asset_get_load_state: None,
            asset_release: None,
            cmd_spawn_scene: None,
            log_diagnostic: None,
            report_guest_fault: None,
            resolve_component_by_uuid: None,
            component_layout_info: Some(mock_component_layout_info),
            register_relation: None,
            intern_symbol: None,
            bind_named_object: None,
            iter_count: Some(mock_iter_count),
            iter_up_cascade: None,
            par_for_each_chunk_readonly: None,
            register_system_handle: Some(mock_register_system_handle),
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
}

unsafe extern "C" fn mock_get_proc_address(
    ctx: *const BamHostCtx,
    name: *const c_char,
) -> *const c_void {
    if ctx.is_null() || name.is_null() {
        return core::ptr::null();
    }
    let host = unsafe { &*ctx.cast::<MockHost>() };
    let Ok(s) = unsafe { CStr::from_ptr(name) }.to_str() else {
        return core::ptr::null();
    };
    host.symbols.get(s).copied().unwrap_or(core::ptr::null())
}

unsafe extern "C" fn mock_last_error(
    _ctx: *const BamHostCtx,
    buf: *mut c_char,
    cap: usize,
) -> usize {
    if !buf.is_null() && cap > 0 {
        unsafe { *buf = 0 };
    }
    0
}

unsafe extern "C" fn mock_load_host_ctx(load_ctx: *const BamLoadCtx) -> *const BamHostCtx {
    load_ctx.cast::<BamHostCtx>()
}

unsafe extern "C" fn mock_register_component(
    load_ctx: *mut BamLoadCtx,
    desc: *const ComponentDesc,
    out: *mut BamComponent,
) -> FfiResult {
    let host = unsafe { &mut *load_ctx.cast::<MockHost>() };
    let d = unsafe { &*desc };
    let name = unsafe { CStr::from_ptr(d.name) }
        .to_str()
        .unwrap()
        .to_owned();
    let handle = BamComponent::from_raw(host.next_handle);
    host.next_handle += 1;
    let mut custom_bytes = Vec::new();
    if d.required_init_desc_count > 0 && !d.required_init_descs.is_null() {
        let slice = unsafe {
            core::slice::from_raw_parts(d.required_init_descs, d.required_init_desc_count)
        };
        for req in slice {
            if req.has_custom_bytes() && req.init_size > 0 && !req.init_bytes.is_null() {
                let bytes = unsafe { core::slice::from_raw_parts(req.init_bytes, req.init_size) };
                custom_bytes.push(bytes.to_vec());
            }
        }
    }
    if !d.vtable_ptr.is_null() {
        host.symbols
            .insert(format!("bam.v0.{name}.vtable"), d.vtable_ptr);
    }
    host.comps.push(MockComp {
        handle,
        name,
        uuid: d.uuid,
        size: d.size,
        align: d.align,
        flags: d.flags,
        raw_insert_allowed: true,
        schema_fingerprint: d.schema_fingerprint,
        field_count: d.field_count,
        required_init_count: d.required_init_desc_count,
        required_custom_bytes: custom_bytes,
    });
    unsafe { *out = handle };
    FfiResult::OK
}

unsafe extern "C" fn mock_resolve_component(
    ctx: *const BamHostCtx,
    name: *const c_char,
    uuid: *const ComponentUuid,
    out: *mut BamComponent,
) -> FfiResult {
    let host = unsafe { &*ctx.cast::<MockHost>() };
    let target_name = if name.is_null() {
        None
    } else {
        unsafe { CStr::from_ptr(name) }.to_str().ok()
    };
    let target_uuid = if uuid.is_null() {
        None
    } else {
        Some(unsafe { *uuid })
    };
    for c in &host.comps {
        let name_ok = target_name.is_none_or(|n| c.name == n);
        let uuid_ok = target_uuid.is_none_or(|u| c.uuid == u);
        if name_ok && uuid_ok {
            unsafe { *out = c.handle };
            return FfiResult::OK;
        }
        if target_name.is_some() && name_ok && !uuid_ok {
            return FfiResult::TYPE_MISMATCH;
        }
    }
    FfiResult::NOT_FOUND
}

unsafe extern "C" fn mock_component_layout_info(
    ctx: *const BamHostCtx,
    component: BamComponent,
    out_info: *mut BamLayoutInfo,
) -> FfiResult {
    let host = unsafe { &*ctx.cast::<MockHost>() };
    let Some(c) = host.find_comp(component) else {
        return FfiResult::INVALID_COMPONENT;
    };
    unsafe {
        *out_info = BamLayoutInfo {
            struct_size: core::mem::size_of::<BamLayoutInfo>() as u32,
            flags: c.flags,
            uuid: c.uuid,
            schema_fingerprint: c.schema_fingerprint,
            size: c.size,
            align: c.align,
            fields: core::ptr::null(),
            field_count: c.field_count,
        };
    }
    FfiResult::OK
}

unsafe extern "C" fn mock_register_system(
    load_ctx: *mut BamLoadCtx,
    desc: *const BamSystemDesc,
) -> FfiResult {
    let mut out = BamSystem::INVALID;
    unsafe { mock_register_system_handle(load_ctx, desc, &mut out) }
}

unsafe extern "C" fn mock_register_system_handle(
    load_ctx: *mut BamLoadCtx,
    desc: *const BamSystemDesc,
    out: *mut BamSystem,
) -> FfiResult {
    let host = unsafe { &mut *load_ctx.cast::<MockHost>() };
    let d = unsafe { &*desc };
    let name = unsafe { CStr::from_ptr(d.name) }
        .to_str()
        .unwrap()
        .to_owned();
    let mut queries = Vec::new();
    if d.query_count > 0 && !d.queries.is_null() {
        let qslice = unsafe { core::slice::from_raw_parts(d.queries, d.query_count) };
        for q in qslice {
            let terms = if q.term_count > 0 && !q.terms.is_null() {
                unsafe { core::slice::from_raw_parts(q.terms, q.term_count) }.to_vec()
            } else {
                Vec::new()
            };
            queries.push(terms);
        }
    }
    let idx = host.systems.len() as u32;
    host.systems.push(MockSystem {
        name,
        queries,
        callback: d.callback.unwrap(),
        user_data: d.user_data,
        run_if: d.run_if,
        run_if_user_data: d.run_if_user_data,
    });
    if !out.is_null() {
        unsafe { *out = BamSystem::from_raw(idx) };
    }
    FfiResult::OK
}

unsafe extern "C" fn mock_register_observer(
    load_ctx: *mut BamLoadCtx,
    desc: *const BamObserverDesc,
) -> FfiResult {
    let host = unsafe { &mut *load_ctx.cast::<MockHost>() };
    let d = unsafe { &*desc };
    let name = if d.name.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(d.name) }
            .to_str()
            .unwrap()
            .to_owned()
    };
    host.observers.push(MockObserver {
        name,
        event: d.event,
        target: d.entity(),
        flags: d.flags,
        callback: d.callback,
        mut_callback: d.mut_callback,
        user_data: d.user_data,
    });
    FfiResult::OK
}

unsafe extern "C" fn mock_sys_host_ctx(sys_ctx: *const BamSystemCtx) -> *const BamHostCtx {
    let state = unsafe { &*sys_ctx.cast::<MockSysCtxState>() };
    state.host.cast::<BamHostCtx>()
}

unsafe extern "C" fn mock_sys_query(
    sys_ctx: *mut BamSystemCtx,
    index: usize,
    out_iter: *mut *mut BamIter,
) -> FfiResult {
    let state = unsafe { &mut *sys_ctx.cast::<MockSysCtxState>() };
    if let Some(iter) = state.iters.get_mut(index) {
        unsafe { *out_iter = core::ptr::from_mut(iter.as_mut()).cast::<BamIter>() };
        FfiResult::OK
    } else {
        FfiResult::OUT_OF_RANGE
    }
}

unsafe extern "C" fn mock_sys_commands(
    sys_ctx: *mut BamSystemCtx,
    out_cmds: *mut *mut BamCommands,
) -> FfiResult {
    unsafe { *out_cmds = sys_ctx.cast::<BamCommands>() };
    FfiResult::OK
}

unsafe extern "C" fn mock_iter_next_chunk(iter: *mut BamIter, out_len: *mut usize) -> FfiBool {
    let st = unsafe { &mut *iter.cast::<MockIterState>() };
    if st.cursor >= st.chunks.len() {
        st.active_chunk = None;
        unsafe { *out_len = 0 };
        return FfiBool::FALSE;
    }
    let range = st.chunks[st.cursor].clone();
    st.cursor += 1;
    let len = range.len();
    st.active_chunk = Some(range);
    unsafe { *out_len = len };
    FfiBool::TRUE
}

unsafe extern "C" fn mock_iter_reset(iter: *mut BamIter) -> FfiResult {
    let st = unsafe { &mut *iter.cast::<MockIterState>() };
    st.cursor = 0;
    st.active_chunk = None;
    FfiResult::OK
}

unsafe extern "C" fn mock_iter_count(iter: *mut BamIter, out_count: *mut usize) -> FfiResult {
    let st = unsafe { &*iter.cast::<MockIterState>() };
    let total: usize = st.chunks.iter().map(|r| r.len()).sum();
    unsafe { *out_count = total };
    FfiResult::OK
}

unsafe extern "C" fn mock_iter_entities(iter: *const BamIter) -> *const FfiEntity {
    let st = unsafe { &*iter.cast::<MockIterState>() };
    let Some(range) = &st.active_chunk else {
        return core::ptr::null();
    };
    st.entities[range.clone()].as_ptr()
}

unsafe extern "C" fn mock_iter_column_readonly(iter: *const BamIter, col: usize) -> *const u8 {
    let st = unsafe { &*iter.cast::<MockIterState>() };
    let Some(range) = &st.active_chunk else {
        return core::ptr::null();
    };
    let Some(term) = st.terms.get(col) else {
        return core::ptr::null();
    };
    let host = unsafe { &*st.host };
    let Some(comp) = host.find_comp(term.component) else {
        return core::ptr::null();
    };
    let Some(buf) = st.columns.get(&col) else {
        return core::ptr::null();
    };
    let byte_off = range.start * comp.size;
    unsafe { buf.as_ptr().cast::<u8>().add(byte_off) }
}

unsafe extern "C" fn mock_iter_column_mutable(iter: *mut BamIter, col: usize) -> *mut u8 {
    let st = unsafe { &mut *iter.cast::<MockIterState>() };
    let Some(range) = &st.active_chunk else {
        return core::ptr::null_mut();
    };
    let Some(term) = st.terms.get(col) else {
        return core::ptr::null_mut();
    };
    let host = unsafe { &*st.host };
    let Some(comp) = host.find_comp(term.component) else {
        return core::ptr::null_mut();
    };
    let Some(buf) = st.columns.get_mut(&col) else {
        return core::ptr::null_mut();
    };
    let byte_off = range.start * comp.size;
    unsafe { buf.as_mut_ptr().cast::<u8>().add(byte_off) }
}

unsafe extern "C" fn mock_iter_mark_changed_range(
    _iter: *mut BamIter,
    _col: usize,
    _start_row: usize,
    _row_count: usize,
) -> FfiResult {
    FfiResult::OK
}

unsafe extern "C" fn mock_iter_column_rows_readonly(
    _iter: *mut BamIter,
    _col: usize,
) -> *const *const u8 {
    core::ptr::null()
}

unsafe extern "C" fn mock_iter_column_rows_mutable(
    _iter: *mut BamIter,
    _col: usize,
) -> *const *mut u8 {
    core::ptr::null()
}

unsafe extern "C" fn mock_res_ptr(iter: *const BamIter, term_index: usize) -> *const u8 {
    let st = unsafe { &*iter.cast::<MockIterState>() };
    st.resources
        .get(&term_index)
        .map_or(core::ptr::null(), |b| b.as_ptr().cast::<u8>())
}

unsafe extern "C" fn mock_res_mut_ptr(iter: *mut BamIter, term_index: usize) -> *mut u8 {
    let st = unsafe { &mut *iter.cast::<MockIterState>() };
    st.resources
        .get_mut(&term_index)
        .map_or(core::ptr::null_mut(), |b| b.as_mut_ptr().cast::<u8>())
}

unsafe extern "C" fn mock_msg_read_batch(
    iter: *mut BamIter,
    term_index: usize,
    out_buf: *mut u8,
    max_count: usize,
    out_read: *mut usize,
) -> FfiResult {
    let st = unsafe { &mut *iter.cast::<MockIterState>() };
    if term_index >= st.terms.len() {
        return FfiResult::OUT_OF_RANGE;
    }
    let Some(queue) = st.messages.get_mut(&term_index) else {
        unsafe { *out_read = 0 };
        return FfiResult::OK;
    };
    let take = max_count.min(queue.len());
    let mut byte_offset = 0usize;
    for item in queue.drain(..take) {
        match item {
            MockMsgItem::Bytes(bytes) => {
                if !bytes.is_empty() && !out_buf.is_null() {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            bytes.as_ptr(),
                            out_buf.add(byte_offset),
                            bytes.len(),
                        );
                    }
                    byte_offset += bytes.len();
                }
            }
            MockMsgItem::Ptr(ptr) => {
                if !out_buf.is_null() {
                    let n = core::mem::size_of::<*const c_void>();
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            core::ptr::from_ref(&ptr).cast::<u8>(),
                            out_buf.add(byte_offset),
                            n,
                        );
                    }
                    byte_offset += n;
                }
            }
        }
    }
    unsafe { *out_read = take };
    FfiResult::OK
}

unsafe extern "C" fn mock_msg_send(
    iter: *mut BamIter,
    term_index: usize,
    data_ptr: *const u8,
    data_len: usize,
) -> FfiResult {
    let st = unsafe { &mut *iter.cast::<MockIterState>() };
    let bytes = if data_len > 0 && !data_ptr.is_null() {
        unsafe { core::slice::from_raw_parts(data_ptr, data_len) }.to_vec()
    } else {
        Vec::new()
    };
    st.messages
        .entry(term_index)
        .or_default()
        .push(MockMsgItem::Bytes(bytes));
    FfiResult::OK
}

unsafe extern "C" fn mock_msg_write_batch(
    iter: *mut BamIter,
    term_index: usize,
    in_buf: *const u8,
    count: usize,
) -> FfiResult {
    let st = unsafe { &mut *iter.cast::<MockIterState>() };
    let Some(term) = st.terms.get(term_index) else {
        return FfiResult::OUT_OF_RANGE;
    };
    let host = unsafe { &*st.host };
    let elem_size = host.find_comp(term.component).map_or(0, |c| c.size);
    let entry = st.messages.entry(term_index).or_default();
    for i in 0..count {
        let bytes = if elem_size > 0 && !in_buf.is_null() {
            unsafe { core::slice::from_raw_parts(in_buf.add(i * elem_size), elem_size) }.to_vec()
        } else {
            Vec::new()
        };
        entry.push(MockMsgItem::Bytes(bytes));
    }
    FfiResult::OK
}

unsafe extern "C" fn mock_iter_column_info(
    iter: *const BamIter,
    col: usize,
    out: *mut BamColumnInfo,
) -> FfiResult {
    let st = unsafe { &*iter.cast::<MockIterState>() };
    let Some(term) = st.terms.get(col) else {
        return FfiResult::OUT_OF_RANGE;
    };
    let host = unsafe { &*st.host };
    let Some(comp) = host.find_comp(term.component) else {
        return FfiResult::INVALID_COMPONENT;
    };
    unsafe {
        *out = BamColumnInfo {
            component: comp.handle,
            mode: term.mode,
            uuid: comp.uuid,
            size: comp.size,
            align: comp.align,
            storage_flags: comp.flags,
            reserved0: 0,
        };
    }
    FfiResult::OK
}

unsafe extern "C" fn mock_iter_read_removed(
    iter: *mut BamIter,
    term_index: usize,
    out_entities: *mut FfiEntity,
    max_count: usize,
    out_read: *mut usize,
) -> FfiResult {
    let st = unsafe { &mut *iter.cast::<MockIterState>() };
    let Some(list) = st.removed.get_mut(&term_index) else {
        unsafe { *out_read = 0 };
        return FfiResult::OK;
    };
    let take = max_count.min(list.len());
    for (i, e) in list.drain(..take).enumerate() {
        unsafe { *out_entities.add(i) = e };
    }
    unsafe { *out_read = take };
    FfiResult::OK
}

unsafe extern "C" fn mock_sys_entity_get(
    sys_ctx: *const BamSystemCtx,
    query_index: usize,
    entity: FfiEntity,
    term_index: usize,
) -> *const u8 {
    let state = unsafe { &*sys_ctx.cast::<MockSysCtxState>() };
    state
        .entity_data
        .get(&(query_index, term_index, entity))
        .map_or(core::ptr::null(), |v| v.as_ptr().cast::<u8>())
}

unsafe extern "C" fn mock_sys_entity_get_mut(
    sys_ctx: *mut BamSystemCtx,
    query_index: usize,
    entity: FfiEntity,
    term_index: usize,
) -> *mut u8 {
    let state = unsafe { &mut *sys_ctx.cast::<MockSysCtxState>() };
    state
        .entity_data
        .get_mut(&(query_index, term_index, entity))
        .map_or(core::ptr::null_mut(), |v| v.as_mut_ptr().cast::<u8>())
}

unsafe extern "C" fn mock_sys_entity_children(
    sys_ctx: *const BamSystemCtx,
    entity: FfiEntity,
    target_handle: BamComponent,
    out_ptr: *mut *const FfiEntity,
    out_len: *mut usize,
) -> FfiResult {
    let state = unsafe { &*sys_ctx.cast::<MockSysCtxState>() };
    if entity.is_null() {
        return FfiResult::INVALID_ENTITY;
    }
    if let Some(kids) = state.children.get(&(target_handle, entity)) {
        unsafe {
            *out_ptr = kids.as_ptr();
            *out_len = kids.len();
        }
    } else {
        unsafe {
            *out_ptr = core::ptr::null();
            *out_len = 0;
        }
    }
    FfiResult::OK
}

unsafe extern "C" fn mock_cmd_spawn_empty(
    cmds: *mut BamCommands,
    out_entity: *mut FfiEntity,
) -> FfiResult {
    let state = unsafe { &mut *cmds.cast::<MockSysCtxState>() };
    let e = FfiEntity::from_bits(state.next_entity);
    state.next_entity += 1;
    state.cmds.push(MockCmd::SpawnEmpty(e));
    unsafe { *out_entity = e };
    FfiResult::OK
}

unsafe extern "C" fn mock_cmd_spawn(
    cmds: *mut BamCommands,
    components: *const BamComponent,
    data_ptrs: *const *const u8,
    count: usize,
    out_entity: *mut FfiEntity,
) -> FfiResult {
    let state = unsafe { &mut *cmds.cast::<MockSysCtxState>() };
    let host = unsafe { &*state.host };
    let e = FfiEntity::from_bits(state.next_entity);
    state.next_entity += 1;
    state.cmds.push(MockCmd::SpawnEmpty(e));
    for i in 0..count {
        let comp = unsafe { *components.add(i) };
        let ptr = unsafe { *data_ptrs.add(i) };
        let len = host.find_comp(comp).map_or(0, |c| c.size);
        let bytes = if len > 0 && !ptr.is_null() {
            unsafe { core::slice::from_raw_parts(ptr, len) }.to_vec()
        } else {
            Vec::new()
        };
        state.cmds.push(MockCmd::Insert(e, comp, bytes));
    }
    unsafe { *out_entity = e };
    FfiResult::OK
}

unsafe extern "C" fn mock_cmd_despawn(cmds: *mut BamCommands, entity: FfiEntity) -> FfiResult {
    let state = unsafe { &mut *cmds.cast::<MockSysCtxState>() };
    state.cmds.push(MockCmd::Despawn(entity));
    FfiResult::OK
}

unsafe extern "C" fn mock_cmd_insert(
    cmds: *mut BamCommands,
    entity: FfiEntity,
    component: BamComponent,
    data: *const u8,
    len: usize,
) -> FfiResult {
    let state = unsafe { &mut *cmds.cast::<MockSysCtxState>() };
    let host = unsafe { &*state.host };
    if let Some(comp) = host.find_comp(component)
        && !comp.raw_insert_allowed
    {
        return FfiResult::PERMISSION_DENIED;
    }
    let bytes = if len > 0 && !data.is_null() {
        unsafe { core::slice::from_raw_parts(data, len) }.to_vec()
    } else {
        Vec::new()
    };
    state.cmds.push(MockCmd::Insert(entity, component, bytes));
    FfiResult::OK
}

unsafe extern "C" fn mock_cmd_insert_default(
    cmds: *mut BamCommands,
    entity: FfiEntity,
    component: BamComponent,
) -> FfiResult {
    let state = unsafe { &mut *cmds.cast::<MockSysCtxState>() };
    state.cmds.push(MockCmd::InsertDefault(entity, component));
    FfiResult::OK
}

unsafe extern "C" fn mock_cmd_remove(
    cmds: *mut BamCommands,
    entity: FfiEntity,
    component: BamComponent,
) -> FfiResult {
    let state = unsafe { &mut *cmds.cast::<MockSysCtxState>() };
    state.cmds.push(MockCmd::Remove(entity, component));
    FfiResult::OK
}

unsafe extern "C" fn mock_cmd_trigger(
    cmds: *mut BamCommands,
    event: BamComponent,
    target: FfiEntity,
    data: *const u8,
    len: usize,
) -> FfiResult {
    let state = unsafe { &mut *cmds.cast::<MockSysCtxState>() };
    let bytes = if len > 0 && !data.is_null() {
        unsafe { core::slice::from_raw_parts(data, len) }.to_vec()
    } else {
        Vec::new()
    };
    state.cmds.push(MockCmd::Trigger(event, target, bytes, 0));
    FfiResult::OK
}

unsafe extern "C" fn mock_cmd_trigger_ex(
    cmds: *mut BamCommands,
    desc: *const BamTriggerDesc,
) -> FfiResult {
    let state = unsafe { &mut *cmds.cast::<MockSysCtxState>() };
    let d = unsafe { &*desc };
    let bytes = if d.data_len > 0 && !d.data_ptr.is_null() {
        unsafe { core::slice::from_raw_parts(d.data_ptr, d.data_len) }.to_vec()
    } else {
        Vec::new()
    };
    state
        .cmds
        .push(MockCmd::Trigger(d.event, d.entity(), bytes, d.flags));
    FfiResult::OK
}

unsafe extern "C" fn mock_cmd_observe(
    cmds: *mut BamCommands,
    desc: *const BamObserverDesc,
    out_entity: *mut FfiEntity,
) -> FfiResult {
    let state = unsafe { &mut *cmds.cast::<MockSysCtxState>() };
    let host = unsafe { &mut *state.host };
    let d = unsafe { &*desc };
    let e = FfiEntity::from_bits(state.next_entity);
    state.next_entity += 1;
    host.observers.push(MockObserver {
        name: String::new(),
        event: d.event,
        target: d.entity(),
        flags: d.flags,
        callback: d.callback,
        mut_callback: d.mut_callback,
        user_data: d.user_data,
    });
    unsafe { *out_entity = e };
    FfiResult::OK
}

/// Scenario: Exercises fluent `EntityCommands`, scoped child spawning, entity disable/enable
/// toggles, and gizmo draw commands end-to-end against a mock host.
#[test]
fn entity_commands_child_spawner_disable_enable_and_gizmos_end_to_end() {
    let mut host = MockHost::new();
    let api = MockHost::api();
    let load_ctx = core::ptr::from_mut(host.as_mut()).cast::<BamLoadCtx>();
    let mut inst: *mut BamInstance = core::ptr::null_mut();

    let manifest = BamManifest::new(c"parity_mod".as_ptr(), c"0.3.0".as_ptr());
    let status = unsafe {
        init_guest_instance(&api, load_ctx, &mut inst, &manifest, |instance, lctx| {
            build_guest_fn(instance, lctx, |app| {
                app.system(c"entity_cmds_sys").build(|ctx| {
                    let cmds = ctx.commands();
                    // Invalid entity fails closed cleanly and preserves first error.
                    let mut invalid = cmds.entity(FfiEntity::NULL);
                    invalid.disable();
                    assert_eq!(invalid.finish(), Err(FfiResult::INVALID_ENTITY));

                    let mut parent = cmds.spawn_entity()?;
                    let parent_id = parent.id();
                    parent
                        .insert_component(&FfiCamera2d::default())
                        .disable()
                        .enable()
                        .with_children(|spawner| {
                            assert_eq!(spawner.parent(), parent_id);
                            let mut c1 = spawner.spawn_empty().unwrap();
                            c1.disable();
                            assert_eq!(c1.finish().unwrap(), FfiEntity::from_bits(2));
                        })
                        .trigger(&FfiPointerClick::default());
                    assert_eq!(parent.finish(), Ok(parent_id));

                    cmds.gizmo_line([0.0, 0.0, 0.0], [1.0, 2.0, 3.0], FfiColor::RED)?;
                    Ok(())
                })
            })
        })
    };
    assert_eq!(status, FfiResult::OK);
    assert_eq!(host.systems.len(), 1);

    let cb = host.systems[0].callback;
    let ud = host.systems[0].user_data;
    let mut sys_ctx = MockSysCtxState {
        host: core::ptr::from_mut(host.as_mut()),
        iters: Vec::new(),
        cmds: Vec::new(),
        next_entity: 1,
        entity_data: HashMap::new(),
        children: HashMap::new(),
    };
    let run_status = unsafe { cb(core::ptr::from_mut(&mut sys_ctx).cast::<BamSystemCtx>(), ud) };
    assert_eq!(run_status, FfiResult::OK);

    // Verify expected commands: Spawn parent(1), Insert Camera2d, InsertDefault Disabled (must use
    // `cmd_insert_default` because real `DisabledBridge` denies raw `cmd_insert`), Remove Disabled,
    // Spawn child(2) + ChildOf(1), InsertDefault Disabled on child(2), Trigger PointerClick on
    // parent(1), Trigger GizmoLine globally.
    assert!(
        sys_ctx
            .cmds
            .iter()
            .any(|c| matches!(c, MockCmd::SpawnEmpty(e) if *e == FfiEntity::from_bits(1)))
    );
    assert!(
        sys_ctx
            .cmds
            .iter()
            .any(|c| matches!(c, MockCmd::InsertDefault(e, _) if *e == FfiEntity::from_bits(1)))
    );
    assert!(
        sys_ctx
            .cmds
            .iter()
            .any(|c| matches!(c, MockCmd::InsertDefault(e, _) if *e == FfiEntity::from_bits(2)))
    );
    assert!(
        sys_ctx
            .cmds
            .iter()
            .any(|c| matches!(c, MockCmd::SpawnEmpty(e) if *e == FfiEntity::from_bits(2)))
    );
    assert!(
        sys_ctx
            .cmds
            .iter()
            .any(|c| matches!(c, MockCmd::Remove(e, _) if *e == FfiEntity::from_bits(1)))
    );
    assert!(
        sys_ctx
            .cmds
            .iter()
            .any(|c| matches!(c, MockCmd::Trigger(_, t, _, 0) if *t == FfiEntity::from_bits(1)))
    );
    assert!(
        sys_ctx
            .cmds
            .iter()
            .any(|c| matches!(c, MockCmd::Trigger(_, t, _, 0) if t.is_null()))
    );

    unsafe { shutdown_guest_instance(inst) };
}

/// Scenario: Exercises combinator run conditions, local system state, `RemovedComponents` streams,
/// 3-way disjoint queries, `single()`/`single_mut()` queries, and recursive descendant traversal.
#[test]
fn run_conditions_local_state_removed_queries_3_single_chunk_and_descendants() {
    let mut host = MockHost::new();
    let marker_sym: u32 = 123;
    host.symbols.insert(
        "bam.v0.custom.flag".to_owned(),
        core::ptr::from_ref(&marker_sym).cast::<c_void>(),
    );
    struct RawHostGuard(*mut MockHost);
    impl Drop for RawHostGuard {
        fn drop(&mut self) {
            unsafe {
                drop(Box::from_raw(self.0));
            }
        }
    }

    let host_guard = RawHostGuard(Box::into_raw(host));
    let host_ptr = host_guard.0;
    let api = MockHost::api();
    let load_ctx = host_ptr.cast::<BamLoadCtx>();
    let mut inst: *mut BamInstance = core::ptr::null_mut();

    static LOCAL_TICKS: AtomicU32 = AtomicU32::new(0);
    static REMOVED_SEEN: AtomicUsize = AtomicUsize::new(0);
    static DESCENDANT_COUNT: AtomicUsize = AtomicUsize::new(0);
    static GET_MANY_OK: AtomicBool = AtomicBool::new(false);

    let manifest = BamManifest::new(c"parity_mod".as_ptr(), c"0.3.0".as_ptr());
    let status = unsafe {
        init_guest_instance(&api, load_ctx, &mut inst, &manifest, |instance, lctx| {
            build_guest_fn(instance, lctx, |app| {
                // System 0: composable run conditions (`and`, `or`, `not`, `xor`, `every_n_ticks`, `symbol_exists`, `component_registered`)
                let s0 = app
                    .system(c"cond_sys")
                    .run_if(conditions::and(
                        conditions::symbol_exists(c"bam.v0.custom.flag"),
                        conditions::component_registered::<FfiDisabled>(),
                    ))
                    .run_if(conditions::every_n_ticks(2))
                    .build_with_local(0u32, |_ctx, local| {
                        *local += 1;
                        LOCAL_TICKS.store(*local, Ordering::SeqCst);
                    });
                assert_eq!(s0, FfiResult::OK);

                // System 1: run_once + xor + every_n_ticks(0)
                let s1 = app
                    .system(c"once_sys")
                    .run_if(conditions::and(
                        conditions::run_once(),
                        conditions::not(conditions::every_n_ticks(0)),
                    ))
                    .build(|_ctx| {});
                assert_eq!(s1, FfiResult::OK);

                // System 2: 3 queries + system-level `.removed::<FfiChildOf>()` + `single_chunk` + `get_many` + `iter_descendants`
                let s2 = app
                    .system(c"multi_sys")
                    .query_typed::<(&FfiChildOf,)>()
                    .query(|q| q.read::<FfiChildOf>().allow_disabled())
                    .query(|q| q.read_children())
                    .removed::<FfiChildOf>()
                    .build(|ctx| {
                        // Read system-level RemovedComponents<FfiChildOf>
                        let removed = ctx.read_removed::<FfiChildOf>(0)?;
                        REMOVED_SEEN.store(removed.len(), Ordering::SeqCst);

                        // get_many on query 0
                        let pair = ctx.get_many::<2, FfiChildOf>(
                            0,
                            0,
                            [FfiEntity::from_bits(10), FfiEntity::from_bits(11)],
                        );
                        if let Some([a, b]) = pair {
                            if a.parent == FfiEntity::from_bits(1)
                                && b.parent == FfiEntity::from_bits(2)
                            {
                                GET_MANY_OK.store(true, Ordering::SeqCst);
                            }
                        }
                        assert!(
                            ctx.get_many::<2, FfiChildOf>(
                                0,
                                0,
                                [FfiEntity::from_bits(10), FfiEntity::from_bits(999)],
                            )
                            .is_none()
                        );

                        // Cycle-safe iter_descendants on cyclic hierarchy 1 -> [2, 3], 2 -> [4, 1], 3 -> [4]
                        let desc = ctx.iter_descendants(FfiEntity::from_bits(1))?;
                        assert_eq!(
                            desc,
                            vec![
                                FfiEntity::from_bits(2),
                                FfiEntity::from_bits(3),
                                FfiEntity::from_bits(4),
                            ]
                        );
                        DESCENDANT_COUNT.store(desc.len(), Ordering::SeqCst);
                        assert_eq!(
                            ctx.iter_descendants(FfiEntity::NULL),
                            Err(FfiResult::INVALID_ENTITY)
                        );

                        // queries_3_mut rejects duplicates and out-of-range
                        assert!(ctx.queries_3_mut(0, 0, 1).is_none());
                        assert!(ctx.queries_3_mut(0, 1, 9).is_none());

                        // queries_3_mut succeeds in reversed permutation (2, 0, 1)
                        let (q2, q0, q1) = ctx.queries_3_mut(2, 0, 1).unwrap();
                        assert!(!q0.is_empty()?);
                        assert_eq!(q1.count()?, 1);
                        assert_eq!(q2.count()?, 0);
                        assert!(q2.is_empty()?);

                        // single_chunk: q0 has 2 entities -> MultipleEntities; q1 has 1 -> Ok; q2 has 0 -> NoEntities
                        assert_eq!(
                            q0.typed::<(&FfiChildOf,)>().single_chunk().unwrap_err(),
                            QuerySingleError::MultipleEntities
                        );
                        let mut q1_typed = q1.typed::<(&FfiChildOf,)>();
                        let (single_slice,) = q1_typed.single_chunk().unwrap();
                        assert_eq!(single_slice.len(), 1);
                        assert_eq!(single_slice[0].parent, FfiEntity::from_bits(77));
                        assert_eq!(
                            q2.typed::<FfiEntity>().single_chunk().unwrap_err(),
                            QuerySingleError::NoEntities
                        );
                        Ok(())
                    });
                assert_eq!(s2, FfiResult::OK);
                FfiResult::OK
            })
        })
    };
    assert_eq!(status, FfiResult::OK);

    // Evaluate run conditions on system 0 (every_n_ticks(2): T, F, T, F) and local state accumulation.
    let (s0_run_if, s0_ru, s0_cb, s0_ud, s1_run_if, s1_ru) = {
        let host_ref = unsafe { &*host_ptr };
        (
            host_ref.systems[0].run_if.unwrap(),
            host_ref.systems[0].run_if_user_data,
            host_ref.systems[0].callback,
            host_ref.systems[0].user_data,
            host_ref.systems[1].run_if.unwrap(),
            host_ref.systems[1].run_if_user_data,
        )
    };
    let mut dummy_ctx = MockSysCtxState {
        host: host_ptr,
        iters: Vec::new(),
        cmds: Vec::new(),
        next_entity: 1,
        entity_data: HashMap::new(),
        children: HashMap::new(),
    };
    let mut fires = Vec::new();
    for _ in 0..4 {
        let should = unsafe { s0_run_if(s0_ru) }.get();
        fires.push(should);
        if should {
            assert_eq!(
                unsafe { s0_cb(core::ptr::from_mut(&mut dummy_ctx).cast(), s0_ud) },
                FfiResult::OK
            );
        }
    }
    assert_eq!(fires, vec![true, false, true, false]);
    assert_eq!(LOCAL_TICKS.load(Ordering::SeqCst), 2);

    // System 1: run_once -> T, F, F
    assert!(unsafe { s1_run_if(s1_ru) }.get());
    assert!(!unsafe { s1_run_if(s1_ru) }.get());
    assert!(!unsafe { s1_run_if(s1_ru) }.get());

    // Execute System 2 with 3 user queries + 1 system resource query (index 3)
    let (child_of_h, q0_terms, q1_terms, q2_terms, q3_terms, s2_cb, s2_ud) = {
        let host_ref = unsafe { &*host_ptr };
        (
            host_ref.comps[0].handle,
            host_ref.systems[2].queries[0].clone(),
            host_ref.systems[2].queries[1].clone(),
            host_ref.systems[2].queries[2].clone(),
            host_ref.systems[2].queries[3].clone(),
            host_ref.systems[2].callback,
            host_ref.systems[2].user_data,
        )
    };
    let mut q0_cols = HashMap::new();
    q0_cols.insert(0, vec![1u64, 2u64]);
    let q0_iter = Box::new(MockIterState {
        host: host_ptr,
        terms: q0_terms,
        entities: vec![FfiEntity::from_bits(10), FfiEntity::from_bits(11)],
        columns: q0_cols,
        removed: HashMap::new(),
        resources: HashMap::new(),
        messages: HashMap::new(),
        chunks: core::iter::once(0..2).collect(),
        cursor: 0,
        active_chunk: None,
    });

    let mut q1_cols = HashMap::new();
    q1_cols.insert(0, vec![77u64]);
    let q1_iter = Box::new(MockIterState {
        host: host_ptr,
        terms: q1_terms,
        entities: vec![FfiEntity::from_bits(20)],
        columns: q1_cols,
        removed: HashMap::new(),
        resources: HashMap::new(),
        messages: HashMap::new(),
        chunks: core::iter::once(0..1).collect(),
        cursor: 0,
        active_chunk: None,
    });

    let q2_iter = Box::new(MockIterState {
        host: host_ptr,
        terms: q2_terms,
        entities: Vec::new(),
        columns: HashMap::new(),
        removed: HashMap::new(),
        resources: HashMap::new(),
        messages: HashMap::new(),
        chunks: Vec::new(),
        cursor: 0,
        active_chunk: None,
    });

    let mut res_removed = HashMap::new();
    res_removed.insert(
        0,
        vec![FfiEntity::from_bits(101), FfiEntity::from_bits(102)],
    );
    let res_iter = Box::new(MockIterState {
        host: host_ptr,
        terms: q3_terms,
        entities: Vec::new(),
        columns: HashMap::new(),
        removed: res_removed,
        resources: HashMap::new(),
        messages: HashMap::new(),
        chunks: Vec::new(),
        cursor: 0,
        active_chunk: None,
    });

    let mut entity_data = HashMap::new();
    entity_data.insert((0, 0, FfiEntity::from_bits(10)), vec![1u64]);
    entity_data.insert((0, 0, FfiEntity::from_bits(11)), vec![2u64]);

    // Cyclic children graph to prove cycle-safe BFS in iter_descendants
    let mut children = HashMap::new();
    children.insert(
        (BamComponent::INVALID, FfiEntity::from_bits(1)),
        vec![FfiEntity::from_bits(2), FfiEntity::from_bits(3)],
    );
    children.insert(
        (BamComponent::INVALID, FfiEntity::from_bits(2)),
        vec![FfiEntity::from_bits(4), FfiEntity::from_bits(1)],
    );
    children.insert(
        (BamComponent::INVALID, FfiEntity::from_bits(3)),
        vec![FfiEntity::from_bits(4)],
    );

    let mut s2_ctx = MockSysCtxState {
        host: host_ptr,
        iters: vec![q0_iter, q1_iter, q2_iter, res_iter],
        cmds: Vec::new(),
        next_entity: 100,
        entity_data,
        children,
    };
    assert_eq!(
        unsafe { s2_cb(core::ptr::from_mut(&mut s2_ctx).cast(), s2_ud) },
        FfiResult::OK
    );
    assert_eq!(REMOVED_SEEN.load(Ordering::SeqCst), 2);
    assert_eq!(DESCENDANT_COUNT.load(Ordering::SeqCst), 3);
    assert!(GET_MANY_OK.load(Ordering::SeqCst));
    let _ = child_of_h;

    unsafe { shutdown_guest_instance(inst) };
    drop(host_guard);
}

/// Scenario: Exercises `TypedObserverBuilder`, Bevy 0.20 picking event observers, and ABI 0.3.0
/// `GuestComponentOptions` flags/hooks/requirements end-to-end.
#[test]
fn typed_observer_builder_picking_events_and_abi_v03_component_options() {
    let mut host = MockHost::new();
    let child_of_h = host.comps[0].handle;
    let api = MockHost::api();
    let load_ctx = core::ptr::from_mut(host.as_mut()).cast::<BamLoadCtx>();
    let mut inst: *mut BamInstance = core::ptr::null_mut();

    static OBSERVED_CLICKS: AtomicU32 = AtomicU32::new(0);
    static FIELDS: [BamFieldDesc; 1] = [BamFieldDesc::new(
        BamFieldKind::F32,
        StrView::from_str("illuminance"),
        StrView::from_str("f32"),
        16,
        4,
        4,
    )];

    let manifest = BamManifest::new(c"picking_mod".as_ptr(), c"0.3.0".as_ptr());
    let status = unsafe {
        init_guest_instance(&api, load_ctx, &mut inst, &manifest, |instance, lctx| {
            build_guest_fn(instance, lctx, |app| {
                // Register a component with ABI 0.3 metadata & custom POD required bytes via GuestComponentOptions
                let default_parent = FfiChildOf::new(FfiEntity::from_bits(55));
                let opts = GuestComponentOptions::<FfiDirectionalLight>::new()
                    .summary_tick()
                    .with_schema_fingerprint(0xDEAD_BEEF_CAFE_BABE)
                    .with_fields(&FIELDS)
                    .require_handle_with_value(child_of_h, &default_parent);
                if let Err(err) = app.register_component_with_options(opts) {
                    return err;
                }

                // Register Bevy 0.20 PointerClick mutable/propagating observer with run_if
                if let Err(err) = app
                    .observe_pointer_click("click_handler")
                    .with_target(FfiEntity::from_bits(9))
                    .with_propagate(true)
                    .run_if(conditions::run_once())
                    .build_mut(|_tgt, ev, propagate, _cmds| {
                        OBSERVED_CLICKS.fetch_add(1, Ordering::SeqCst);
                        ev.duration_secs = 0.25;
                        *propagate = false;
                    })
                {
                    return err;
                }

                // Register Bevy 0.20 PointerDrag observer as well
                if let Err(err) = app
                    .observe_pointer_drag("drag_handler")
                    .build(|_tgt, _ev, _cmds| {})
                {
                    return err;
                }

                FfiResult::OK
            })
        })
    };
    assert_eq!(status, FfiResult::OK);

    // Verify ABI 0.3 component metadata reached host ComponentDesc
    let light_comp = host
        .comps
        .iter()
        .find(|c| c.name == "bamabi::camera_light::FfiDirectionalLight")
        .expect("FfiDirectionalLight registered");
    assert_eq!(light_comp.schema_fingerprint, 0xDEAD_BEEF_CAFE_BABE);
    assert_eq!(light_comp.field_count, 1);
    assert_eq!(light_comp.required_init_count, 1);
    assert_eq!(light_comp.required_custom_bytes.len(), 1);
    assert_eq!(
        light_comp.required_custom_bytes[0],
        55u64.to_ne_bytes().to_vec()
    );

    // Invoke registered PointerClick observer twice: run_once lets 1st run and mutate/stop propagation, skips 2nd.
    assert_eq!(host.observers.len(), 2);
    assert_eq!(host.observers[0].target, FfiEntity::from_bits(9));
    assert_ne!(host.observers[0].flags & BAM_OBSERVER_FLAG_PROPAGATE, 0);
    let mut_cb = host.observers[0].mut_callback.unwrap();
    let ud = host.observers[0].user_data;

    let mut sys_ctx = MockSysCtxState {
        host: core::ptr::from_mut(host.as_mut()),
        iters: Vec::new(),
        cmds: Vec::new(),
        next_entity: 1,
        entity_data: HashMap::new(),
        children: HashMap::new(),
    };
    let mut click = FfiPointerClick {
        duration_secs: 0.1,
        ..Default::default()
    };
    let mut prop = FfiBool::TRUE;
    assert_eq!(
        unsafe {
            mut_cb(
                FfiEntity::from_bits(9),
                core::ptr::from_mut(&mut click).cast::<u8>(),
                &mut prop,
                core::ptr::from_mut(&mut sys_ctx).cast::<BamCommands>(),
                ud,
            )
        },
        FfiResult::OK
    );
    assert_eq!(OBSERVED_CLICKS.load(Ordering::SeqCst), 1);
    assert_eq!(click.duration_secs, 0.25);
    assert!(!prop.get());

    // Second invocation skipped by run_once, leaving propagate untouched (`TRUE`).
    let mut prop2 = FfiBool::TRUE;
    assert_eq!(
        unsafe {
            mut_cb(
                FfiEntity::from_bits(9),
                core::ptr::from_mut(&mut click).cast::<u8>(),
                &mut prop2,
                core::ptr::from_mut(&mut sys_ctx).cast::<BamCommands>(),
                ud,
            )
        },
        FfiResult::OK
    );
    assert_eq!(OBSERVED_CLICKS.load(Ordering::SeqCst), 1);
    assert!(prop2.get());

    unsafe { shutdown_guest_instance(inst) };
}

unsafe extern "C" fn mock_opt_u32_get(src: *const c_void, out: *mut FfiOption<u32>) -> FfiResult {
    let val = unsafe { *src.cast::<FfiOption<u32>>() };
    unsafe { *out = val };
    FfiResult::OK
}

unsafe extern "C" fn mock_u32_get(src: *const c_void, out: *mut u32) -> FfiResult {
    unsafe { *out = *src.cast::<u32>() };
    FfiResult::OK
}

unsafe extern "C" fn mock_u32_set(dst: *mut c_void, val: *const u32) -> FfiResult {
    unsafe { *dst.cast::<u32>() = *val };
    FfiResult::OK
}

/// Scenario: Exercises `GuestStateBridge::current_optional`, `ModStateSlab` (`ModStateEntry`,
/// `ModStateTransitionRequest`, `ModDespawnOnExit`, `ModDespawnOnEnter`), POD message streams, and
/// runtime deferred lifecycle observers against mock host resources and VTables.
#[test]
fn state_bridge_mod_state_slab_pod_messages_and_runtime_lifecycle_observers() {
    let mut host = MockHost::new();

    // Register primary State#0, ModNextState#0, real ModStateSlab components/messages, and AssetEvent
    let state_uuid = ComponentUuid::from_canonical_name("bevy_state::State#0");
    let next_state_uuid = ComponentUuid::from_canonical_name("bamabi::ModNextState#0");

    host.add_comp(
        "bevy_state::State#0",
        state_uuid,
        core::mem::size_of::<u32>(),
        core::mem::align_of::<u32>(),
        BAM_STORAGE_FLAG_RESOURCE,
    );
    host.add_comp(
        "bamabi::ModNextState#0",
        next_state_uuid,
        core::mem::size_of::<u32>(),
        core::mem::align_of::<u32>(),
        BAM_STORAGE_FLAG_RESOURCE,
    );
    host.add_comp(
        "bevy_state::ModStateEntry",
        <ModStateEntry as GuestComponent>::UUID,
        core::mem::size_of::<ModStateEntry>(),
        core::mem::align_of::<ModStateEntry>(),
        0,
    );
    host.add_comp(
        "bevy_state::ModStateTransitionRequest",
        <ModStateTransitionRequest as GuestComponent>::UUID,
        core::mem::size_of::<ModStateTransitionRequest>(),
        core::mem::align_of::<ModStateTransitionRequest>(),
        BAM_STORAGE_FLAG_MESSAGE,
    );
    host.add_comp(
        "bevy_state::ModDespawnOnExit",
        <ModDespawnOnExit as GuestComponent>::UUID,
        core::mem::size_of::<ModDespawnOnExit>(),
        core::mem::align_of::<ModDespawnOnExit>(),
        0,
    );
    host.add_comp(
        "bevy_state::ModDespawnOnEnter",
        <ModDespawnOnEnter as GuestComponent>::UUID,
        core::mem::size_of::<ModDespawnOnEnter>(),
        core::mem::align_of::<ModDespawnOnEnter>(),
        0,
    );
    host.add_comp(
        "bevy_asset::AssetEvent",
        <FfiAssetEvent as GuestComponent>::UUID,
        core::mem::size_of::<FfiAssetEvent>(),
        core::mem::align_of::<FfiAssetEvent>(),
        BAM_STORAGE_FLAG_MESSAGE,
    );

    let state_vt = MappedWireVTable::<u32> {
        header: VTableHeader::new::<MappedWireVTable<u32>, u32>(state_uuid),
        get: Some(mock_u32_get),
        set: None,
    };
    let next_state_vt = MappedWireVTable::<u32> {
        header: VTableHeader::new::<MappedWireVTable<u32>, u32>(next_state_uuid),
        get: Some(mock_u32_get),
        set: Some(mock_u32_set),
    };
    host.symbols.insert(
        "bam.v0.bevy_state::State#0.vtable".to_owned(),
        core::ptr::from_ref(&state_vt).cast::<c_void>(),
    );
    host.symbols.insert(
        "bam.v0.bamabi::ModNextState#0.vtable".to_owned(),
        core::ptr::from_ref(&next_state_vt).cast::<c_void>(),
    );

    let api = MockHost::api();
    let load_ctx = core::ptr::from_mut(host.as_mut()).cast::<BamLoadCtx>();
    let mut inst: *mut BamInstance = core::ptr::null_mut();

    static STATE_ACTIVE_OK: AtomicBool = AtomicBool::new(false);
    static STATE_ABSENT_OK: AtomicBool = AtomicBool::new(false);
    static SLAB_OK: AtomicBool = AtomicBool::new(false);
    static MSGS_ROUNDTRIP: AtomicU32 = AtomicU32::new(0);

    let sub_handle = ModStateHandle::new(2, 1);
    let comp_handle = ModStateHandle::new(3, 1);

    let manifest = BamManifest::new(c"state_msg_mod".as_ptr(), c"0.3.0".as_ptr());
    let status = unsafe {
        init_guest_instance(&api, load_ctx, &mut inst, &manifest, |instance, lctx| {
            build_guest_fn(instance, lctx, |app| {
                let state = app.state_bridge().unwrap();

                app.system(c"state_and_msg_sys")
                    .query(|q| {
                        q.resource_read_handle(state.state_handle())
                            .resource_write_handle(state.next_state_handle())
                    })
                    .query(|q| q.read_state_entries())
                    .write_state_transitions()
                    .message_write::<FfiAssetEvent>()
                    .build(move |ctx| {
                        // First run checks Some(3) + writes next state 7; second run checks absent -> Ok(None)
                        match state.current_optional(ctx, 0)? {
                            Some(3) => {
                                assert!(state.is_in_state(ctx, 0, 3)?);
                                assert_eq!(state.set_state_if_different(ctx, 1, 7), FfiResult::OK);
                                STATE_ACTIVE_OK.store(true, Ordering::SeqCst);
                            }
                            None => {
                                STATE_ABSENT_OK.store(true, Ordering::SeqCst);
                            }
                            other => panic!("unexpected state {other:?}"),
                        }

                        // Query 1 reads ModStateSlab entries (`SubState` active + `ComputedState` absent)
                        let mut sub_disc = None;
                        let mut comp_disc = Some(999);
                        let q1 = ctx.query_mut(1).unwrap();
                        while let Some(chunk) = q1.next_chunk_view() {
                            for entry in chunk.component::<ModStateEntry>(0).unwrap() {
                                if entry.state_handle() == sub_handle {
                                    sub_disc = entry.active_discriminant();
                                } else if entry.state_handle() == comp_handle {
                                    comp_disc = entry.active_discriminant();
                                }
                            }
                        }
                        assert_eq!(sub_disc, Some(5));
                        assert_eq!(comp_disc, None);
                        // System-level message writer at term 0 of the resource query sends a slab transition
                        assert_eq!(
                            ctx.request_state_transition(0, sub_handle, 6),
                            FfiResult::OK
                        );
                        SLAB_OK.store(true, Ordering::SeqCst);

                        // Batch send + read back POD messages on term 1 of the system resource query
                        let batch = [
                            FfiAssetEvent {
                                handle: FfiAssetHandle::from_index(BamComponent::from_raw(1), 1, 0),
                                kind: FfiAssetEventKind::ADDED,
                                reserved0: [0; 3],
                            },
                            FfiAssetEvent {
                                handle: FfiAssetHandle::from_index(BamComponent::from_raw(1), 2, 0),
                                kind: FfiAssetEventKind::LOADED_WITH_DEPENDENCIES,
                                reserved0: [0; 3],
                            },
                        ];
                        ctx.send_pod_messages(1, &batch)?;
                        let got: Vec<FfiAssetEvent> = ctx.read_messages(1)?;
                        assert_eq!(got.len(), 2);
                        assert_eq!(got[1].kind, FfiAssetEventKind::LOADED_WITH_DEPENDENCIES);
                        MSGS_ROUNDTRIP.fetch_add(got.len() as u32, Ordering::SeqCst);

                        // State-scoped slab despawn markers + runtime lifecycle observers via GuestCommands
                        let cmds = ctx.commands();
                        let e = FfiEntity::from_bits(42);
                        let mut ec = cmds.entity(e);
                        ec.despawn_on_exit_slab(sub_handle, 5)
                            .despawn_on_enter_slab(sub_handle, 6);
                        assert_eq!(ec.finish(), Ok(e));
                        let _ = cmds.observe_add::<FfiChildOf, _>(e, |_t, _c| {})?;
                        let _ = cmds.observe_insert::<FfiChildOf, _>(e, |_t, _c| {})?;
                        let _ = cmds.observe_discard::<FfiChildOf, _>(e, |_t, _c| {})?;
                        let _ = cmds.observe_remove::<FfiChildOf, _>(e, |_t, _c| {})?;
                        let _ = cmds.observe_despawn::<FfiChildOf, _>(e, |_t, _c| {})?;
                        let _ = cmds.observe_entity_mut_with::<FfiPointerClick, _>(
                            e,
                            true,
                            BamComponent::INVALID,
                            |_t, _ev, _p, _c| {},
                        )?;
                        Ok(())
                    })
            })
        })
    };
    assert_eq!(status, FfiResult::OK);

    // Prepare ModStateSlab entries for query 1: sub_handle = active(5), comp_handle = absent
    let slab_entries = [
        ModStateEntry {
            state_slot: sub_handle.slot_index,
            state_generation: sub_handle.generation,
            current_discriminant: 5,
            flags: MOD_STATE_FLAG_ACTIVE | MOD_STATE_FLAG_MUTABLE | MOD_STATE_FLAG_SUB_STATE,
        },
        ModStateEntry {
            state_slot: comp_handle.slot_index,
            state_generation: comp_handle.generation,
            current_discriminant: STATE_ABSENT_DISCRIMINANT,
            flags: MOD_STATE_FLAG_COMPUTED,
        },
    ];
    let slab_bytes = unsafe {
        core::slice::from_raw_parts(
            slab_entries.as_ptr().cast::<u8>(),
            core::mem::size_of_val(&slab_entries),
        )
    };
    let make_slab_iter = |host_ptr: *mut MockHost, terms: Vec<BamTermDesc>| {
        let mut cols = HashMap::new();
        cols.insert(0, aligned_buf_from_bytes(slab_bytes));
        Box::new(MockIterState {
            host: host_ptr,
            terms,
            entities: vec![FfiEntity::from_bits(200), FfiEntity::from_bits(201)],
            columns: cols,
            removed: HashMap::new(),
            resources: HashMap::new(),
            messages: HashMap::new(),
            chunks: core::iter::once(0..2).collect(),
            cursor: 0,
            active_chunk: None,
        })
    };

    // Run 1: State#0 = 3, ModNextState#0 = 0
    let mut resources = HashMap::new();
    resources.insert(0, aligned_buf_from_bytes(&3u32.to_ne_bytes()));
    resources.insert(1, aligned_buf_from_bytes(&0u32.to_ne_bytes()));

    let q0 = host.systems[0].queries[0].clone();
    let q1 = host.systems[0].queries[1].clone();
    let q2 = host.systems[0].queries[2].clone();
    let cb = host.systems[0].callback;
    let ud = host.systems[0].user_data;

    let host_ptr = core::ptr::from_mut(host.as_mut());
    let iter0 = Box::new(MockIterState {
        host: host_ptr,
        terms: q0.clone(),
        entities: Vec::new(),
        columns: HashMap::new(),
        removed: HashMap::new(),
        resources,
        messages: HashMap::new(),
        chunks: Vec::new(),
        cursor: 0,
        active_chunk: None,
    });
    let iter1 = make_slab_iter(host_ptr, q1.clone());
    let res_iter = Box::new(MockIterState {
        host: host_ptr,
        terms: q2.clone(),
        entities: Vec::new(),
        columns: HashMap::new(),
        removed: HashMap::new(),
        resources: HashMap::new(),
        messages: HashMap::new(),
        chunks: Vec::new(),
        cursor: 0,
        active_chunk: None,
    });
    let mut sys_ctx1 = MockSysCtxState {
        host: host_ptr,
        iters: vec![iter0, iter1, res_iter],
        cmds: Vec::new(),
        next_entity: 50,
        entity_data: HashMap::new(),
        children: HashMap::new(),
    };
    assert_eq!(
        unsafe { cb(core::ptr::from_mut(&mut sys_ctx1).cast(), ud) },
        FfiResult::OK
    );
    assert!(STATE_ACTIVE_OK.load(Ordering::SeqCst));
    assert!(SLAB_OK.load(Ordering::SeqCst));
    let next_state_val =
        unsafe { core::ptr::read(sys_ctx1.iters[0].resources[&1].as_ptr().cast::<u32>()) };
    assert_eq!(next_state_val, 7);
    assert_eq!(sys_ctx1.iters[2].messages[&0].len(), 1);
    assert_eq!(host.observers.len(), 6);
    assert_ne!(host.observers[5].flags & BAM_OBSERVER_FLAG_PROPAGATE, 0);

    // Run 2: State#0 ABSENT in World (NOT_FOUND) -> `current_optional` yields Ok(None)!
    // Re-derive `host_ptr` after reading `host.observers` above so Tree Borrows gives
    // Run 2 a fresh mutable provenance instead of reusing the frozen Run 1 pointer.
    let host_ptr = core::ptr::from_mut(host.as_mut());
    let iter0_absent = Box::new(MockIterState {
        host: host_ptr,
        terms: q0,
        entities: Vec::new(),
        columns: HashMap::new(),
        removed: HashMap::new(),
        resources: HashMap::new(),
        messages: HashMap::new(),
        chunks: Vec::new(),
        cursor: 0,
        active_chunk: None,
    });
    let iter1_2 = make_slab_iter(host_ptr, q1);
    let res_iter_2 = Box::new(MockIterState {
        host: host_ptr,
        terms: q2,
        entities: Vec::new(),
        columns: HashMap::new(),
        removed: HashMap::new(),
        resources: HashMap::new(),
        messages: HashMap::new(),
        chunks: Vec::new(),
        cursor: 0,
        active_chunk: None,
    });
    let mut sys_ctx2 = MockSysCtxState {
        host: host_ptr,
        iters: vec![iter0_absent, iter1_2, res_iter_2],
        cmds: Vec::new(),
        next_entity: 70,
        entity_data: HashMap::new(),
        children: HashMap::new(),
    };
    assert_eq!(
        unsafe { cb(core::ptr::from_mut(&mut sys_ctx2).cast(), ud) },
        FfiResult::OK
    );
    assert!(STATE_ABSENT_OK.load(Ordering::SeqCst));
    assert_eq!(MSGS_ROUNDTRIP.load(Ordering::SeqCst), 4);

    unsafe { shutdown_guest_instance(inst) };
}

/// Scenario: Exercises `#[derive(ModComponent)]` on a non-POD `String`-bearing component across
/// dynamic entity access and wire-encoded non-POD message streams end-to-end.
#[cfg(feature = "derive")]
#[test]
fn derive_non_pod_dynamic_entity_access_and_non_pod_messages_end_to_end() {
    #[derive(Clone, Debug, Default, PartialEq, bamabi_guest::ModComponent)]
    #[mod_component(name = "parity::DynNote")]
    struct DynNote {
        text: String,
        score: i32,
    }

    #[derive(Clone, Debug, Default, PartialEq, bamabi_guest::ModComponent)]
    #[mod_component(name = "parity::DynChatMsg")]
    struct DynChatMsg {
        body: String,
        channel: u32,
    }

    let mut host = MockHost::new();
    let non_pod_marker: u32 = 1;
    let export_ptr = DynChatMsg::resolve_symbol("bam.v0.parity::DynChatMsg.export_fields")
        .expect("export_fields sym");
    host.symbols.insert(
        "bam.v0.parity::DynChatMsg.non_pod_message".to_owned(),
        core::ptr::from_ref(&non_pod_marker).cast::<c_void>(),
    );
    host.symbols.insert(
        "bam.v0.parity::DynChatMsg.export_fields".to_owned(),
        export_ptr,
    );
    host.add_comp(
        "parity::DynChatMsg",
        DynChatMsg::UUID,
        core::mem::size_of::<*const c_void>(),
        core::mem::align_of::<*const c_void>(),
        BAM_STORAGE_FLAG_MESSAGE,
    );

    let api = MockHost::api();
    let load_ctx = core::ptr::from_mut(host.as_mut()).cast::<BamLoadCtx>();
    let mut inst: *mut BamInstance = core::ptr::null_mut();

    static DYN_MUTATED: AtomicBool = AtomicBool::new(false);
    static DYN_MSG_READ: AtomicBool = AtomicBool::new(false);

    let manifest = BamManifest::new(c"dyn_mod".as_ptr(), c"0.3.0".as_ptr());
    let status = unsafe {
        init_guest_instance(&api, load_ctx, &mut inst, &manifest, |instance, lctx| {
            build_guest_fn(instance, lctx, |app| {
                let h = match app.register_dynamic_component::<DynNote>() {
                    Ok(h) => h,
                    Err(e) => return e,
                };
                assert!(h.is_valid());
                app.system(c"dyn_sys")
                    .query(|q| q.write_dynamic::<DynNote>())
                    .message_write_dynamic::<DynChatMsg>()
                    .build(|ctx| {
                        let e = FfiEntity::from_bits(7);
                        {
                            let note = ctx.get_dynamic::<DynNote>(0, 0, e).expect("note present");
                            assert_eq!(note.text, "hello");
                            assert_eq!(note.score, 10);
                        }
                        {
                            let note_mut =
                                ctx.get_dynamic_mut::<DynNote>(0, 0, e).expect("note mut");
                            note_mut.text.push_str(" world");
                            note_mut.score = 42;
                            DYN_MUTATED.store(true, Ordering::SeqCst);
                        }

                        // Read host non-POD message reconstructed via export_fields -> import_field
                        let msgs: Vec<DynChatMsg> = ctx.read_non_pod_messages(0)?;
                        assert_eq!(msgs.len(), 1);
                        assert_eq!(msgs[0].body, "from_host");
                        assert_eq!(msgs[0].channel, 99);
                        DYN_MSG_READ.store(true, Ordering::SeqCst);

                        // Send guest non-POD message encoded as a length-prefixed witness packet
                        let outgoing = DynChatMsg {
                            body: "from_guest".to_owned(),
                            channel: 77,
                        };
                        let send_st = ctx.send_non_pod_message(0, &outgoing);
                        assert_eq!(send_st, FfiResult::OK);
                        Ok(())
                    })
            })
        })
    };
    assert_eq!(status, FfiResult::OK);

    // 8-byte-aligned live DynNote storage for entity 7 (moved in and reclaimed below)
    let words = core::mem::size_of::<DynNote>().div_ceil(8);
    let mut live_words = vec![0u64; words];
    let live_note = DynNote {
        text: "hello".to_owned(),
        score: 10,
    };
    unsafe {
        core::ptr::write(live_words.as_mut_ptr().cast::<DynNote>(), live_note);
    }

    // Host-side live DynChatMsg queued for `read_non_pod_messages`
    let host_msg = DynChatMsg {
        body: "from_host".to_owned(),
        channel: 99,
    };
    let host_msg_ptr = core::ptr::from_ref(&host_msg).cast::<c_void>();
    let mut incoming_msgs = HashMap::new();
    incoming_msgs.insert(0, vec![MockMsgItem::Ptr(host_msg_ptr)]);

    let q0_terms = host.systems[0].queries[0].clone();
    let q1_terms = host.systems[0].queries[1].clone();
    let cb = host.systems[0].callback;
    let ud = host.systems[0].user_data;
    let host_ptr = core::ptr::from_mut(host.as_mut());

    let q0_iter = Box::new(MockIterState {
        host: host_ptr,
        terms: q0_terms,
        entities: Vec::new(),
        columns: HashMap::new(),
        removed: HashMap::new(),
        resources: HashMap::new(),
        messages: HashMap::new(),
        chunks: Vec::new(),
        cursor: 0,
        active_chunk: None,
    });
    let res_iter = Box::new(MockIterState {
        host: host_ptr,
        terms: q1_terms,
        entities: Vec::new(),
        columns: HashMap::new(),
        removed: HashMap::new(),
        resources: HashMap::new(),
        messages: incoming_msgs,
        chunks: Vec::new(),
        cursor: 0,
        active_chunk: None,
    });
    let mut entity_data = HashMap::new();
    entity_data.insert((0, 0, FfiEntity::from_bits(7)), live_words);

    let mut sys_ctx = MockSysCtxState {
        host: host_ptr,
        iters: vec![q0_iter, res_iter],
        cmds: Vec::new(),
        next_entity: 1,
        entity_data,
        children: HashMap::new(),
    };
    assert_eq!(
        unsafe { cb(core::ptr::from_mut(&mut sys_ctx).cast(), ud) },
        FfiResult::OK
    );
    assert!(DYN_MUTATED.load(Ordering::SeqCst));
    assert!(DYN_MSG_READ.load(Ordering::SeqCst));

    // Verify send_non_pod_message pushed a non-empty witness packet with field_count == 2
    let sent_packets = &sys_ctx.iters[1].messages[&0];
    assert_eq!(sent_packets.len(), 1);
    let MockMsgItem::Bytes(packet) = &sent_packets[0] else {
        panic!("expected encoded witness byte packet");
    };
    assert_eq!(u32::from_le_bytes(packet[..4].try_into().unwrap()), 2);

    // Reclaim the live_words back into DynNote to verify mutation AND drop without leak!
    let final_words = sys_ctx
        .entity_data
        .remove(&(0, 0, FfiEntity::from_bits(7)))
        .unwrap();
    let final_note: DynNote = unsafe { core::ptr::read(final_words.as_ptr().cast::<DynNote>()) };
    assert_eq!(final_note.text, "hello world");
    assert_eq!(final_note.score, 42);

    unsafe { shutdown_guest_instance(inst) };
}

// ============================================================================
// Targeted Regression Tests for Adversarial Review Fixes
// ============================================================================

/// Regression: On ABI `0.2.0` hosts where `iter_count` is `None` (`Err(UNSUPPORTED)`),
/// `TypedChunkIter::single_chunk` MUST scan across all chunks rather than stopping at the first
/// 1-entity chunk — otherwise two 1-entity chunks silently returned `Ok(chunk_0)` instead of
/// `Err(QuerySingleError::MultipleEntities)`.
#[test]
fn typed_chunk_iter_single_chunk_v02_fallback_detects_multiple_1_entity_chunks() {
    let mut host = MockHost::new();
    let mut api = MockHost::api();
    api.iter_count = None; // Simulate an ABI 0.2.0 host lacking `iter_count`!
    let load_ctx = core::ptr::from_mut(host.as_mut()).cast::<BamLoadCtx>();
    let mut inst: *mut BamInstance = core::ptr::null_mut();

    static RAN: AtomicBool = AtomicBool::new(false);

    let manifest = BamManifest::new(c"v02_single_mod".as_ptr(), c"0.3.0".as_ptr());
    let status = unsafe {
        init_guest_instance(&api, load_ctx, &mut inst, &manifest, |instance, lctx| {
            build_guest_fn(instance, lctx, |app| {
                app.system(c"v02_single_sys")
                    .query_typed::<(&FfiChildOf,)>() // q0: two 1-entity chunks -> MultipleEntities!
                    .query_typed::<(&FfiChildOf,)>() // q1: one empty chunk + one 1-entity chunk -> Ok!
                    .query_typed::<(&FfiChildOf,)>() // q2: zero entities -> NoEntities!
                    .build(|ctx| {
                        let (q0, q1, q2) = ctx.queries_3_mut(0, 1, 2).unwrap();
                        assert_eq!(
                            q0.typed::<(&FfiChildOf,)>().single_chunk().unwrap_err(),
                            QuerySingleError::MultipleEntities
                        );
                        let mut q1_typed = q1.typed::<(&FfiChildOf,)>();
                        let (parents,) = q1_typed.single_chunk().expect("exactly 1 entity total");
                        assert_eq!(parents.len(), 1);
                        assert_eq!(parents[0].parent, FfiEntity::from_bits(88));
                        assert_eq!(
                            q2.typed::<(&FfiChildOf,)>().single_chunk().unwrap_err(),
                            QuerySingleError::NoEntities
                        );
                        RAN.store(true, Ordering::SeqCst);
                        Ok(())
                    })
            })
        })
    };
    assert_eq!(status, FfiResult::OK);

    let q0_terms = host.systems[0].queries[0].clone();
    let q1_terms = host.systems[0].queries[1].clone();
    let q2_terms = host.systems[0].queries[2].clone();
    let cb = host.systems[0].callback;
    let ud = host.systems[0].user_data;
    let host_ptr = core::ptr::from_mut(host.as_mut());
    // q0: two separate chunks [0..1] and [1..2] (1 entity each, 2 total)
    let mut q0_cols = HashMap::new();
    q0_cols.insert(0, vec![10u64, 20u64]);
    let q0 = Box::new(MockIterState {
        host: host_ptr,
        terms: q0_terms,
        entities: vec![FfiEntity::from_bits(1), FfiEntity::from_bits(2)],
        columns: q0_cols,
        removed: HashMap::new(),
        resources: HashMap::new(),
        messages: HashMap::new(),
        chunks: vec![0..1, 1..2],
        cursor: 0,
        active_chunk: None,
    });
    // q1: an empty chunk [0..0] followed by a 1-entity chunk [0..1]
    let mut q1_cols = HashMap::new();
    q1_cols.insert(0, vec![88u64]);
    let q1 = Box::new(MockIterState {
        host: host_ptr,
        terms: q1_terms,
        entities: vec![FfiEntity::from_bits(3)],
        columns: q1_cols,
        removed: HashMap::new(),
        resources: HashMap::new(),
        messages: HashMap::new(),
        chunks: vec![0..0, 0..1],
        cursor: 0,
        active_chunk: None,
    });
    // q2: zero chunks
    let q2 = Box::new(MockIterState {
        host: host_ptr,
        terms: q2_terms,
        entities: Vec::new(),
        columns: HashMap::new(),
        removed: HashMap::new(),
        resources: HashMap::new(),
        messages: HashMap::new(),
        chunks: Vec::new(),
        cursor: 0,
        active_chunk: None,
    });

    let mut sys_ctx = MockSysCtxState {
        host: host_ptr,
        iters: vec![q0, q1, q2],
        cmds: Vec::new(),
        next_entity: 10,
        entity_data: HashMap::new(),
        children: HashMap::new(),
    };
    assert_eq!(
        unsafe { cb(core::ptr::from_mut(&mut sys_ctx).cast(), ud) },
        FfiResult::OK
    );
    assert!(RAN.load(Ordering::SeqCst));

    unsafe { shutdown_guest_instance(inst) };
}

/// Regression: When a system has `user_query_count == 0` and a system-level resource query whose
/// term `0` is NOT a `REMOVED` stream (e.g. a resource read), `SystemContext::read_removed` MUST
/// return `Err(FfiResult::TYPE_MISMATCH)` rather than falling back to query `0` and masking the
/// error as `OUT_OF_RANGE` (or double-calling the same query).
#[test]
fn read_removed_preserves_type_mismatch_when_zero_user_queries() {
    let mut host = MockHost::new();
    let api = MockHost::api();
    let load_ctx = core::ptr::from_mut(host.as_mut()).cast::<BamLoadCtx>();
    let mut inst: *mut BamInstance = core::ptr::null_mut();

    static CHECKED: AtomicBool = AtomicBool::new(false);

    let manifest = BamManifest::new(c"removed_err_mod".as_ptr(), c"0.3.0".as_ptr());
    let status = unsafe {
        init_guest_instance(&api, load_ctx, &mut inst, &manifest, |instance, lctx| {
            build_guest_fn(instance, lctx, |app| {
                // Zero user queries + 1 system-level resource read (`FfiCamera2d`)
                app.system(c"res_only_sys")
                    .resource_read::<FfiCamera2d>()
                    .build(|ctx| {
                        let err = ctx.read_removed::<FfiChildOf>(0).unwrap_err();
                        assert_eq!(err, FfiResult::TYPE_MISMATCH);
                        let mut buf = [FfiEntity::NULL; 4];
                        let err_batch = ctx
                            .read_removed_batch::<FfiChildOf>(0, &mut buf)
                            .unwrap_err();
                        assert_eq!(err_batch, FfiResult::TYPE_MISMATCH);
                        CHECKED.store(true, Ordering::SeqCst);
                        Ok(())
                    })
            })
        })
    };
    assert_eq!(status, FfiResult::OK);

    let q0_terms = host.systems[0].queries[0].clone();
    let cb = host.systems[0].callback;
    let ud = host.systems[0].user_data;
    let host_ptr = core::ptr::from_mut(host.as_mut());
    let res_iter = Box::new(MockIterState {
        host: host_ptr,
        terms: q0_terms,
        entities: Vec::new(),
        columns: HashMap::new(),
        removed: HashMap::new(),
        resources: HashMap::new(),
        messages: HashMap::new(),
        chunks: Vec::new(),
        cursor: 0,
        active_chunk: None,
    });
    let mut sys_ctx = MockSysCtxState {
        host: host_ptr,
        iters: vec![res_iter],
        cmds: Vec::new(),
        next_entity: 1,
        entity_data: HashMap::new(),
        children: HashMap::new(),
    };
    assert_eq!(
        unsafe { cb(core::ptr::from_mut(&mut sys_ctx).cast(), ud) },
        FfiResult::OK
    );
    assert!(CHECKED.load(Ordering::SeqCst));

    unsafe { shutdown_guest_instance(inst) };
}

/// Regression: Exercises `ModUiBridgePlugin`'s real `MappedStringVTable` bridges (`Text`,
/// `TextSpan`) and `MappedWireVTable` bridges (`BackgroundColor`, `BorderColor`, `RemSize`,
/// `EmSize`, `ThemeContext`) plus `StdTypesBridgePlugin`'s `Disabled` bridge (`GuestDisabledBridge`)
/// and `GuestAppUiExt::{observe_value_change, observe_activate}` end-to-end.
#[test]
fn ui_mapped_string_and_wire_bridges_and_disabled_bridge_roundtrip() {
    use bamabi_guest::raw::MappedStringVTable;

    unsafe extern "C" fn mock_string_get(ptr: *const c_void) -> StrView {
        let s = unsafe { &*ptr.cast::<String>() };
        StrView::from_str(s.as_str())
    }
    unsafe extern "C" fn mock_string_set(ptr: *mut c_void, view: StrView) -> FfiResult {
        let s = unsafe { &mut *ptr.cast::<String>() };
        let Ok(slice) = (unsafe { view.try_as_str() }) else {
            return FfiResult::INVALID_UTF8;
        };
        s.clear();
        s.push_str(slice);
        FfiResult::OK
    }
    unsafe extern "C" fn mock_rgba_get(src: *const c_void, out: *mut FfiRgba) -> FfiResult {
        unsafe { *out = *src.cast::<FfiRgba>() };
        FfiResult::OK
    }
    unsafe extern "C" fn mock_rgba_set(dst: *mut c_void, val: *const FfiRgba) -> FfiResult {
        unsafe { *dst.cast::<FfiRgba>() = *val };
        FfiResult::OK
    }
    unsafe extern "C" fn mock_rem_get(src: *const c_void, out: *mut FfiRemSize) -> FfiResult {
        unsafe { *out = *src.cast::<FfiRemSize>() };
        FfiResult::OK
    }
    unsafe extern "C" fn mock_rem_set(dst: *mut c_void, val: *const FfiRemSize) -> FfiResult {
        unsafe { *dst.cast::<FfiRemSize>() = *val };
        FfiResult::OK
    }
    unsafe extern "C" fn mock_disabled_get(_src: *const c_void, out: *mut FfiBool) -> FfiResult {
        unsafe { *out = FfiBool::TRUE };
        FfiResult::OK
    }

    let mut host = MockHost::new();

    let text_uuid = ComponentUuid::from_canonical_name("bevy_ui::widget::text::Text");
    let bg_uuid = ComponentUuid::from_canonical_name("bevy_ui::ui_node::BackgroundColor");
    let rem_uuid = ComponentUuid::from_canonical_name("bevy_text::text::RemSize");
    let disabled_uuid = ComponentUuid::from_canonical_name(BAM_DISABLED_NAME);

    host.add_comp(
        "bevy_ui::widget::text::Text",
        text_uuid,
        core::mem::size_of::<String>(),
        core::mem::align_of::<String>(),
        0,
    );
    host.add_comp(
        "bevy_ui::ui_node::BackgroundColor",
        bg_uuid,
        core::mem::size_of::<FfiRgba>(),
        core::mem::align_of::<FfiRgba>(),
        0,
    );
    host.add_comp(
        "bevy_text::text::RemSize",
        rem_uuid,
        core::mem::size_of::<FfiRemSize>(),
        core::mem::align_of::<FfiRemSize>(),
        BAM_STORAGE_FLAG_RESOURCE,
    );
    host.add_comp(
        "bamabi::ui::FfiValueChange",
        <FfiValueChange as GuestComponent>::UUID,
        core::mem::size_of::<FfiValueChange>(),
        core::mem::align_of::<FfiValueChange>(),
        0,
    );
    host.add_comp(
        "bevy_ui_widgets::Activate",
        ACTIVATE_EVENT_UUID,
        core::mem::size_of::<FfiEntity>(),
        core::mem::align_of::<FfiEntity>(),
        0,
    );

    let text_vt = MappedStringVTable {
        header: VTableHeader::new::<MappedStringVTable, String>(text_uuid),
        get: Some(mock_string_get),
        set: Some(mock_string_set),
    };
    let bg_vt = MappedWireVTable::<FfiRgba> {
        header: VTableHeader::new::<MappedWireVTable<FfiRgba>, FfiRgba>(bg_uuid),
        get: Some(mock_rgba_get),
        set: Some(mock_rgba_set),
    };
    let rem_vt = MappedWireVTable::<FfiRemSize> {
        header: VTableHeader::new::<MappedWireVTable<FfiRemSize>, FfiRemSize>(rem_uuid),
        get: Some(mock_rem_get),
        set: Some(mock_rem_set),
    };
    let disabled_vt = MappedWireVTable::<FfiBool> {
        header: VTableHeader::new::<MappedWireVTable<FfiBool>, FfiDisabled>(disabled_uuid),
        get: Some(mock_disabled_get),
        set: None,
    };

    host.symbols.insert(
        "bam.v0.Text.vtable".to_owned(),
        core::ptr::from_ref(&text_vt).cast::<c_void>(),
    );
    host.symbols.insert(
        "bam.v0.BackgroundColor.vtable".to_owned(),
        core::ptr::from_ref(&bg_vt).cast::<c_void>(),
    );
    host.symbols.insert(
        "bam.v0.RemSize.vtable".to_owned(),
        core::ptr::from_ref(&rem_vt).cast::<c_void>(),
    );
    host.symbols.insert(
        "bam.v0.Disabled.vtable".to_owned(),
        core::ptr::from_ref(&disabled_vt).cast::<c_void>(),
    );

    let api = MockHost::api();
    let load_ctx = core::ptr::from_mut(host.as_mut()).cast::<BamLoadCtx>();
    let mut inst: *mut BamInstance = core::ptr::null_mut();

    static UI_BRIDGES_OK: AtomicBool = AtomicBool::new(false);

    let manifest = BamManifest::new(c"ui_bridges_mod".as_ptr(), c"0.3.0".as_ptr());
    let status = unsafe {
        init_guest_instance(&api, load_ctx, &mut inst, &manifest, |instance, lctx| {
            build_guest_fn(instance, lctx, |app| {
                let text_b = app.ui_text_bridge().unwrap();
                let bg_b = app.background_color_bridge().unwrap();
                let rem_b = app.rem_size_bridge().unwrap();
                let dis_b = app.disabled_bridge().unwrap();

                if let Err(err) = app
                    .observe_value_change("on_val")
                    .build(|_tgt, _ev, _cmds| {})
                {
                    return err;
                }
                if let Err(err) = app
                    .observe_activate("on_act")
                    .build(|_tgt, _data, _cmds| {})
                {
                    return err;
                }

                app.system(c"ui_bridge_sys")
                    .query(|q| {
                        q.write_ui_text()
                            .write_background_color()
                            .read::<FfiDisabled>()
                            .write_rem_size()
                            .allow_disabled()
                    })
                    .build(move |ctx| {
                        let rem = rem_b.read_resource(ctx, 3)?;
                        assert!((rem.0 - 16.0).abs() < 1e-6);
                        assert_eq!(
                            rem_b.write_resource(ctx, 3, &FfiRemSize(20.0)),
                            FfiResult::OK
                        );

                        while let Some(mut chunk) = ctx.iter().next_chunk_view() {
                            let label = text_b.read_chunk_row(&mut chunk, 0, 0)?.unwrap();
                            assert_eq!(label, "Hello");
                            assert_eq!(
                                text_b.write_chunk_row(&mut chunk, 0, 0, "Updated"),
                                FfiResult::OK
                            );

                            let color = bg_b.read_chunk_row(&mut chunk, 1, 0)?.unwrap();
                            assert_eq!(color, FfiRgba::WHITE);
                            assert_eq!(
                                bg_b.write_chunk_row(&mut chunk, 1, 0, &FfiRgba::BLACK),
                                FfiResult::OK
                            );

                            let is_dis = dis_b.read_chunk_row(&mut chunk, 2, 0)?.unwrap();
                            assert!(is_dis.get());
                        }
                        UI_BRIDGES_OK.store(true, Ordering::SeqCst);
                        Ok(())
                    })
            })
        })
    };
    assert_eq!(status, FfiResult::OK);
    assert_eq!(host.observers.len(), 2);

    // Prepare 8-byte-aligned backing storage for String (`Text`), FfiRgba (`BackgroundColor`),
    // ZST (`Disabled`), and FfiRemSize (`RemSize` resource).
    let str_words = core::mem::size_of::<String>().div_ceil(8);
    let mut text_buf = vec![0u64; str_words];
    unsafe {
        core::ptr::write(
            text_buf.as_mut_ptr().cast::<String>(),
            String::from("Hello"),
        );
    }
    let bg_bytes = unsafe {
        core::slice::from_raw_parts(
            core::ptr::from_ref(&FfiRgba::WHITE).cast::<u8>(),
            core::mem::size_of::<FfiRgba>(),
        )
    };
    let rem_bytes = unsafe {
        core::slice::from_raw_parts(
            core::ptr::from_ref(&FfiRemSize(16.0)).cast::<u8>(),
            core::mem::size_of::<FfiRemSize>(),
        )
    };

    let mut cols = HashMap::new();
    cols.insert(0, text_buf);
    cols.insert(1, aligned_buf_from_bytes(bg_bytes));
    cols.insert(2, vec![0u64; 1]); // Non-null dangling-safe backing for ZST Disabled column

    let mut resources = HashMap::new();
    resources.insert(3, aligned_buf_from_bytes(rem_bytes));

    let q0_terms = host.systems[0].queries[0].clone();
    let cb = host.systems[0].callback;
    let ud = host.systems[0].user_data;
    let host_ptr = core::ptr::from_mut(host.as_mut());
    let iter0 = Box::new(MockIterState {
        host: host_ptr,
        terms: q0_terms,
        entities: vec![FfiEntity::from_bits(1)],
        columns: cols,
        removed: HashMap::new(),
        resources,
        messages: HashMap::new(),
        chunks: core::iter::once(0..1).collect(),
        cursor: 0,
        active_chunk: None,
    });
    let mut sys_ctx = MockSysCtxState {
        host: host_ptr,
        iters: vec![iter0],
        cmds: Vec::new(),
        next_entity: 2,
        entity_data: HashMap::new(),
        children: HashMap::new(),
    };
    assert_eq!(
        unsafe { cb(core::ptr::from_mut(&mut sys_ctx).cast(), ud) },
        FfiResult::OK
    );
    assert!(UI_BRIDGES_OK.load(Ordering::SeqCst));

    // Reclaim String (proving mutation + zero leak under Miri) and verify BackgroundColor & RemSize updates!
    let final_text_words = sys_ctx.iters[0].columns.remove(&0).unwrap();
    let final_text: String = unsafe { core::ptr::read(final_text_words.as_ptr().cast::<String>()) };
    assert_eq!(final_text, "Updated");
    let final_bg: FfiRgba =
        unsafe { core::ptr::read(sys_ctx.iters[0].columns[&1].as_ptr().cast::<FfiRgba>()) };
    assert_eq!(final_bg, FfiRgba::BLACK);
    let final_rem: FfiRemSize =
        unsafe { core::ptr::read(sys_ctx.iters[0].resources[&3].as_ptr().cast::<FfiRemSize>()) };
    assert!((final_rem.0 - 20.0).abs() < 1e-6);

    unsafe { shutdown_guest_instance(inst) };
}

/// Exhaustive ABI parity proof: defines exact `#[derive(ModComponent)]` mirrors of all 48
/// `bamabi_bridges` `ModComponent` types (including `TargetEntity` on all 7 `FfiPointer*` events)
/// and asserts bit-identical `(size_of, align_of, CANONICAL_NAME, UUID)` against the unconditional
/// `bamabi_guest_sys` / `bamabi_guest` wire types.
#[cfg(feature = "derive")]
#[test]
fn all_48_host_mod_components_match_derive_uuid_size_and_align() {
    use bamabi_guest::ModComponent;

    // Exact host newtype used on `bevy_picking::events::Pointer*` target fields!
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
    struct TargetEntity(pub FfiEntity);
    unsafe impl RawInsertable for TargetEntity {}
    unsafe impl AbiPod for TargetEntity {}

    fn assert_parity<G: GuestComponent, H: ModComponent>() {
        assert_eq!(
            core::mem::size_of::<G>(),
            core::mem::size_of::<H>(),
            "size mismatch for {}",
            H::CANONICAL_NAME
        );
        assert_eq!(
            core::mem::align_of::<G>(),
            core::mem::align_of::<H>(),
            "align mismatch for {}",
            H::CANONICAL_NAME
        );
        assert_eq!(
            G::NAME.to_str().unwrap(),
            H::CANONICAL_NAME,
            "canonical name mismatch for {}",
            H::CANONICAL_NAME
        );
        assert_eq!(
            G::UUID,
            H::UUID,
            "ComponentUuid (FIELD_SCHEMA_HASH) mismatch for {}",
            H::CANONICAL_NAME
        );
    }

    // ---- 1. input_ext (4) ----
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bevy_input::mouse::AccumulatedMouseMotion")]
    struct HMouseMotion {
        pub delta: [f32; 2],
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bevy_input::mouse::AccumulatedMouseScroll")]
    struct HMouseScroll {
        pub delta: [f32; 2],
        pub unit: u8,
        pub reserved0: [u8; 3],
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bevy_input::gamepad::Gamepad")]
    struct HGamepadState {
        pub pressed: u64,
        pub just_pressed: u64,
        pub just_released: u64,
        pub left_stick: [f32; 2],
        pub right_stick: [f32; 2],
        pub left_trigger: f32,
        pub right_trigger: f32,
        pub left_z: f32,
        pub right_z: f32,
        pub dpad: [f32; 2],
        pub vendor_id: u16,
        pub product_id: u16,
        pub has_vendor_id: FfiBool,
        pub has_product_id: FfiBool,
        pub connected: FfiBool,
        pub reserved0: u8,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bevy_input::gamepad::GamepadRumbleRequest")]
    struct HGamepadRumble {
        pub gamepad: FfiEntity,
        pub duration_secs: f32,
        pub strong_motor: f32,
        pub weak_motor: f32,
        pub kind: u8,
        pub reserved0: [u8; 3],
    }
    assert_parity::<FfiMouseMotion, HMouseMotion>();
    assert_parity::<FfiMouseScroll, HMouseScroll>();
    assert_parity::<FfiGamepadState, HGamepadState>();
    assert_parity::<FfiGamepadRumble, HGamepadRumble>();

    // ---- 2. state (4) ----
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bevy_state::ModStateEntry")]
    struct HModStateEntry {
        pub state_slot: u32,
        pub state_generation: u32,
        pub current_discriminant: u32,
        pub flags: u32,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bevy_state::ModStateTransitionRequest")]
    struct HModStateTransitionRequest {
        pub state_slot: u32,
        pub state_generation: u32,
        pub target_discriminant: u32,
        pub reserved0: u32,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bevy_state::ModDespawnOnExit")]
    struct HModDespawnOnExit {
        pub state_slot: u32,
        pub state_generation: u32,
        pub discriminant: u32,
        pub reserved0: u32,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bevy_state::ModDespawnOnEnter")]
    struct HModDespawnOnEnter {
        pub state_slot: u32,
        pub state_generation: u32,
        pub discriminant: u32,
        pub reserved0: u32,
    }
    assert_parity::<ModStateEntry, HModStateEntry>();
    assert_parity::<ModStateTransitionRequest, HModStateTransitionRequest>();
    assert_parity::<ModDespawnOnExit, HModDespawnOnExit>();
    assert_parity::<ModDespawnOnEnter, HModDespawnOnEnter>();

    // ---- 3. picking (8) ----
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bevy_picking::events::PointerPress")]
    struct HPointerPress {
        pub target: TargetEntity,
        pub hit_entity: FfiEntity,
        pub hit_camera: FfiEntity,
        pub window: FfiEntity,
        pub pointer_id: u64,
        pub pointer_id_hi: u64,
        pub position: [f32; 2],
        pub hit_depth: f32,
        pub hit_position: [f32; 3],
        pub hit_normal: [f32; 3],
        pub pointer_kind: u8,
        pub button: u8,
        pub count: u8,
        pub has_hit_position: FfiBool,
        pub has_hit_normal: FfiBool,
        pub reserved0: [u8; 7],
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bevy_picking::events::PointerRelease")]
    struct HPointerRelease {
        pub target: TargetEntity,
        pub hit_entity: FfiEntity,
        pub hit_camera: FfiEntity,
        pub window: FfiEntity,
        pub pointer_id: u64,
        pub pointer_id_hi: u64,
        pub position: [f32; 2],
        pub hit_depth: f32,
        pub hit_position: [f32; 3],
        pub hit_normal: [f32; 3],
        pub pointer_kind: u8,
        pub button: u8,
        pub has_hit_position: FfiBool,
        pub has_hit_normal: FfiBool,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bevy_picking::events::PointerMove")]
    struct HPointerMove {
        pub target: TargetEntity,
        pub hit_entity: FfiEntity,
        pub hit_camera: FfiEntity,
        pub window: FfiEntity,
        pub pointer_id: u64,
        pub pointer_id_hi: u64,
        pub position: [f32; 2],
        pub delta: [f32; 2],
        pub hit_depth: f32,
        pub hit_position: [f32; 3],
        pub hit_normal: [f32; 3],
        pub pointer_kind: u8,
        pub has_hit_position: FfiBool,
        pub has_hit_normal: FfiBool,
        pub reserved0: u8,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bevy_picking::events::PointerClick")]
    struct HPointerClick {
        pub target: TargetEntity,
        pub hit_entity: FfiEntity,
        pub hit_camera: FfiEntity,
        pub window: FfiEntity,
        pub pointer_id: u64,
        pub pointer_id_hi: u64,
        pub duration_secs: f64,
        pub position: [f32; 2],
        pub hit_depth: f32,
        pub hit_position: [f32; 3],
        pub hit_normal: [f32; 3],
        pub pointer_kind: u8,
        pub button: u8,
        pub count: u8,
        pub has_hit_position: FfiBool,
        pub has_hit_normal: FfiBool,
        pub reserved0: [u8; 7],
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bevy_picking::events::PointerOver")]
    struct HPointerOver {
        pub target: TargetEntity,
        pub hit_entity: FfiEntity,
        pub hit_camera: FfiEntity,
        pub window: FfiEntity,
        pub pointer_id: u64,
        pub pointer_id_hi: u64,
        pub position: [f32; 2],
        pub hit_depth: f32,
        pub hit_position: [f32; 3],
        pub hit_normal: [f32; 3],
        pub pointer_kind: u8,
        pub has_hit_position: FfiBool,
        pub has_hit_normal: FfiBool,
        pub reserved0: u8,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bevy_picking::events::PointerOut")]
    struct HPointerOut {
        pub target: TargetEntity,
        pub hit_entity: FfiEntity,
        pub hit_camera: FfiEntity,
        pub window: FfiEntity,
        pub pointer_id: u64,
        pub pointer_id_hi: u64,
        pub position: [f32; 2],
        pub hit_depth: f32,
        pub hit_position: [f32; 3],
        pub hit_normal: [f32; 3],
        pub pointer_kind: u8,
        pub has_hit_position: FfiBool,
        pub has_hit_normal: FfiBool,
        pub reserved0: u8,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bevy_picking::events::PointerDrag")]
    struct HPointerDrag {
        pub target: TargetEntity,
        pub hit_entity: FfiEntity,
        pub window: FfiEntity,
        pub pointer_id: u64,
        pub pointer_id_hi: u64,
        pub position: [f32; 2],
        pub distance: [f32; 2],
        pub delta: [f32; 2],
        pub pointer_kind: u8,
        pub button: u8,
        pub reserved0: [u8; 6],
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bevy_picking::cursor::EntityCursor")]
    struct HEntityCursor {
        pub icon: u8,
    }
    assert_parity::<FfiPointerPress, HPointerPress>();
    assert_parity::<FfiPointerRelease, HPointerRelease>();
    assert_parity::<FfiPointerMove, HPointerMove>();
    assert_parity::<FfiPointerClick, HPointerClick>();
    assert_parity::<FfiPointerOver, HPointerOver>();
    assert_parity::<FfiPointerOut, HPointerOut>();
    assert_parity::<FfiPointerDrag, HPointerDrag>();
    assert_parity::<FfiEntityCursor, HEntityCursor>();

    // ---- 4. camera_light (7) ----
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::camera_light::FfiCamera2d")]
    struct HCamera2d {
        pub order: i32,
        pub clear_color: [f32; 4],
        pub is_active: FfiBool,
        pub clear_color_mode: u8,
        pub invert_culling: FfiBool,
        pub hdr: FfiBool,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::camera_light::FfiCamera3d")]
    struct HCamera3d {
        pub order: i32,
        pub clear_color: [f32; 4],
        pub depth_clear_value: f32,
        pub depth_texture_usages: u32,
        pub exposure_ev100: f32,
        pub transmission_steps: u32,
        pub is_active: FfiBool,
        pub clear_color_mode: u8,
        pub invert_culling: FfiBool,
        pub hdr: FfiBool,
        pub depth_load_op: u8,
        pub tonemapping: u8,
        pub deband_dither: FfiBool,
        pub transmission_enabled: FfiBool,
        pub transmission_quality: u8,
        pub reserved0: [u8; 3],
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::camera_light::FfiPointLight")]
    struct HPointLight {
        pub color: [f32; 4],
        pub intensity: f32,
        pub range: f32,
        pub radius: f32,
        pub shadow_depth_bias: f32,
        pub shadow_normal_bias: f32,
        pub shadow_map_near_z: f32,
        pub shadow_maps_enabled: FfiBool,
        pub contact_shadows_enabled: FfiBool,
        pub affects_lightmapped_mesh_diffuse: FfiBool,
        pub reserved0: u8,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::camera_light::FfiSpotLight")]
    struct HSpotLight {
        pub color: [f32; 4],
        pub intensity: f32,
        pub range: f32,
        pub radius: f32,
        pub shadow_depth_bias: f32,
        pub shadow_normal_bias: f32,
        pub shadow_map_near_z: f32,
        pub outer_angle: f32,
        pub inner_angle: f32,
        pub shadow_maps_enabled: FfiBool,
        pub contact_shadows_enabled: FfiBool,
        pub affects_lightmapped_mesh_diffuse: FfiBool,
        pub reserved0: u8,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::camera_light::FfiDirectionalLight")]
    struct HDirectionalLight {
        pub color: [f32; 4],
        pub illuminance: f32,
        pub shadow_depth_bias: f32,
        pub shadow_normal_bias: f32,
        pub shadow_maps_enabled: FfiBool,
        pub contact_shadows_enabled: FfiBool,
        pub affects_lightmapped_mesh_diffuse: FfiBool,
        pub reserved0: u8,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::camera_light::FfiPanOrbitCamera")]
    struct HPanOrbitCamera {
        pub last_anchor_depth: f64,
        pub zoom_min_size_per_pixel: f64,
        pub zoom_max_size_per_pixel: f64,
        pub orbit_up: [f64; 3],
        pub momentum_pan_friction: f64,
        pub momentum_orbit_friction: f64,
        pub smoothing_pan_secs: f32,
        pub smoothing_orbit_secs: f32,
        pub smoothing_zoom_secs: f32,
        pub sensitivity_orbit: [f32; 2],
        pub sensitivity_zoom: f32,
        pub momentum_init_pan_secs: f32,
        pub momentum_init_orbit_secs: f32,
        pub input_debounce_secs: f32,
        pub perspective_near_clip_min: f32,
        pub perspective_near_clip_max: f32,
        pub perspective_near_clip_multiplier: f32,
        pub ortho_scale_to_near_clip: f32,
        pub ortho_near_clip_min: f32,
        pub ortho_near_clip_max: f32,
        pub ortho_far_clip_multiplier: f32,
        pub enabled_pan: FfiBool,
        pub enabled_orbit: FfiBool,
        pub enabled_zoom: FfiBool,
        pub orbit_constraint_mode: u8,
        pub orbit_can_pass_tdc: FfiBool,
        pub zoom_through_objects: FfiBool,
        pub momentum_pan_damping: u8,
        pub momentum_orbit_damping: u8,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::camera_light::FfiSolariLighting")]
    struct HSolariLighting {
        pub confidence_weight_cap: f32,
        pub primary_di_samples: u32,
        pub secondary_di_samples: u32,
        pub max_bounces: u32,
        pub world_cache_max_temporal_samples: f32,
        pub world_cache_direct_light_sample_count: u32,
        pub world_cache_max_gi_ray_distance: f32,
        pub world_cache_cell_updates_soft_target: u32,
        pub world_cache_position_base_cell_size: f32,
        pub world_cache_position_lod_scale: f32,
        pub restir: FfiBool,
        pub reset: FfiBool,
        pub reserved0: [u8; 2],
    }
    assert_parity::<FfiCamera2d, HCamera2d>();
    assert_parity::<FfiCamera3d, HCamera3d>();
    assert_parity::<FfiPointLight, HPointLight>();
    assert_parity::<FfiSpotLight, HSpotLight>();
    assert_parity::<FfiDirectionalLight, HDirectionalLight>();
    assert_parity::<FfiPanOrbitCamera, HPanOrbitCamera>();
    assert_parity::<FfiSolariLighting, HSolariLighting>();

    // ---- 5. audio (3) ----
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::audio::FfiAudioPlaybackSettings")]
    struct HAudioPlaybackSettings {
        pub volume: f32,
        pub speed: f32,
        pub spatial_scale: [f32; 3],
        pub start_position_secs: f32,
        pub duration_secs: f32,
        pub mode: u8,
        pub volume_mode: u8,
        pub paused: FfiBool,
        pub muted: FfiBool,
        pub spatial: FfiBool,
        pub has_spatial_scale: FfiBool,
        pub has_start_position: FfiBool,
        pub has_duration: FfiBool,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::audio::FfiAudioPlayerSpec")]
    struct HAudioPlayerSpec {
        pub source: FfiAssetHandle,
        pub settings: FfiAudioPlaybackSettings,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::audio::FfiSpatialAudioSinkControl")]
    struct HSpatialAudioSinkControl {
        pub volume: f32,
        pub speed: f32,
        pub position_secs: f32,
        pub emitter_position: [f32; 3],
        pub left_ear_position: [f32; 3],
        pub right_ear_position: [f32; 3],
        pub playback_command: u8,
        pub mute_command: u8,
        pub volume_mode: u8,
        pub apply_flags: u8,
        pub is_paused: FfiBool,
        pub is_muted: FfiBool,
        pub is_empty: FfiBool,
        pub is_spatial: FfiBool,
    }
    assert_parity::<FfiAudioPlaybackSettings, HAudioPlaybackSettings>();
    assert_parity::<FfiAudioPlayerSpec, HAudioPlayerSpec>();
    assert_parity::<FfiSpatialAudioSinkControl, HSpatialAudioSinkControl>();

    // ---- 6. ui (22) ----
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiVal")]
    struct HVal {
        pub unit: u8,
        pub reserved0: [u8; 3],
        pub value: f32,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiUiRect")]
    struct HUiRect {
        pub left: FfiVal,
        pub right: FfiVal,
        pub top: FfiVal,
        pub bottom: FfiVal,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiCornerRadius")]
    struct HCornerRadius {
        pub x: FfiVal,
        pub y: FfiVal,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiBorderRadius")]
    struct HBorderRadius {
        pub top_left: FfiCornerRadius,
        pub top_right: FfiCornerRadius,
        pub bottom_right: FfiCornerRadius,
        pub bottom_left: FfiCornerRadius,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiRgba")]
    struct HRgba {
        pub r: f32,
        pub g: f32,
        pub b: f32,
        pub a: f32,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiBorderColor")]
    struct HBorderColor {
        pub top: FfiRgba,
        pub right: FfiRgba,
        pub bottom: FfiRgba,
        pub left: FfiRgba,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiUiNode")]
    struct HUiNode {
        pub width: FfiVal,
        pub height: FfiVal,
        pub min_width: FfiVal,
        pub min_height: FfiVal,
        pub max_width: FfiVal,
        pub max_height: FfiVal,
        pub left: FfiVal,
        pub right: FfiVal,
        pub top: FfiVal,
        pub bottom: FfiVal,
        pub flex_basis: FfiVal,
        pub row_gap: FfiVal,
        pub column_gap: FfiVal,
        pub padding: FfiUiRect,
        pub margin: FfiUiRect,
        pub border: FfiUiRect,
        pub border_radius: FfiBorderRadius,
        pub flex_grow: f32,
        pub flex_shrink: f32,
        pub display: u8,
        pub position_type: u8,
        pub flex_direction: u8,
        pub flex_wrap: u8,
        pub align_items: u8,
        pub justify_content: u8,
        pub fixed_node: FfiBool,
        pub reserved0: u8,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiFontSize")]
    struct HFontSize {
        pub unit: u8,
        pub reserved0: [u8; 3],
        pub value: f32,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiUiTextStyle")]
    struct HUiTextStyle {
        pub font_handle: FfiAssetHandle,
        pub color: FfiRgba,
        pub font_size: FfiFontSize,
        pub weight: u16,
        pub justify: u8,
        pub linebreak: u8,
        pub font_smoothing: u8,
        pub font_style: u8,
        pub ensure_text: FfiBool,
        pub reserved0: u8,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiEditableTextConfig")]
    struct HEditableTextConfig {
        pub cursor_margin: [f32; 2],
        pub cursor_width: f32,
        pub cursor_blink_period_secs: f32,
        pub visible_lines: f32,
        pub visible_width: f32,
        pub max_characters: u32,
        pub interactive: FfiBool,
        pub allow_newlines: FfiBool,
        pub read_write_mode: u8,
        pub has_max_characters: FfiBool,
        pub has_visible_lines: FfiBool,
        pub has_visible_width: FfiBool,
        pub reserved0: [u8; 2],
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiInlineBox")]
    struct HInlineBox {
        pub size: [f32; 2],
        pub kind: u8,
        pub reserved0: [u8; 3],
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiInlineImage")]
    struct HInlineImage {
        pub image: FfiAssetHandle,
        pub color: FfiRgba,
        pub width: f32,
        pub height: f32,
        pub has_width: FfiBool,
        pub has_height: FfiBool,
        pub flip_x: FfiBool,
        pub flip_y: FfiBool,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiTabList")]
    struct HTabList {
        pub orientation: u8,
        pub activation: u8,
        pub self_update: FfiBool,
        pub disabled: FfiBool,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiTab")]
    struct HTab {
        pub disabled: FfiBool,
        pub reserved0: [u8; 3],
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiSelectedTab")]
    struct HSelectedTab {
        pub selected: FfiEntity,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiValueChange")]
    struct HValueChange {
        pub source: FfiEntity,
        pub entity_value: FfiEntity,
        pub f64_value: f64,
        pub i64_value: i64,
        pub color_value: FfiRgba,
        pub f32_value: f32,
        pub kind: u8,
        pub bool_value: FfiBool,
        pub is_final: FfiBool,
        pub reserved0: u8,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiThemeContext")]
    struct HThemeContext {
        pub surface_level: u8,
        pub reserved0: [u8; 3],
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiFeathersColorInput")]
    struct HFeathersColorInput {
        pub color: FfiRgba,
        pub opaque_color_percentage: f32,
        pub mode: u8,
        pub apply_mode_to_settings: FfiBool,
        pub disabled: FfiBool,
        pub reserved0: u8,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiFeathersDropdown")]
    struct HFeathersDropdown {
        pub selected_row: FfiEntity,
        pub selected_index: u32,
        pub max_visible: u32,
        pub has_selected_index: FfiBool,
        pub rounded_corners: u8,
        pub disabled: FfiBool,
        pub reserved0: [u8; 5],
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiFeathersScrollableList")]
    struct HFeathersScrollableList {
        pub scrollbar_gutter: FfiVal,
        pub orientation: u8,
        pub selectable: FfiBool,
        pub scrollable: FfiBool,
        pub disabled: FfiBool,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiFeathersListRow")]
    struct HFeathersListRow {
        pub option_index: u32,
        pub has_option_index: FfiBool,
        pub selected: FfiBool,
        pub disabled: FfiBool,
        pub reserved0: u8,
    }
    #[repr(C)]
    #[derive(Clone, Copy, ModComponent)]
    #[mod_component(name = "bamabi::ui::FfiFeathersNumberInput")]
    struct HFeathersNumberInput {
        pub value_f64: f64,
        pub value_i64: i64,
        pub soft_min: f64,
        pub soft_max: f64,
        pub hard_min: f64,
        pub hard_max: f64,
        pub step: f64,
        pub precision: i32,
        pub format: u8,
        pub has_soft_limit: FfiBool,
        pub has_hard_limit: FfiBool,
        pub wrap: FfiBool,
        pub disabled: FfiBool,
        pub reserved0: [u8; 7],
    }
    assert_parity::<FfiVal, HVal>();
    assert_parity::<FfiUiRect, HUiRect>();
    assert_parity::<FfiCornerRadius, HCornerRadius>();
    assert_parity::<FfiBorderRadius, HBorderRadius>();
    assert_parity::<FfiRgba, HRgba>();
    assert_parity::<FfiBorderColor, HBorderColor>();
    assert_parity::<FfiUiNode, HUiNode>();
    assert_parity::<FfiFontSize, HFontSize>();
    assert_parity::<FfiUiTextStyle, HUiTextStyle>();
    assert_parity::<FfiEditableTextConfig, HEditableTextConfig>();
    assert_parity::<FfiInlineBox, HInlineBox>();
    assert_parity::<FfiInlineImage, HInlineImage>();
    assert_parity::<FfiTabList, HTabList>();
    assert_parity::<FfiTab, HTab>();
    assert_parity::<FfiSelectedTab, HSelectedTab>();
    assert_parity::<FfiValueChange, HValueChange>();
    assert_parity::<FfiThemeContext, HThemeContext>();
    assert_parity::<FfiFeathersColorInput, HFeathersColorInput>();
    assert_parity::<FfiFeathersDropdown, HFeathersDropdown>();
    assert_parity::<FfiFeathersScrollableList, HFeathersScrollableList>();
    assert_parity::<FfiFeathersListRow, HFeathersListRow>();
    assert_parity::<FfiFeathersNumberInput, HFeathersNumberInput>();
}

/// Scenario: Verifies complete 194-key `FfiKeyCode` parity (`0..=193` matching
/// `bamabi_bridges::std_types::FfiKeyCode::from_bevy`), full 4-word bitset coverage on
/// `FfiKeyboardInput`, and `FfiUiRect` / `FfiBorderRadius` / `FfiRgba` / `FfiBorderColor` /
/// `FfiValueChange` constructor and `Default` parity with `bamabi_bridges::ui`.
#[test]
fn ffi_key_code_all_194_indices_and_ui_wire_constructors_match_bridges() {
    assert_eq!(FfiKeyCode::COUNT, 194);
    assert_eq!(FfiKeyCode::ALL.len(), 194);
    for (idx, &code) in FfiKeyCode::ALL.iter().enumerate() {
        assert_eq!(code.0 as usize, idx);
        assert!(code.is_valid());
    }
    assert!(!FfiKeyCode(194).is_valid());
    assert!(!FfiKeyCode(255).is_valid());
    assert!(!FfiKeyCode(256).is_valid());

    // Spot-check key boundary indices across every group against `bamabi_bridges::std_types`.
    assert_eq!(FfiKeyCode::BACKQUOTE.0, 0);
    assert_eq!(FfiKeyCode::INTL_BACKSLASH.0, 16);
    assert_eq!(FfiKeyCode::INTL_RO.0, 17);
    assert_eq!(FfiKeyCode::INTL_YEN.0, 18);
    assert_eq!(FfiKeyCode::KEY_A.0, 19);
    assert_eq!(FfiKeyCode::KEY_Z.0, 44);
    assert_eq!(FfiKeyCode::CONTEXT_MENU.0, 54);
    assert_eq!(FfiKeyCode::CONVERT.0, 64);
    assert_eq!(FfiKeyCode::KANA_MODE.0, 65);
    assert_eq!(FfiKeyCode::LANG_1.0, 66);
    assert_eq!(FfiKeyCode::LANG_5.0, 70);
    assert_eq!(FfiKeyCode::NON_CONVERT.0, 71);
    assert_eq!(FfiKeyCode::HELP.0, 74);
    assert_eq!(FfiKeyCode::NUM_LOCK.0, 83);
    assert_eq!(FfiKeyCode::NUMPAD_0.0, 84);
    assert_eq!(FfiKeyCode::NUMPAD_9.0, 93);
    assert_eq!(FfiKeyCode::NUMPAD_ADD.0, 94);
    assert_eq!(FfiKeyCode::NUMPAD_ENTER.0, 101);
    assert_eq!(FfiKeyCode::NUMPAD_SUBTRACT.0, 113);
    assert_eq!(FfiKeyCode::ESCAPE.0, 114);
    assert_eq!(FfiKeyCode::FN.0, 115);
    assert_eq!(FfiKeyCode::FN_LOCK.0, 116);
    assert_eq!(FfiKeyCode::PRINT_SCREEN.0, 117);
    assert_eq!(FfiKeyCode::SCROLL_LOCK.0, 118);
    assert_eq!(FfiKeyCode::PAUSE.0, 119);
    assert_eq!(FfiKeyCode::BROWSER_BACK.0, 120);
    assert_eq!(FfiKeyCode::BROWSER_STOP.0, 126);
    assert_eq!(FfiKeyCode::EJECT.0, 127);
    assert_eq!(FfiKeyCode::LAUNCH_APP_1.0, 128);
    assert_eq!(FfiKeyCode::LAUNCH_APP_2.0, 129);
    assert_eq!(FfiKeyCode::LAUNCH_MAIL.0, 130);
    assert_eq!(FfiKeyCode::MEDIA_PLAY_PAUSE.0, 131);
    assert_eq!(FfiKeyCode::MEDIA_TRACK_PREVIOUS.0, 135);
    assert_eq!(FfiKeyCode::POWER.0, 136);
    assert_eq!(FfiKeyCode::SLEEP.0, 137);
    assert_eq!(FfiKeyCode::AUDIO_VOLUME_DOWN.0, 138);
    assert_eq!(FfiKeyCode::AUDIO_VOLUME_MUTE.0, 139);
    assert_eq!(FfiKeyCode::AUDIO_VOLUME_UP.0, 140);
    assert_eq!(FfiKeyCode::WAKE_UP.0, 141);
    assert_eq!(FfiKeyCode::META.0, 142);
    assert_eq!(FfiKeyCode::HYPER.0, 143);
    assert_eq!(FfiKeyCode::TURBO.0, 144);
    assert_eq!(FfiKeyCode::ABORT.0, 145);
    assert_eq!(FfiKeyCode::UNDO.0, 156);
    assert_eq!(FfiKeyCode::HIRAGANA.0, 157);
    assert_eq!(FfiKeyCode::KATAKANA.0, 158);
    assert_eq!(FfiKeyCode::F1.0, 159);
    assert_eq!(FfiKeyCode::F12.0, 170);
    assert_eq!(FfiKeyCode::F13.0, 171);
    assert_eq!(FfiKeyCode::F35.0, 193);

    // Exercise every single one of the 194 named keys across all 4 `u64` words of `FfiKeyboardInput`.
    let mut kb = FfiKeyboardInput::default();
    for &code in &FfiKeyCode::ALL {
        kb.press(code);
        assert!(kb.pressed(code));
        assert!(kb.just_pressed(code));
        assert!(!kb.just_released(code));
    }
    assert!(kb.all_pressed(FfiKeyCode::ALL));
    assert!(kb.all_just_pressed([
        FfiKeyCode::NUM_LOCK,
        FfiKeyCode::NUMPAD_5,
        FfiKeyCode::PRINT_SCREEN,
        FfiKeyCode::PAUSE,
        FfiKeyCode::MEDIA_PLAY_PAUSE,
        FfiKeyCode::F35,
    ]));
    kb.clear();
    assert!(!kb.any_just_pressed(FfiKeyCode::ALL));
    assert!(kb.all_pressed(FfiKeyCode::ALL));

    for &code in &FfiKeyCode::ALL {
        kb.release(code);
        assert!(!kb.pressed(code));
        assert!(kb.just_released(code));
    }
    assert!(kb.all_just_released(FfiKeyCode::ALL));
    // Out-of-range key indices (`>= 256`) fail closed without panicking or mutating bits.
    kb.press(FfiKeyCode(256));
    assert!(!kb.pressed(FfiKeyCode(256)));
    assert!(!kb.just_pressed(FfiKeyCode(256)));
    assert!(!kb.just_released(FfiKeyCode(256)));

    // UI wire constructors & `Default` parity with `bamabi_bridges::ui`.
    let rect = FfiUiRect::new(
        FfiVal::px(1.0),
        FfiVal::percent(2.0),
        FfiVal::em(3.0),
        FfiVal::rem(4.0),
    );
    assert_eq!(rect.left, FfiVal::px(1.0));
    assert_eq!(rect.right, FfiVal::percent(2.0));
    assert_eq!(rect.top, FfiVal::em(3.0));
    assert_eq!(rect.bottom, FfiVal::rem(4.0));

    let ell = FfiBorderRadius::elliptical_all(FfiVal::px(12.0), FfiVal::percent(25.0));
    let expected_corner = FfiCornerRadius::elliptical(FfiVal::px(12.0), FfiVal::percent(25.0));
    assert_eq!(ell.top_left, expected_corner);
    assert_eq!(ell.top_right, expected_corner);
    assert_eq!(ell.bottom_right, expected_corner);
    assert_eq!(ell.bottom_left, expected_corner);

    let corners = FfiBorderRadius::corners_px(2.0, 4.0, 6.0, 8.0);
    assert_eq!(corners.top_left, FfiCornerRadius::px(2.0));
    assert_eq!(corners.top_right, FfiCornerRadius::px(4.0));
    assert_eq!(corners.bottom_right, FfiCornerRadius::px(6.0));
    assert_eq!(corners.bottom_left, FfiCornerRadius::px(8.0));

    assert_eq!(FfiRgba::default(), FfiRgba::WHITE);
    assert_eq!(
        FfiBorderColor::default(),
        FfiBorderColor::all(FfiRgba::TRANSPARENT)
    );
    let vc = FfiValueChange::default();
    assert_eq!(vc.source, FfiEntity::NULL);
    assert_eq!(vc.entity_value, FfiEntity::NULL);
    assert_eq!(vc.color_value, FfiRgba::TRANSPARENT);
    assert_eq!(vc.kind, FfiValueChange::KIND_ENTITY);
    assert_eq!(vc.bool_value, FfiBool::FALSE);
    assert_eq!(vc.is_final, FfiBool::TRUE);
}

/// Regression: Verifies `GuestCommandsAudioExt::spawn_audio` resolves `FfiAudioPlayerSpec` before
/// reserving any entity (zero orphan entities when unregistered), spawns atomically on ABI `0.3.0`,
/// and handles ABI `0.2.0` fallback + `insert`-error despawn cleanup without leaking orphans.
#[test]
fn spawn_audio_resolves_before_spawn_no_orphan_entity_and_v02_fallback() {
    let source = FfiAssetHandle::from_index(BamComponent::from_raw(9), 77, 1);
    let settings = FfiAudioPlaybackSettings {
        volume: 0.75,
        speed: 1.25,
        ..FfiAudioPlaybackSettings::default()
    };

    // 1. Unregistered `FfiAudioPlayerSpec`: returns `NOT_FOUND` and queues ZERO commands (no orphan).
    {
        let mut host = MockHost::new();
        let api = MockHost::api();
        let load_ctx = core::ptr::from_mut(host.as_mut()).cast::<BamLoadCtx>();
        let mut inst: *mut BamInstance = core::ptr::null_mut();
        let manifest = BamManifest::new(c"audio_unreg_mod".as_ptr(), c"0.3.0".as_ptr());

        static SAW_UNREG_ERR: AtomicBool = AtomicBool::new(false);
        let status = unsafe {
            init_guest_instance(&api, load_ctx, &mut inst, &manifest, |instance, lctx| {
                build_guest_fn(instance, lctx, |app| {
                    app.system(c"try_unreg_audio").build(move |ctx| {
                        let res = ctx.commands().spawn_audio(source, settings);
                        assert_eq!(res, Err(FfiResult::NOT_FOUND));
                        // Also check disabled/null command buffer fails closed with `NULL_POINTER`.
                        let mut null_cmds = GuestCommands::from_validated(
                            bamabi_guest_sys::ValidatedCommands::empty(ctx.host()),
                        );
                        assert_eq!(
                            null_cmds.spawn_audio(source, settings),
                            Err(FfiResult::NULL_POINTER)
                        );
                        SAW_UNREG_ERR.store(true, Ordering::SeqCst);
                    })
                })
            })
        };
        assert_eq!(status, FfiResult::OK);
        let cb = host.systems[0].callback;
        let ud = host.systems[0].user_data;
        let host_ptr = core::ptr::from_mut(host.as_mut());
        let mut sys_ctx = MockSysCtxState {
            host: host_ptr,
            iters: Vec::new(),
            cmds: Vec::new(),
            next_entity: 100,
            entity_data: HashMap::new(),
            children: HashMap::new(),
        };
        assert_eq!(
            unsafe { cb(core::ptr::from_mut(&mut sys_ctx).cast(), ud) },
            FfiResult::OK
        );
        assert!(SAW_UNREG_ERR.load(Ordering::SeqCst));
        assert!(
            sys_ctx.cmds.is_empty(),
            "orphan entity leaked into command buffer: {:?}",
            sys_ctx.cmds
        );
        assert_eq!(sys_ctx.next_entity, 100);
        unsafe { shutdown_guest_instance(inst) };
    }

    // 2. Registered `FfiAudioPlayerSpec` on ABI 0.3.0: single atomic `cmd_spawn` with exact payload.
    {
        let mut host = MockHost::new();
        let spec_handle = host.add_comp(
            "bamabi::audio::FfiAudioPlayerSpec",
            <FfiAudioPlayerSpec as GuestComponent>::UUID,
            core::mem::size_of::<FfiAudioPlayerSpec>(),
            core::mem::align_of::<FfiAudioPlayerSpec>(),
            0,
        );
        let api = MockHost::api();
        let load_ctx = core::ptr::from_mut(host.as_mut()).cast::<BamLoadCtx>();
        let mut inst: *mut BamInstance = core::ptr::null_mut();
        let manifest = BamManifest::new(c"audio_ok_mod".as_ptr(), c"0.3.0".as_ptr());

        static SPAWNED_BITS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        let status = unsafe {
            init_guest_instance(&api, load_ctx, &mut inst, &manifest, |instance, lctx| {
                build_guest_fn(instance, lctx, |app| {
                    app.system(c"spawn_audio_ok").build(move |ctx| {
                        let ent = ctx.commands().spawn_audio(source, settings).unwrap();
                        SPAWNED_BITS.store(ent.to_bits(), Ordering::SeqCst);
                    })
                })
            })
        };
        assert_eq!(status, FfiResult::OK);
        let cb = host.systems[0].callback;
        let ud = host.systems[0].user_data;
        let host_ptr = core::ptr::from_mut(host.as_mut());
        let mut sys_ctx = MockSysCtxState {
            host: host_ptr,
            iters: Vec::new(),
            cmds: Vec::new(),
            next_entity: 200,
            entity_data: HashMap::new(),
            children: HashMap::new(),
        };
        assert_eq!(
            unsafe { cb(core::ptr::from_mut(&mut sys_ctx).cast(), ud) },
            FfiResult::OK
        );
        let ent = FfiEntity::from_bits(SPAWNED_BITS.load(Ordering::SeqCst));
        assert_eq!(ent, FfiEntity::from_bits(200));
        let expected_spec = FfiAudioPlayerSpec::new(source, settings);
        let expected_bytes = unsafe {
            core::slice::from_raw_parts(
                core::ptr::from_ref(&expected_spec).cast::<u8>(),
                core::mem::size_of::<FfiAudioPlayerSpec>(),
            )
            .to_vec()
        };
        assert_eq!(
            sys_ctx.cmds,
            vec![
                MockCmd::SpawnEmpty(ent),
                MockCmd::Insert(ent, spec_handle, expected_bytes),
            ]
        );
        unsafe { shutdown_guest_instance(inst) };
    }

    // 3. ABI 0.2.0 host (`component_layout_info = None`): fallback succeeds when insert succeeds,
    //    and queues `Despawn` cleanup if `insert` fails (`PERMISSION_DENIED`).
    for allow_raw_insert in [true, false] {
        let mut host = MockHost::new();
        let spec_handle = host.add_comp(
            "bamabi::audio::FfiAudioPlayerSpec",
            <FfiAudioPlayerSpec as GuestComponent>::UUID,
            core::mem::size_of::<FfiAudioPlayerSpec>(),
            core::mem::align_of::<FfiAudioPlayerSpec>(),
            0,
        );
        host.comps
            .iter_mut()
            .find(|c| c.handle == spec_handle)
            .unwrap()
            .raw_insert_allowed = allow_raw_insert;

        let mut api = MockHost::api();
        api.component_layout_info = None; // Triggers `FfiResult::UNSUPPORTED` in `spawn`
        let load_ctx = core::ptr::from_mut(host.as_mut()).cast::<BamLoadCtx>();
        let mut inst: *mut BamInstance = core::ptr::null_mut();
        let manifest = BamManifest::new(c"audio_v02_mod".as_ptr(), c"0.3.0".as_ptr());

        let status = unsafe {
            init_guest_instance(&api, load_ctx, &mut inst, &manifest, |instance, lctx| {
                build_guest_fn(instance, lctx, |app| {
                    app.system(c"spawn_audio_v02").build(move |ctx| {
                        let res = ctx.commands().spawn_audio(source, settings);
                        if allow_raw_insert {
                            assert_eq!(res, Ok(FfiEntity::from_bits(300)));
                        } else {
                            assert_eq!(res, Err(FfiResult::PERMISSION_DENIED));
                        }
                    })
                })
            })
        };
        assert_eq!(status, FfiResult::OK);
        let cb = host.systems[0].callback;
        let ud = host.systems[0].user_data;
        let host_ptr = core::ptr::from_mut(host.as_mut());
        let mut sys_ctx = MockSysCtxState {
            host: host_ptr,
            iters: Vec::new(),
            cmds: Vec::new(),
            next_entity: 300,
            entity_data: HashMap::new(),
            children: HashMap::new(),
        };
        assert_eq!(
            unsafe { cb(core::ptr::from_mut(&mut sys_ctx).cast(), ud) },
            FfiResult::OK
        );
        let ent = FfiEntity::from_bits(300);
        if allow_raw_insert {
            assert_eq!(sys_ctx.cmds.len(), 2);
            assert_eq!(sys_ctx.cmds[0], MockCmd::SpawnEmpty(ent));
            assert!(
                matches!(sys_ctx.cmds[1], MockCmd::Insert(e, h, _) if e == ent && h == spec_handle)
            );
        } else {
            assert_eq!(
                sys_ctx.cmds,
                vec![MockCmd::SpawnEmpty(ent), MockCmd::Despawn(ent)]
            );
        }
        unsafe { shutdown_guest_instance(inst) };
    }
}

/// Scenario: Exercises `GuestUiHandles`, `GuestUiQueryBuilderExt::{read_text_span, write_text_span}`,
/// `GuestCommandsUiExt::{insert_default_*, trigger_activate, trigger_activate_with}`,
/// `GuestStdSystemBuilderExt::write_gamepad_rumble`, and `GuestGamepadRumbleExt` end-to-end
/// (including optional `ThemeContext` absent vs present, alias fallback, `TYPE_MISMATCH` rejection,
/// and `FfiEntity::NULL` validation).
#[test]
fn ui_handles_text_span_default_inserters_trigger_activate_and_gamepad_rumble_end_to_end() {
    use bamabi_guest::raw::MappedStringVTable;

    unsafe extern "C" fn mock_string_get(ptr: *const c_void) -> StrView {
        let s = unsafe { &*ptr.cast::<String>() };
        StrView::from_str(s.as_str())
    }
    unsafe extern "C" fn mock_string_set(ptr: *mut c_void, view: StrView) -> FfiResult {
        let s = unsafe { &mut *ptr.cast::<String>() };
        let Ok(slice) = (unsafe { view.try_as_str() }) else {
            return FfiResult::INVALID_UTF8;
        };
        s.clear();
        s.push_str(slice);
        FfiResult::OK
    }

    // Part A: Full UI handles WITHOUT optional `bridge-feathers` (`ThemeContext` absent) +
    // `TextSpan` query/bridge + `trigger_activate` + `FfiGamepadRumble` messages.
    {
        let mut host = MockHost::new();
        let text_uuid = ComponentUuid::from_canonical_name("bevy_ui::widget::text::Text");
        let span_uuid = ComponentUuid::from_canonical_name("bevy_text::text::TextSpan");
        let bg_uuid = ComponentUuid::from_canonical_name("bevy_ui::ui_node::BackgroundColor");
        let border_uuid = ComponentUuid::from_canonical_name("bevy_ui::ui_node::BorderColor");
        let em_uuid = ComponentUuid::from_canonical_name("bevy_text::text::EmSize");

        let text_h = host.add_comp("bevy_ui::widget::text::Text", text_uuid, 24, 8, 0);
        let span_h = host.add_comp(
            "bevy_text::text::TextSpan",
            span_uuid,
            core::mem::size_of::<String>(),
            core::mem::align_of::<String>(),
            0,
        );
        let bg_h = host.add_comp("bevy_ui::ui_node::BackgroundColor", bg_uuid, 16, 4, 0);
        let border_h = host.add_comp("bevy_ui::ui_node::BorderColor", border_uuid, 64, 4, 0);
        let em_h = host.add_comp("bevy_text::text::EmSize", em_uuid, 4, 4, 0);
        let act_h = host.add_comp(
            "bevy_ui_widgets::Activate",
            ACTIVATE_EVENT_UUID,
            core::mem::size_of::<FfiEntity>(),
            core::mem::align_of::<FfiEntity>(),
            0,
        );
        let _rumble_h = host.add_comp(
            "bevy_input::gamepad::GamepadRumbleRequest",
            <FfiGamepadRumble as GuestComponent>::UUID,
            core::mem::size_of::<FfiGamepadRumble>(),
            core::mem::align_of::<FfiGamepadRumble>(),
            BAM_STORAGE_FLAG_MESSAGE,
        );

        let span_vt = MappedStringVTable {
            header: VTableHeader::new::<MappedStringVTable, String>(span_uuid),
            get: Some(mock_string_get),
            set: Some(mock_string_set),
        };
        host.symbols.insert(
            "bam.v0.TextSpan.vtable".to_owned(),
            core::ptr::from_ref(&span_vt).cast::<c_void>(),
        );

        let api = MockHost::api();
        let load_ctx = core::ptr::from_mut(host.as_mut()).cast::<BamLoadCtx>();
        let mut inst: *mut BamInstance = core::ptr::null_mut();
        let manifest = BamManifest::new(c"ui_input_parity_mod".as_ptr(), c"0.3.0".as_ptr());

        static PARITY_RAN: AtomicBool = AtomicBool::new(false);
        let target_ent = FfiEntity::from_bits(42);
        let gp_ent = FfiEntity::from_bits(77);

        let status = unsafe {
            init_guest_instance(&api, load_ctx, &mut inst, &manifest, |instance, lctx| {
                build_guest_fn(instance, lctx, |app| {
                    let handles = app.ui_handles().unwrap();
                    assert_eq!(handles.text, text_h);
                    assert_eq!(handles.text_span, span_h);
                    assert_eq!(handles.background_color, bg_h);
                    assert_eq!(handles.border_color, border_h);
                    assert_eq!(handles.em_size, em_h);
                    assert_eq!(handles.theme_context, None);
                    assert_eq!(handles.activate, act_h);

                    let span_bridge = app.text_span_bridge().unwrap();

                    app.system(c"ui_and_rumble_sys")
                        .query(|q| q.write_text_span())
                        .query(|q| q.read_text_span())
                        .write_gamepad_rumble()
                        .build(move |ctx| {
                            // 1. Read & mutate `TextSpan` via `GuestTextSpanBridge`.
                            while let Some(mut chunk) = ctx.iter().next_chunk_view() {
                                let before = span_bridge.read_chunk_row(&mut chunk, 0, 0)?.unwrap();
                                assert_eq!(before, "span-before");
                                assert_eq!(
                                    span_bridge.write_chunk_row(&mut chunk, 0, 0, "span-after"),
                                    FfiResult::OK
                                );
                            }

                            // 2. Emit `FfiGamepadRumble` messages (add, stop, raw, batch, bad idx).
                            assert_eq!(
                                ctx.send_gamepad_rumble_add(0, gp_ent, 0.5, 0.8, 0.25),
                                FfiResult::OK
                            );
                            assert_eq!(ctx.send_gamepad_rumble_stop(0, gp_ent), FfiResult::OK);
                            let raw_req = FfiGamepadRumble::add(gp_ent, 0.25, 0.4, 0.6);
                            assert_eq!(ctx.send_gamepad_rumble(0, raw_req), FfiResult::OK);
                            ctx.send_gamepad_rumbles(
                                0,
                                &[
                                    FfiGamepadRumble::add(gp_ent, 0.1, 1.0, 0.0),
                                    FfiGamepadRumble::stop(gp_ent),
                                ],
                            )?;
                            assert_eq!(
                                ctx.send_gamepad_rumble_stop(9, gp_ent),
                                FfiResult::OUT_OF_RANGE
                            );

                            // 3. Insert all host-defaulted UI components + trigger `Activate`.
                            let cmds = ctx.commands();
                            assert_eq!(
                                cmds.insert_default_ui_text(target_ent, &handles),
                                FfiResult::OK
                            );
                            assert_eq!(
                                cmds.insert_default_text_span(target_ent, &handles),
                                FfiResult::OK
                            );
                            assert_eq!(
                                cmds.insert_default_background_color(target_ent, &handles),
                                FfiResult::OK
                            );
                            assert_eq!(
                                cmds.insert_default_border_color(target_ent, &handles),
                                FfiResult::OK
                            );
                            assert_eq!(
                                cmds.insert_default_em_size(target_ent, &handles),
                                FfiResult::OK
                            );
                            assert_eq!(
                                cmds.insert_default_theme_context(target_ent, &handles),
                                FfiResult::NOT_FOUND
                            );
                            assert_eq!(
                                cmds.trigger_activate(FfiEntity::NULL),
                                Err(FfiResult::INVALID_ENTITY)
                            );
                            assert_eq!(
                                cmds.trigger_activate_with(&handles, FfiEntity::NULL),
                                Err(FfiResult::INVALID_ENTITY)
                            );
                            cmds.trigger_activate(target_ent)?;
                            cmds.trigger_activate_with(&handles, target_ent)?;

                            PARITY_RAN.store(true, Ordering::SeqCst);
                            Ok(())
                        })
                })
            })
        };
        assert_eq!(status, FfiResult::OK);
        // Verify `write_text_span` / `read_text_span` query terms and `MESSAGE_WRITE` term.
        assert_eq!(host.systems[0].queries[0][0].component, span_h);
        assert_eq!(host.systems[0].queries[0][0].mode, AccessMode::WRITE);
        assert_eq!(host.systems[0].queries[1][0].component, span_h);
        assert_eq!(host.systems[0].queries[1][0].mode, AccessMode::READ);
        assert_eq!(
            host.systems[0].queries[2][0].mode,
            AccessMode::MESSAGE_WRITE
        );

        // Allocate aligned backing storage for `String` in `TextSpan` column 0.
        let str_words = core::mem::size_of::<String>().div_ceil(8);
        let mut span_buf = vec![0u64; str_words];
        unsafe {
            core::ptr::write(
                span_buf.as_mut_ptr().cast::<String>(),
                String::from("span-before"),
            );
        }
        let mut cols = HashMap::new();
        cols.insert(0, span_buf);

        let q0 = host.systems[0].queries[0].clone();
        let q1 = host.systems[0].queries[1].clone();
        let q2 = host.systems[0].queries[2].clone();
        let cb = host.systems[0].callback;
        let ud = host.systems[0].user_data;

        let host_ptr = core::ptr::from_mut(host.as_mut());
        let iter0 = Box::new(MockIterState {
            host: host_ptr,
            terms: q0,
            entities: vec![target_ent],
            columns: cols,
            removed: HashMap::new(),
            resources: HashMap::new(),
            messages: HashMap::new(),
            chunks: core::iter::once(0..1).collect(),
            cursor: 0,
            active_chunk: None,
        });
        let iter1 = Box::new(MockIterState {
            host: host_ptr,
            terms: q1,
            entities: Vec::new(),
            columns: HashMap::new(),
            removed: HashMap::new(),
            resources: HashMap::new(),
            messages: HashMap::new(),
            chunks: Vec::new(),
            cursor: 0,
            active_chunk: None,
        });
        let iter_res = Box::new(MockIterState {
            host: host_ptr,
            terms: q2,
            entities: Vec::new(),
            columns: HashMap::new(),
            removed: HashMap::new(),
            resources: HashMap::new(),
            messages: HashMap::new(),
            chunks: Vec::new(),
            cursor: 0,
            active_chunk: None,
        });
        let mut sys_ctx = MockSysCtxState {
            host: host_ptr,
            iters: vec![iter0, iter1, iter_res],
            cmds: Vec::new(),
            next_entity: 50,
            entity_data: HashMap::new(),
            children: HashMap::new(),
        };
        assert_eq!(
            unsafe { cb(core::ptr::from_mut(&mut sys_ctx).cast(), ud) },
            FfiResult::OK
        );
        assert!(PARITY_RAN.load(Ordering::SeqCst));

        // Reclaim `String` so Miri verifies zero memory leaks and exact text mutation.
        let final_span_words = sys_ctx.iters[0].columns.remove(&0).unwrap();
        let final_span: String =
            unsafe { core::ptr::read(final_span_words.as_ptr().cast::<String>()) };
        assert_eq!(final_span, "span-after");

        // Verify all 5 emitted `FfiGamepadRumble` messages.
        let sent_msgs = &sys_ctx.iters[2].messages[&0];
        assert_eq!(sent_msgs.len(), 5);
        let decode_rumble = |item: &MockMsgItem| -> FfiGamepadRumble {
            let MockMsgItem::Bytes(bytes) = item else {
                panic!("expected POD bytes");
            };
            assert_eq!(bytes.len(), core::mem::size_of::<FfiGamepadRumble>());
            let aligned = aligned_buf_from_bytes(bytes);
            unsafe { core::ptr::read(aligned.as_ptr().cast::<FfiGamepadRumble>()) }
        };
        assert_eq!(
            decode_rumble(&sent_msgs[0]),
            FfiGamepadRumble::add(gp_ent, 0.5, 0.8, 0.25)
        );
        assert_eq!(decode_rumble(&sent_msgs[1]), FfiGamepadRumble::stop(gp_ent));
        assert_eq!(
            decode_rumble(&sent_msgs[2]),
            FfiGamepadRumble::add(gp_ent, 0.25, 0.4, 0.6)
        );
        assert_eq!(
            decode_rumble(&sent_msgs[3]),
            FfiGamepadRumble::add(gp_ent, 0.1, 1.0, 0.0)
        );
        assert_eq!(decode_rumble(&sent_msgs[4]), FfiGamepadRumble::stop(gp_ent));

        // Verify all 5 `InsertDefault` commands + 2 `Activate` triggers (with target `FfiEntity` payload).
        let target_bytes = target_ent.to_bits().to_ne_bytes().to_vec();
        assert_eq!(
            sys_ctx.cmds,
            vec![
                MockCmd::InsertDefault(target_ent, text_h),
                MockCmd::InsertDefault(target_ent, span_h),
                MockCmd::InsertDefault(target_ent, bg_h),
                MockCmd::InsertDefault(target_ent, border_h),
                MockCmd::InsertDefault(target_ent, em_h),
                MockCmd::Trigger(act_h, target_ent, target_bytes.clone(), 0),
                MockCmd::Trigger(act_h, target_ent, target_bytes, 0),
            ]
        );
        unsafe { shutdown_guest_instance(inst) };
    }

    // Part B: With `ThemeContext` present + short-alias fallback ("Activate") + `TYPE_MISMATCH` guard.
    {
        let mut host = MockHost::new();
        host.add_comp(
            "Text",
            ComponentUuid::from_canonical_name("bevy_ui::widget::text::Text"),
            24,
            8,
            0,
        );
        host.add_comp(
            "TextSpan",
            ComponentUuid::from_canonical_name("bevy_text::text::TextSpan"),
            24,
            8,
            0,
        );
        host.add_comp(
            "BackgroundColor",
            ComponentUuid::from_canonical_name("bevy_ui::ui_node::BackgroundColor"),
            16,
            4,
            0,
        );
        host.add_comp(
            "BorderColor",
            ComponentUuid::from_canonical_name("bevy_ui::ui_node::BorderColor"),
            64,
            4,
            0,
        );
        host.add_comp(
            "EmSize",
            ComponentUuid::from_canonical_name("bevy_text::text::EmSize"),
            4,
            4,
            0,
        );
        let theme_h = host.add_comp(
            "ThemeContext",
            ComponentUuid::from_canonical_name("bevy_feathers::theme::ThemeContext"),
            4,
            1,
            0,
        );
        let act_h = host.add_comp(
            "Activate",
            ACTIVATE_EVENT_UUID,
            core::mem::size_of::<FfiEntity>(),
            core::mem::align_of::<FfiEntity>(),
            0,
        );

        let api = MockHost::api();
        let load_ctx = core::ptr::from_mut(host.as_mut()).cast::<BamLoadCtx>();
        let mut inst: *mut BamInstance = core::ptr::null_mut();
        let manifest = BamManifest::new(c"ui_feathers_alias_mod".as_ptr(), c"0.3.0".as_ptr());

        let status = unsafe {
            init_guest_instance(&api, load_ctx, &mut inst, &manifest, |instance, lctx| {
                build_guest_fn(instance, lctx, |app| {
                    let handles = app.ui_handles().unwrap();
                    assert_eq!(handles.theme_context, Some(theme_h));
                    assert_eq!(handles.activate, act_h);

                    app.system(c"insert_theme_default").build(move |ctx| {
                        let e = FfiEntity::from_bits(88);
                        assert_eq!(
                            ctx.commands().insert_default_theme_context(e, &handles),
                            FfiResult::OK
                        );
                    })
                })
            })
        };
        assert_eq!(status, FfiResult::OK);
        let cb = host.systems[0].callback;
        let ud = host.systems[0].user_data;
        let host_ptr = core::ptr::from_mut(host.as_mut());
        let mut sys_ctx = MockSysCtxState {
            host: host_ptr,
            iters: Vec::new(),
            cmds: Vec::new(),
            next_entity: 90,
            entity_data: HashMap::new(),
            children: HashMap::new(),
        };
        assert_eq!(
            unsafe { cb(core::ptr::from_mut(&mut sys_ctx).cast(), ud) },
            FfiResult::OK
        );
        assert_eq!(
            sys_ctx.cmds,
            vec![MockCmd::InsertDefault(FfiEntity::from_bits(88), theme_h)]
        );
        unsafe { shutdown_guest_instance(inst) };
    }

    // Part C: Spoofed/mismatched UUID on `bevy_ui_widgets::Activate` fails closed with `TYPE_MISMATCH`.
    {
        let mut host = MockHost::new();
        host.add_comp(
            "bevy_ui_widgets::Activate",
            ComponentUuid::from_canonical_name("bevy_ui_widgets::Activate"), // wrong UUID (size=0)
            0,
            1,
            0,
        );
        let api = MockHost::api();
        let load_ctx = core::ptr::from_mut(host.as_mut()).cast::<BamLoadCtx>();
        let mut inst: *mut BamInstance = core::ptr::null_mut();
        let manifest = BamManifest::new(c"ui_bad_uuid_mod".as_ptr(), c"0.3.0".as_ptr());

        let status = unsafe {
            init_guest_instance(&api, load_ctx, &mut inst, &manifest, |instance, lctx| {
                build_guest_fn(instance, lctx, |app| {
                    assert_eq!(
                        GuestUiHandles::resolve_activate(app.host()),
                        Err(FfiResult::TYPE_MISMATCH)
                    );
                    assert_eq!(
                        app.observe_activate("bad_act").build(|_t, _e, _c| {}).err(),
                        Some(FfiResult::TYPE_MISMATCH)
                    );
                    FfiResult::OK
                })
            })
        };
        assert_eq!(status, FfiResult::OK);
        unsafe { shutdown_guest_instance(inst) };
    }
}

/// Scenario: Verifies exact `Default` parity and constructor parity with `bamabi_bridges` /
/// Bevy `0.20` across `FfiRemSize`, `FfiEmSize`, `FfiUiTextStyle`, `FfiEditableTextConfig`,
/// `FfiInlineBox`, `FfiSelectedTab`, `FfiValueChange`, `FfiFeathersColorInput`,
/// `FfiFeathersNumberInput`, `FfiSpatialAudioSinkControl`, and `FfiGamepadButton::bit`.
#[test]
fn wire_defaults_and_constructors_match_bamabi_bridges_and_bevy_020() {
    // 1. Bevy 0.20 `DEFAULT_REM_SIZE_PX = 20.0` (`RemSize` & `EmSize`).
    assert_eq!(FfiRemSize::default(), FfiRemSize(20.0));
    assert_eq!(FfiEmSize::default(), FfiEmSize(20.0));

    // 2. `FfiUiTextStyle::default()` uses `FontSize::Rem(1.0)` (not `Px(20.0)`).
    let text_style = FfiUiTextStyle::default();
    assert_eq!(text_style.font_size, FfiFontSize::rem(1.0));
    assert_eq!(text_style.weight, 400);
    assert_eq!(text_style.color, FfiRgba::WHITE);

    // 3. `FfiEditableTextConfig::default()` matches `bamabi_bridges::ui`.
    let edit_cfg = FfiEditableTextConfig::default();
    assert_eq!(edit_cfg.cursor_margin, [0.2, 0.2]);
    assert_eq!(edit_cfg.has_visible_lines, FfiBool::TRUE);
    assert_eq!(edit_cfg.visible_lines, 1.0);
    assert_eq!(edit_cfg.interactive, FfiBool::TRUE);

    // 4. `FfiInlineBox::{in_flow, out_of_flow, default}`.
    let in_flow = FfiInlineBox::in_flow(24.0, 12.0);
    assert_eq!(in_flow.size, [24.0, 12.0]);
    assert_eq!(in_flow.kind, FfiInlineBox::KIND_IN_FLOW);
    let out_flow = FfiInlineBox::out_of_flow(8.0, 16.0);
    assert_eq!(out_flow.size, [8.0, 16.0]);
    assert_eq!(out_flow.kind, FfiInlineBox::KIND_OUT_OF_FLOW);
    assert_eq!(FfiInlineBox::default(), FfiInlineBox::out_of_flow(0.0, 0.0));

    // 5. `FfiSelectedTab::{from_option, to_option}`.
    let tab_ent = FfiEntity::from_bits(77);
    assert_eq!(FfiSelectedTab::from_option(None).to_option(), None);
    assert_eq!(
        FfiSelectedTab::from_option(Some(tab_ent)).to_option(),
        Some(tab_ent)
    );

    // 6. All 8 `FfiValueChange::from_*` constructors.
    let src = FfiEntity::from_bits(10);
    let val_ent = FfiEntity::from_bits(20);
    let vc_ent = FfiValueChange::from_entity(src, val_ent, true);
    assert_eq!(vc_ent.kind, FfiValueChange::KIND_ENTITY);
    assert_eq!(vc_ent.source, src);
    assert_eq!(vc_ent.entity_value, val_ent);
    assert!(vc_ent.is_final.get());

    let vc_opt = FfiValueChange::from_optional_entity(src, None, false);
    assert_eq!(vc_opt.kind, FfiValueChange::KIND_OPTIONAL_ENTITY);
    assert!(vc_opt.entity_value.is_null());
    assert!(!vc_opt.is_final.get());

    let vc_f32 = FfiValueChange::from_f32(src, 3.5, true);
    assert_eq!(vc_f32.kind, FfiValueChange::KIND_F32);
    assert_eq!(vc_f32.f32_value, 3.5);
    assert_eq!(vc_f32.f64_value, 3.5_f64);

    let vc_bool = FfiValueChange::from_bool(src, true, true);
    assert_eq!(vc_bool.kind, FfiValueChange::KIND_BOOL);
    assert!(vc_bool.bool_value.get());

    let vc_f64 = FfiValueChange::from_f64(src, 12.25, false);
    assert_eq!(vc_f64.kind, FfiValueChange::KIND_F64);
    assert_eq!(vc_f64.f64_value, 12.25);
    assert_eq!(vc_f64.f32_value, 12.25_f32);

    let vc_i32 = FfiValueChange::from_i32(src, -42, true);
    assert_eq!(vc_i32.kind, FfiValueChange::KIND_I32);
    assert_eq!(vc_i32.i64_value, -42);
    assert_eq!(vc_i32.f64_value, -42.0);
    assert_eq!(vc_i32.f32_value, -42.0);

    let vc_i64 = FfiValueChange::from_i64(src, 9001, true);
    assert_eq!(vc_i64.kind, FfiValueChange::KIND_I64);
    assert_eq!(vc_i64.i64_value, 9001);
    assert_eq!(vc_i64.f64_value, 9001.0);

    let rgba = FfiRgba::new(0.25, 0.5, 0.75, 1.0);
    let vc_col = FfiValueChange::from_color(src, rgba, true);
    assert_eq!(vc_col.kind, FfiValueChange::KIND_COLOR);
    assert_eq!(vc_col.color_value, rgba);

    // 7. `FfiFeathersColorInput` & `FfiFeathersNumberInput` defaults and builders.
    let color_in = FfiFeathersColorInput::default();
    assert_eq!(color_in.color, FfiRgba::WHITE);
    assert_eq!(color_in.opaque_color_percentage, 0.0);

    let num_def = FfiFeathersNumberInput::default();
    assert_eq!(num_def.soft_min, 0.0);
    assert_eq!(num_def.soft_max, 0.0);
    assert_eq!(num_def.hard_min, 0.0);
    assert_eq!(num_def.hard_max, 0.0);
    assert_eq!(num_def.step, 1.0);
    assert_eq!(num_def.precision, 2);
    assert_eq!(num_def.format, FfiFeathersNumberInput::FORMAT_F32);

    let num_f32 = FfiFeathersNumberInput::f32(12.5)
        .with_soft_limit(0.0, 100.0)
        .with_hard_limit(-50.0, 200.0);
    assert_eq!(num_f32.format, FfiFeathersNumberInput::FORMAT_F32);
    assert_eq!(num_f32.value_f64, 12.5);
    assert!(num_f32.has_soft_limit.get());
    assert_eq!((num_f32.soft_min, num_f32.soft_max), (0.0, 100.0));
    assert!(num_f32.has_hard_limit.get());
    assert_eq!((num_f32.hard_min, num_f32.hard_max), (-50.0, 200.0));

    let num_f64 = FfiFeathersNumberInput::f64(2.5);
    assert_eq!(num_f64.format, FfiFeathersNumberInput::FORMAT_F64);
    assert_eq!(num_f64.value_f64, 2.5);

    let num_i32 = FfiFeathersNumberInput::i32(-7);
    assert_eq!(num_i32.format, FfiFeathersNumberInput::FORMAT_I32);
    assert_eq!(num_i32.value_i64, -7);
    assert_eq!(num_i32.precision, 0);

    let num_i64 = FfiFeathersNumberInput::i64(42);
    assert_eq!(num_i64.format, FfiFeathersNumberInput::FORMAT_I64);
    assert_eq!(num_i64.value_i64, 42);
    assert_eq!(num_i64.precision, 0);

    // 8. `FfiSpatialAudioSinkControl` defaults (4m ear gap matching `SpatialListener::new(4.0)`)
    // and full command helper parity.
    let mut sink = FfiSpatialAudioSinkControl::default();
    assert_eq!(sink.left_ear_position, [-2.0, 0.0, 0.0]);
    assert_eq!(sink.right_ear_position, [2.0, 0.0, 0.0]);
    assert!(!sink.has_pending_command());

    sink.request_toggle_playback();
    assert_eq!(sink.playback_command, SINK_CMD_TOGGLE_PLAYBACK);
    sink.request_mute();
    assert_eq!(sink.mute_command, MUTE_CMD_MUTE);
    sink.request_unmute();
    assert_eq!(sink.mute_command, MUTE_CMD_UNMUTE);
    sink.request_toggle_mute();
    assert_eq!(sink.mute_command, MUTE_CMD_TOGGLE);
    sink.set_linear_volume(0.75);
    assert_eq!(sink.volume_mode, VOLUME_LINEAR);
    assert_eq!(sink.volume, 0.75);
    assert_ne!(sink.apply_flags & APPLY_VOLUME, 0);
    sink.set_decibel_volume(-6.0);
    assert_eq!(sink.volume_mode, VOLUME_DECIBELS);
    assert_eq!(sink.volume, -6.0);
    sink.set_ear_positions([-1.5, 0.0, 0.0], [1.5, 0.0, 0.0]);
    assert_eq!(sink.left_ear_position, [-1.5, 0.0, 0.0]);
    assert_eq!(sink.right_ear_position, [1.5, 0.0, 0.0]);
    assert_ne!(sink.apply_flags & APPLY_EAR_POSITIONS, 0);
    assert!(sink.has_pending_command());

    // 9. `FfiGamepadButton::bit` alias matches `bit_mask`.
    assert_eq!(FfiGamepadButton::SOUTH.bit(), 1u64 << 0);
    assert_eq!(FfiGamepadButton::DPAD_RIGHT.bit(), 1u64 << 18);
    assert_eq!(FfiGamepadButton(64).bit(), 0);
}

/// Regression: Verifies fail-closed edge cases across UI, hierarchy, and `read_removed`:
/// 1. `GuestUiHandles::resolve` fails closed with `TYPE_MISMATCH` when optional `ThemeContext`
///    has a spoofed/mismatched UUID (never silently collapsing to `None`).
/// 2. `GuestUiHandles::resolve_rem_size` resolves `RemSize` (and alias fallback).
/// 3. `trigger_activate` returns `NOT_FOUND` when `Activate` is unregistered, and both
///    `trigger_activate` / `trigger_activate_with` return `NULL_POINTER` on invalid commands.
/// 4. `QueryBuilder::read_children` and `GuestCommands::spawn_child` fail closed with
///    `TYPE_MISMATCH` when canonical `Children`/`ChildOf` has a mismatched UUID even if short
///    aliases `"Children"`/`"ChildOf"` are also present.
/// 5. `SystemContext::read_removed` / `read_removed_batch` preserve `TYPE_MISMATCH` when a user
///    query ALSO exists alongside `.removed::<T>()`.
#[test]
fn fail_closed_ui_theme_mismatch_unregistered_activate_hierarchy_and_removed_with_user_query() {
    // Part A: `GuestUiHandles::resolve` must propagate `TYPE_MISMATCH` on `ThemeContext`, and
    // `resolve_rem_size` resolves `RemSize`.
    {
        let mut host = MockHost::new();
        host.add_comp(
            "bevy_ui::widget::text::Text",
            ComponentUuid::from_canonical_name("bevy_ui::widget::text::Text"),
            24,
            8,
            0,
        );
        host.add_comp(
            "bevy_text::text::TextSpan",
            ComponentUuid::from_canonical_name("bevy_text::text::TextSpan"),
            24,
            8,
            0,
        );
        host.add_comp(
            "bevy_ui::ui_node::BackgroundColor",
            ComponentUuid::from_canonical_name("bevy_ui::ui_node::BackgroundColor"),
            16,
            4,
            0,
        );
        host.add_comp(
            "bevy_ui::ui_node::BorderColor",
            ComponentUuid::from_canonical_name("bevy_ui::ui_node::BorderColor"),
            64,
            4,
            0,
        );
        let rem_h = host.add_comp(
            "RemSize",
            ComponentUuid::from_canonical_name("bevy_text::text::RemSize"),
            4,
            4,
            BAM_STORAGE_FLAG_RESOURCE,
        );
        host.add_comp(
            "bevy_text::text::EmSize",
            ComponentUuid::from_canonical_name("bevy_text::text::EmSize"),
            4,
            4,
            0,
        );
        // Spoofed UUID on `ThemeContext` (size=4 instead of canonical 0-size bridged UUID)
        host.add_comp(
            "bevy_feathers::theme::ThemeContext",
            ComponentUuid::from_parts("bevy_feathers::theme::ThemeContext", 4, 1, 0),
            4,
            1,
            0,
        );
        host.add_comp(
            "bevy_ui_widgets::Activate",
            ACTIVATE_EVENT_UUID,
            core::mem::size_of::<FfiEntity>(),
            core::mem::align_of::<FfiEntity>(),
            0,
        );

        let api = MockHost::api();
        let load_ctx = core::ptr::from_mut(host.as_mut()).cast::<BamLoadCtx>();
        let mut inst: *mut BamInstance = core::ptr::null_mut();
        let manifest = BamManifest::new(c"theme_mismatch_mod".as_ptr(), c"0.3.0".as_ptr());

        let status = unsafe {
            init_guest_instance(&api, load_ctx, &mut inst, &manifest, |instance, lctx| {
                build_guest_fn(instance, lctx, |app| {
                    assert_eq!(GuestUiHandles::resolve_rem_size(app.host()), Ok(rem_h));
                    assert_eq!(app.ui_handles(), Err(FfiResult::TYPE_MISMATCH));
                    FfiResult::OK
                })
            })
        };
        assert_eq!(status, FfiResult::OK);
        unsafe { shutdown_guest_instance(inst) };
    }

    // Part B: `trigger_activate` with unregistered `Activate` -> `NOT_FOUND`, and invalid
    // `GuestCommands` -> `NULL_POINTER` (both `trigger_activate` and `trigger_activate_with`).
    {
        let mut host = MockHost::new();
        let api = MockHost::api();
        let load_ctx = core::ptr::from_mut(host.as_mut()).cast::<BamLoadCtx>();
        let mut inst: *mut BamInstance = core::ptr::null_mut();
        let manifest = BamManifest::new(c"unreg_activate_mod".as_ptr(), c"0.3.0".as_ptr());

        static CHECKED: AtomicBool = AtomicBool::new(false);
        let status = unsafe {
            init_guest_instance(&api, load_ctx, &mut inst, &manifest, |instance, lctx| {
                build_guest_fn(instance, lctx, |app| {
                    app.system(c"check_activate_errors").build(|ctx| {
                        // Valid commands + unregistered `Activate` -> `NOT_FOUND`.
                        assert_eq!(
                            ctx.commands().trigger_activate(FfiEntity::from_bits(5)),
                            Err(FfiResult::NOT_FOUND)
                        );
                        // Invalid/null commands -> `NULL_POINTER` even for null target.
                        let mut empty_cmds = GuestCommands::from_validated(
                            bamabi_guest::raw::ValidatedCommands::empty(ctx.host()),
                        );
                        assert_eq!(
                            empty_cmds.trigger_activate(FfiEntity::NULL),
                            Err(FfiResult::NULL_POINTER)
                        );
                        assert_eq!(
                            empty_cmds.trigger_activate(FfiEntity::from_bits(5)),
                            Err(FfiResult::NULL_POINTER)
                        );
                        let fake_handles = GuestUiHandles {
                            text: BamComponent::from_raw(1),
                            text_span: BamComponent::from_raw(2),
                            background_color: BamComponent::from_raw(3),
                            border_color: BamComponent::from_raw(4),
                            em_size: BamComponent::from_raw(5),
                            theme_context: None,
                            activate: BamComponent::from_raw(6),
                        };
                        assert_eq!(
                            empty_cmds.trigger_activate_with(&fake_handles, FfiEntity::NULL),
                            Err(FfiResult::NULL_POINTER)
                        );
                        assert_eq!(
                            empty_cmds.spawn_child(FfiEntity::from_bits(5), &[]),
                            Err(FfiResult::NULL_POINTER)
                        );
                        CHECKED.store(true, Ordering::SeqCst);
                        Ok(())
                    })
                })
            })
        };
        assert_eq!(status, FfiResult::OK);
        let cb = host.systems[0].callback;
        let ud = host.systems[0].user_data;
        let host_ptr = core::ptr::from_mut(host.as_mut());
        let mut sys_ctx = MockSysCtxState {
            host: host_ptr,
            iters: Vec::new(),
            cmds: Vec::new(),
            next_entity: 10,
            entity_data: HashMap::new(),
            children: HashMap::new(),
        };
        assert_eq!(
            unsafe { cb(core::ptr::from_mut(&mut sys_ctx).cast(), ud) },
            FfiResult::OK
        );
        assert!(CHECKED.load(Ordering::SeqCst));
        assert!(sys_ctx.cmds.is_empty());
        unsafe { shutdown_guest_instance(inst) };
    }

    // Part C: Spoofed canonical `Children` / `ChildOf` UUID fails closed with `TYPE_MISMATCH`
    // even when short aliases `"Children"` / `"ChildOf"` are also present.
    {
        let mut host = MockHost::new();
        host.comps.clear();
        host.add_comp(
            "bevy_ecs::hierarchy::Children",
            ComponentUuid::from_parts("bevy_ecs::hierarchy::Children", 99, 1, 0), // wrong UUID
            99,
            1,
            0,
        );
        host.add_comp(
            "Children",
            ComponentUuid::from_canonical_name("bevy_ecs::hierarchy::Children"),
            0,
            1,
            0,
        );
        host.add_comp(
            "bevy_ecs::hierarchy::ChildOf",
            ComponentUuid::from_parts("bevy_ecs::hierarchy::ChildOf", 16, 8, 0), // wrong UUID
            16,
            8,
            0,
        );
        host.add_comp("ChildOf", FfiChildOf::UUID, 8, 8, 0);

        let api = MockHost::api();
        let load_ctx = core::ptr::from_mut(host.as_mut()).cast::<BamLoadCtx>();
        let mut inst: *mut BamInstance = core::ptr::null_mut();
        let manifest = BamManifest::new(c"hierarchy_spoof_mod".as_ptr(), c"0.3.0".as_ptr());

        static SPAWN_CHECKED: AtomicBool = AtomicBool::new(false);
        let status = unsafe {
            init_guest_instance(&api, load_ctx, &mut inst, &manifest, |instance, lctx| {
                build_guest_fn(instance, lctx, |app| {
                    assert_eq!(
                        app.system(c"spoofed_children")
                            .query(|q| q.read_children())
                            .build(|_ctx| {}),
                        FfiResult::TYPE_MISMATCH
                    );
                    app.system(c"spoofed_child_of").build(|ctx| {
                        assert_eq!(
                            ctx.commands().spawn_child(FfiEntity::from_bits(1), &[]),
                            Err(FfiResult::TYPE_MISMATCH)
                        );
                        SPAWN_CHECKED.store(true, Ordering::SeqCst);
                        Ok(())
                    })
                })
            })
        };
        assert_eq!(status, FfiResult::OK);
        let cb = host.systems[0].callback;
        let ud = host.systems[0].user_data;
        let host_ptr = core::ptr::from_mut(host.as_mut());
        let mut sys_ctx = MockSysCtxState {
            host: host_ptr,
            iters: Vec::new(),
            cmds: Vec::new(),
            next_entity: 10,
            entity_data: HashMap::new(),
            children: HashMap::new(),
        };
        assert_eq!(
            unsafe { cb(core::ptr::from_mut(&mut sys_ctx).cast(), ud) },
            FfiResult::OK
        );
        assert!(SPAWN_CHECKED.load(Ordering::SeqCst));
        assert!(sys_ctx.cmds.is_empty());
        unsafe { shutdown_guest_instance(inst) };
    }

    // Part D: `SystemContext::read_removed` & `read_removed_batch` preserve `TYPE_MISMATCH` even
    // when a user query ALSO exists alongside `.removed::<T>()` (and `q0` has fewer terms than
    // `term_index`, which previously clobbered `TYPE_MISMATCH` with `OUT_OF_RANGE`).
    {
        let mut host = MockHost::new();
        let api = MockHost::api();
        let load_ctx = core::ptr::from_mut(host.as_mut()).cast::<BamLoadCtx>();
        let mut inst: *mut BamInstance = core::ptr::null_mut();
        let manifest = BamManifest::new(c"removed_with_q0_mod".as_ptr(), c"0.3.0".as_ptr());

        static CHECKED: AtomicBool = AtomicBool::new(false);
        let status = unsafe {
            init_guest_instance(&api, load_ctx, &mut inst, &manifest, |instance, lctx| {
                build_guest_fn(instance, lctx, |app| {
                    app.system(c"q0_plus_removed")
                        .query_typed::<(&FfiChildOf,)>() // 1 term at index 0
                        .removed::<FfiChildOf>() // res term 0
                        .removed::<FfiCamera2d>() // res term 1
                        .build(|ctx| {
                            // Wrong type at res term 1 (out of range for q0!) -> must stay TYPE_MISMATCH.
                            assert_eq!(
                                ctx.read_removed::<FfiChildOf>(1),
                                Err(FfiResult::TYPE_MISMATCH)
                            );
                            let mut buf = [FfiEntity::NULL; 4];
                            assert_eq!(
                                ctx.read_removed_batch::<FfiChildOf>(1, &mut buf),
                                Err(FfiResult::TYPE_MISMATCH)
                            );
                            // Correct type at res term 1 succeeds!
                            assert_eq!(
                                ctx.read_removed::<FfiCamera2d>(1),
                                Ok(vec![FfiEntity::from_bits(77)])
                            );
                            CHECKED.store(true, Ordering::SeqCst);
                            Ok(())
                        })
                })
            })
        };
        assert_eq!(status, FfiResult::OK);
        let q0_terms = host.systems[0].queries[0].clone();
        let res_terms = host.systems[0].queries[1].clone();
        let cb = host.systems[0].callback;
        let ud = host.systems[0].user_data;
        let host_ptr = core::ptr::from_mut(host.as_mut());
        let q0_iter = Box::new(MockIterState {
            host: host_ptr,
            terms: q0_terms,
            entities: Vec::new(),
            columns: HashMap::new(),
            removed: HashMap::new(),
            resources: HashMap::new(),
            messages: HashMap::new(),
            chunks: Vec::new(),
            cursor: 0,
            active_chunk: None,
        });
        let mut removed_map = HashMap::new();
        removed_map.insert(1usize, vec![FfiEntity::from_bits(77)]);
        let res_iter = Box::new(MockIterState {
            host: host_ptr,
            terms: res_terms,
            entities: Vec::new(),
            columns: HashMap::new(),
            removed: removed_map,
            resources: HashMap::new(),
            messages: HashMap::new(),
            chunks: Vec::new(),
            cursor: 0,
            active_chunk: None,
        });
        let mut sys_ctx = MockSysCtxState {
            host: host_ptr,
            iters: vec![q0_iter, res_iter],
            cmds: Vec::new(),
            next_entity: 10,
            entity_data: HashMap::new(),
            children: HashMap::new(),
        };
        assert_eq!(
            unsafe { cb(core::ptr::from_mut(&mut sys_ctx).cast(), ud) },
            FfiResult::OK
        );
        assert!(CHECKED.load(Ordering::SeqCst));
        unsafe { shutdown_guest_instance(inst) };
    }
}

/// Regression: Verifies that every camera, lighting, controller, and Solari wire `Default`
/// implementation in `bamabi_guest::bridges::camera_light` matches Bevy `0.20.1` and
/// `bamabi_bridges::camera_light` field-for-field.
#[test]
fn camera_light_wire_defaults_match_bevy_020_bridges() {
    use bamabi_guest::bridges::camera_light::{
        CLEAR_COLOR_DEFAULT, DEPTH_LOAD_OP_CLEAR, FfiCamera2d, FfiCamera3d, FfiDirectionalLight,
        FfiPanOrbitCamera, FfiPointLight, FfiSolariLighting, FfiSpotLight, ORBIT_CONSTRAINT_FIXED,
        TONEMAPPING_TONY_MC_MAPFACE, TRANSMISSION_QUALITY_MEDIUM,
    };

    let cam2d = FfiCamera2d::default();
    assert_eq!(cam2d.order, 0);
    assert_eq!(cam2d.clear_color, [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(cam2d.is_active, FfiBool::TRUE);
    assert_eq!(cam2d.clear_color_mode, CLEAR_COLOR_DEFAULT);
    assert_eq!(cam2d.invert_culling, FfiBool::FALSE);
    assert_eq!(cam2d.hdr, FfiBool::FALSE);

    let cam3d = FfiCamera3d::default();
    assert_eq!(cam3d.order, 0);
    assert_eq!(cam3d.clear_color, [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(cam3d.depth_clear_value.to_bits(), 0.0f32.to_bits());
    assert_eq!(cam3d.depth_texture_usages, 16);
    assert_eq!(cam3d.exposure_ev100.to_bits(), 9.7f32.to_bits());
    assert_eq!(cam3d.transmission_steps, 1);
    assert_eq!(cam3d.is_active, FfiBool::TRUE);
    assert_eq!(cam3d.clear_color_mode, CLEAR_COLOR_DEFAULT);
    assert_eq!(cam3d.invert_culling, FfiBool::FALSE);
    assert_eq!(cam3d.hdr, FfiBool::FALSE);
    assert_eq!(cam3d.depth_load_op, DEPTH_LOAD_OP_CLEAR);
    assert_eq!(cam3d.tonemapping, TONEMAPPING_TONY_MC_MAPFACE);
    assert_eq!(cam3d.deband_dither, FfiBool::FALSE);
    assert_eq!(cam3d.transmission_enabled, FfiBool::FALSE);
    assert_eq!(cam3d.transmission_quality, TRANSMISSION_QUALITY_MEDIUM);
    assert_eq!(cam3d.reserved0, [0; 3]);

    let point = FfiPointLight::default();
    assert_eq!(point.color, [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(point.intensity.to_bits(), 1_000_000.0f32.to_bits());
    assert_eq!(point.range.to_bits(), 20.0f32.to_bits());
    assert_eq!(point.radius.to_bits(), 0.0f32.to_bits());
    assert_eq!(point.shadow_depth_bias.to_bits(), 0.08f32.to_bits());
    assert_eq!(point.shadow_normal_bias.to_bits(), 0.6f32.to_bits());
    assert_eq!(point.shadow_map_near_z.to_bits(), 0.1f32.to_bits());
    assert_eq!(point.shadow_maps_enabled, FfiBool::FALSE);
    assert_eq!(point.contact_shadows_enabled, FfiBool::FALSE);
    assert_eq!(point.affects_lightmapped_mesh_diffuse, FfiBool::TRUE);
    assert_eq!(point.reserved0, 0);

    let spot = FfiSpotLight::default();
    assert_eq!(spot.color, [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(spot.intensity.to_bits(), 1_000_000.0f32.to_bits());
    assert_eq!(spot.range.to_bits(), 20.0f32.to_bits());
    assert_eq!(spot.radius.to_bits(), 0.0f32.to_bits());
    assert_eq!(spot.shadow_depth_bias.to_bits(), 0.02f32.to_bits());
    assert_eq!(spot.shadow_normal_bias.to_bits(), 1.8f32.to_bits());
    assert_eq!(spot.shadow_map_near_z.to_bits(), 0.1f32.to_bits());
    assert_eq!(
        spot.outer_angle.to_bits(),
        core::f32::consts::FRAC_PI_4.to_bits()
    );
    assert_eq!(spot.inner_angle.to_bits(), 0.0f32.to_bits());
    assert_eq!(spot.shadow_maps_enabled, FfiBool::FALSE);
    assert_eq!(spot.contact_shadows_enabled, FfiBool::FALSE);
    assert_eq!(spot.affects_lightmapped_mesh_diffuse, FfiBool::TRUE);
    assert_eq!(spot.reserved0, 0);

    let dir = FfiDirectionalLight::default();
    assert_eq!(dir.color, [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(dir.illuminance.to_bits(), 10_000.0f32.to_bits());
    assert_eq!(dir.shadow_depth_bias.to_bits(), 0.02f32.to_bits());
    assert_eq!(dir.shadow_normal_bias.to_bits(), 1.8f32.to_bits());
    assert_eq!(dir.shadow_maps_enabled, FfiBool::FALSE);
    assert_eq!(dir.contact_shadows_enabled, FfiBool::FALSE);
    assert_eq!(dir.affects_lightmapped_mesh_diffuse, FfiBool::TRUE);
    assert_eq!(dir.reserved0, 0);

    let pan = FfiPanOrbitCamera::default();
    assert_eq!(pan.last_anchor_depth.to_bits(), (-2.0f64).to_bits());
    assert_eq!(pan.zoom_min_size_per_pixel.to_bits(), 1e-6f64.to_bits());
    assert_eq!(pan.zoom_max_size_per_pixel.to_bits(), 1e27f64.to_bits());
    assert_eq!(pan.orbit_up, [0.0, 1.0, 0.0]);
    assert_eq!(pan.momentum_pan_friction.to_bits(), 0.2f64.to_bits());
    assert_eq!(pan.momentum_orbit_friction.to_bits(), 0.2f64.to_bits());
    assert_eq!(
        pan.smoothing_pan_secs.to_bits(),
        core::time::Duration::from_millis(10)
            .as_secs_f32()
            .to_bits()
    );
    assert_eq!(
        pan.smoothing_orbit_secs.to_bits(),
        core::time::Duration::from_millis(30)
            .as_secs_f32()
            .to_bits()
    );
    assert_eq!(
        pan.smoothing_zoom_secs.to_bits(),
        core::time::Duration::from_millis(60)
            .as_secs_f32()
            .to_bits()
    );
    assert_eq!(pan.sensitivity_orbit, [1.0, 1.0]);
    assert_eq!(pan.sensitivity_zoom.to_bits(), 1.0f32.to_bits());
    assert_eq!(
        pan.momentum_init_pan_secs.to_bits(),
        core::time::Duration::from_millis(40)
            .as_secs_f32()
            .to_bits()
    );
    assert_eq!(
        pan.momentum_init_orbit_secs.to_bits(),
        core::time::Duration::from_millis(60)
            .as_secs_f32()
            .to_bits()
    );
    assert_eq!(
        pan.input_debounce_secs.to_bits(),
        core::time::Duration::from_millis(80)
            .as_secs_f32()
            .to_bits()
    );
    assert_eq!(pan.perspective_near_clip_min.to_bits(), 1e-9f32.to_bits());
    assert_eq!(
        pan.perspective_near_clip_max.to_bits(),
        f32::INFINITY.to_bits()
    );
    assert_eq!(
        pan.perspective_near_clip_multiplier.to_bits(),
        0.05f32.to_bits()
    );
    assert_eq!(
        pan.ortho_scale_to_near_clip.to_bits(),
        1_000_000.0f32.to_bits()
    );
    assert_eq!(pan.ortho_near_clip_min.to_bits(), 1.0f32.to_bits());
    assert_eq!(pan.ortho_near_clip_max.to_bits(), 1_000_000.0f32.to_bits());
    assert_eq!(pan.ortho_far_clip_multiplier.to_bits(), 1.0f32.to_bits());
    assert_eq!(pan.enabled_pan, FfiBool::TRUE);
    assert_eq!(pan.enabled_orbit, FfiBool::TRUE);
    assert_eq!(pan.enabled_zoom, FfiBool::TRUE);
    assert_eq!(pan.orbit_constraint_mode, ORBIT_CONSTRAINT_FIXED);
    assert_eq!(pan.orbit_can_pass_tdc, FfiBool::FALSE);
    assert_eq!(pan.zoom_through_objects, FfiBool::FALSE);
    assert_eq!(pan.momentum_pan_damping, 160);
    assert_eq!(pan.momentum_orbit_damping, 160);

    let solari = FfiSolariLighting::default();
    assert_eq!(solari.confidence_weight_cap.to_bits(), 8.0f32.to_bits());
    assert_eq!(solari.primary_di_samples, 8);
    assert_eq!(solari.secondary_di_samples, 4);
    assert_eq!(solari.max_bounces, 3);
    assert_eq!(
        solari.world_cache_max_temporal_samples.to_bits(),
        32.0f32.to_bits()
    );
    assert_eq!(solari.world_cache_direct_light_sample_count, 32);
    assert_eq!(
        solari.world_cache_max_gi_ray_distance.to_bits(),
        50.0f32.to_bits()
    );
    assert_eq!(solari.world_cache_cell_updates_soft_target, 40_000);
    assert_eq!(
        solari.world_cache_position_base_cell_size.to_bits(),
        0.15f32.to_bits()
    );
    assert_eq!(
        solari.world_cache_position_lod_scale.to_bits(),
        15.0f32.to_bits()
    );
    assert_eq!(solari.restir, FfiBool::FALSE);
    assert_eq!(solari.reset, FfiBool::TRUE);
    assert_eq!(solari.reserved0, [0; 2]);
}
