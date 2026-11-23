//! Bevy `0.20` `ModStateSlab` wire types (`bamabi_bridges::state`), 64-byte `bql_lsp`
//! telemetry frame (`bamabi_reflect::telemetry`), plus host-opt-in `bevy_window`, `AppExit`,
//! `AssetEvent`, `StateTransitionEvent`, and `bevy_gizmos` wire projection types.

use bamabi_sys::{FfiAssetHandle, FfiBool, FfiEntity};

use super::std_types::FfiColor;

// ============================================================================
// `bamabi_bridges::state` — Unbounded Dynamic `ModStateSlab` Wire Types
// ============================================================================

/// Wire discriminant sentinel indicating that a conditional state (`SubStates` or
/// `ComputedStates`) does not currently exist in the `World`.
pub const STATE_ABSENT_DISCRIMINANT: u32 = u32::MAX;

/// [`ModStateEntry::flags`] bit: transitions may be requested (`FreelyMutable` or `SubState`).
pub const MOD_STATE_FLAG_MUTABLE: u32 = 1 << 0;
/// [`ModStateEntry::flags`] bit: `State<S>` resource currently exists in the `World`.
pub const MOD_STATE_FLAG_ACTIVE: u32 = 1 << 1;
/// [`ModStateEntry::flags`] bit: state is a conditional `SubStates`.
pub const MOD_STATE_FLAG_SUB_STATE: u32 = 1 << 2;
/// [`ModStateEntry::flags`] bit: state is a read-only `ComputedStates`.
pub const MOD_STATE_FLAG_COMPUTED: u32 = 1 << 3;

/// Generation-indexed handle `(slot_index, generation)` identifying an exposed state entry inside
/// host `ModStateSlab` (8 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ModStateHandle {
    /// Zero-based index inside `ModStateSlab`.
    pub slot_index: u32,
    /// Non-zero generation counter invalidating stale handles after slot release/reuse.
    pub generation: u32,
}

impl_wire_abi_pod!(
    ModStateHandle,
    size = 8,
    align = 4,
    schema = [
        slot_index: u32 => "u32",
        generation: u32 => "u32",
    ]
);

impl ModStateHandle {
    /// Sentinel invalid handle (`slot_index = u32::MAX`, `generation = 0`).
    pub const INVALID: Self = Self {
        slot_index: u32::MAX,
        generation: 0,
    };

    /// Constructs a handle from raw `(slot_index, generation)`.
    #[inline]
    pub const fn new(slot_index: u32, generation: u32) -> Self {
        Self {
            slot_index,
            generation,
        }
    }

    /// Returns `true` if this handle carries a non-zero generation and valid slot index.
    #[inline]
    pub const fn is_valid(self) -> bool {
        self.generation != 0 && self.slot_index != u32::MAX
    }
}

/// Read-only zero-copy `#[repr(C)]` component mirroring the live state of any exposed `States`,
/// `SubStates`, or `ComputedStates` slot inside host `ModStateSlab`
/// (`"bevy_state::ModStateEntry"`, 16 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ModStateEntry {
    /// Zero-based slot index inside `ModStateSlab`.
    pub state_slot: u32,
    /// Non-zero generation counter of the exposed state slot.
    pub state_generation: u32,
    /// Active state discriminant, or [`STATE_ABSENT_DISCRIMINANT`] (`u32::MAX`) when absent.
    pub current_discriminant: u32,
    /// Bitmask of `MOD_STATE_FLAG_*` (`MUTABLE`, `ACTIVE`, `SUB_STATE`, `COMPUTED`).
    pub flags: u32,
}

impl_wire_abi_pod!(
    ModStateEntry,
    size = 16,
    align = 4,
    schema = [
        state_slot: u32 => "u32",
        state_generation: u32 => "u32",
        current_discriminant: u32 => "u32",
        flags: u32 => "u32",
    ]
);

impl ModStateEntry {
    /// Returns the generation-indexed [`ModStateHandle`] for this entry.
    #[inline]
    pub const fn state_handle(self) -> ModStateHandle {
        ModStateHandle::new(self.state_slot, self.state_generation)
    }

    /// Returns `true` if `State<S>` currently exists in the `World`.
    #[inline]
    pub const fn is_active(self) -> bool {
        (self.flags & MOD_STATE_FLAG_ACTIVE) != 0
    }

    /// Returns `true` if transitions are permitted on this state (`false` for `ComputedStates`).
    #[inline]
    pub const fn is_mutable(self) -> bool {
        (self.flags & MOD_STATE_FLAG_MUTABLE) != 0
    }

    /// Returns `true` if this state is a conditional `SubStates`.
    #[inline]
    pub const fn is_sub_state(self) -> bool {
        (self.flags & MOD_STATE_FLAG_SUB_STATE) != 0
    }

    /// Returns `true` if this state is a read-only `ComputedStates`.
    #[inline]
    pub const fn is_computed(self) -> bool {
        (self.flags & MOD_STATE_FLAG_COMPUTED) != 0
    }

    /// Returns the active discriminant, or `None` when absent/inactive.
    #[inline]
    pub const fn active_discriminant(self) -> Option<u32> {
        if self.is_active() && self.current_discriminant != STATE_ABSENT_DISCRIMINANT {
            Some(self.current_discriminant)
        } else {
            None
        }
    }
}

/// Zero-padding `#[repr(C)]` message requesting a state transition on any mutable state
/// (`FreelyMutableState` or `SubStates`) in host `ModStateSlab`
/// (`"bevy_state::ModStateTransitionRequest"`, 16 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ModStateTransitionRequest {
    /// Zero-based slot index inside `ModStateSlab`.
    pub state_slot: u32,
    /// Non-zero generation counter of the target state slot.
    pub state_generation: u32,
    /// Target variant discriminant to transition to via `NextState::set_if_different`.
    pub target_discriminant: u32,
    /// Explicit zero-reserved alignment bytes (`0`).
    pub reserved0: u32,
}

impl_wire_abi_pod!(
    ModStateTransitionRequest,
    size = 16,
    align = 4,
    schema = [
        state_slot: u32 => "u32",
        state_generation: u32 => "u32",
        target_discriminant: u32 => "u32",
        reserved0: u32 => "u32",
    ]
);

impl ModStateTransitionRequest {
    /// Constructs a transition request for `(state, target_discriminant)`.
    #[inline]
    pub const fn new(state: ModStateHandle, target_discriminant: u32) -> Self {
        Self {
            state_slot: state.slot_index,
            state_generation: state.generation,
            target_discriminant,
            reserved0: 0,
        }
    }

    /// Returns the generation-indexed [`ModStateHandle`] targeted by this request.
    #[inline]
    pub const fn state_handle(self) -> ModStateHandle {
        ModStateHandle::new(self.state_slot, self.state_generation)
    }
}

/// Dynamic, zero-padding `#[repr(C)]` component attaching Bevy's `DespawnOnExit<S>(variant)` for
/// any state in host `ModStateSlab` (`"bevy_state::ModDespawnOnExit"`, 16 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ModDespawnOnExit {
    /// Zero-based slot index inside `ModStateSlab`.
    pub state_slot: u32,
    /// Non-zero generation counter of the target state slot.
    pub state_generation: u32,
    /// Variant discriminant on whose exit this entity is despawned.
    pub discriminant: u32,
    /// Explicit zero-reserved alignment bytes (`0`).
    pub reserved0: u32,
}

impl_wire_abi_pod!(
    ModDespawnOnExit,
    size = 16,
    align = 4,
    schema = [
        state_slot: u32 => "u32",
        state_generation: u32 => "u32",
        discriminant: u32 => "u32",
        reserved0: u32 => "u32",
    ]
);

impl ModDespawnOnExit {
    /// Constructs a [`ModDespawnOnExit`] marker for `(state, discriminant)`.
    #[inline]
    pub const fn new(state: ModStateHandle, discriminant: u32) -> Self {
        Self {
            state_slot: state.slot_index,
            state_generation: state.generation,
            discriminant,
            reserved0: 0,
        }
    }

    /// Returns the generation-indexed [`ModStateHandle`] encoded in this marker.
    #[inline]
    pub const fn state_handle(self) -> ModStateHandle {
        ModStateHandle::new(self.state_slot, self.state_generation)
    }
}

/// Dynamic, zero-padding `#[repr(C)]` component attaching Bevy's `DespawnOnEnter<S>(variant)` for
/// any state in host `ModStateSlab` (`"bevy_state::ModDespawnOnEnter"`, 16 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ModDespawnOnEnter {
    /// Zero-based slot index inside `ModStateSlab`.
    pub state_slot: u32,
    /// Non-zero generation counter of the target state slot.
    pub state_generation: u32,
    /// Variant discriminant on whose entry this entity is despawned.
    pub discriminant: u32,
    /// Explicit zero-reserved alignment bytes (`0`).
    pub reserved0: u32,
}

impl_wire_abi_pod!(
    ModDespawnOnEnter,
    size = 16,
    align = 4,
    schema = [
        state_slot: u32 => "u32",
        state_generation: u32 => "u32",
        discriminant: u32 => "u32",
        reserved0: u32 => "u32",
    ]
);

impl ModDespawnOnEnter {
    /// Constructs a [`ModDespawnOnEnter`] marker for `(state, discriminant)`.
    #[inline]
    pub const fn new(state: ModStateHandle, discriminant: u32) -> Self {
        Self {
            state_slot: state.slot_index,
            state_generation: state.generation,
            discriminant,
            reserved0: 0,
        }
    }

    /// Returns the generation-indexed [`ModStateHandle`] encoded in this marker.
    #[inline]
    pub const fn state_handle(self) -> ModStateHandle {
        ModStateHandle::new(self.state_slot, self.state_generation)
    }
}

// ============================================================================
// `bamabi_reflect::telemetry` — Fixed 64-Byte Telemetry Frame (from `bamabi_sys`)
// ============================================================================

pub use bamabi_sys::{
    SYSTEM_TELEMETRY_DIGEST_SIZE, SYSTEM_TELEMETRY_DIGEST_VERSION, SYSTEM_TELEMETRY_MAGIC,
    SystemTelemetryDigest64, TELEMETRY_FLAG_BUDGET_BREACHED, TELEMETRY_FLAG_FAULTED,
    TELEMETRY_FLAG_QUARANTINED, TELEMETRY_FLAG_SUMMARY_TICK_ACTIVE, TelemetryFrameError,
    TelemetrySliceError, fnv1a_64_hash_system,
};

// ============================================================================
// Host-Opt-In Convention Wire Types (Window, AppExit, AssetEvent, Gizmos)
// ============================================================================

/// Fixed-layout `#[repr(transparent)]` window display mode matching `bevy_window::WindowMode`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfiWindowMode(pub u8);

impl_wire_abi_pod!(FfiWindowMode, size = 1, align = 1, fields = [u8]);

#[allow(missing_docs)]
impl FfiWindowMode {
    pub const WINDOWED: Self = Self(0);
    pub const BORDERLESS_FULLSCREEN: Self = Self(1);
    pub const FULLSCREEN: Self = Self(2);
}

/// Host-opt-in `#[repr(C)]` projection of `bevy_window::Window` geometry and focus state.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiWindow {
    /// Logical width in DIPs.
    pub width: f32,
    /// Logical height in DIPs.
    pub height: f32,
    /// Backing framebuffer physical width in pixels.
    pub physical_width: u32,
    /// Backing framebuffer physical height in pixels.
    pub physical_height: u32,
    /// HiDPI scale factor (`physical / logical`).
    pub scale_factor: f32,
    /// Last known logical cursor position `[x, y]` inside the window (active when `has_cursor_position`).
    pub cursor_position: [f32; 2],
    /// Window display mode ([`FfiWindowMode`]).
    pub mode: FfiWindowMode,
    /// Whether the window currently has OS input focus.
    pub focused: FfiBool,
    /// Whether the cursor is visible over this window.
    pub cursor_visible: FfiBool,
    /// Whether `cursor_position` holds a valid in-window coordinate.
    pub has_cursor_position: FfiBool,
}

impl_wire_abi_pod!(
    FfiWindow,
    size = 32,
    align = 4,
    schema = [
        width: f32 => "f32",
        height: f32 => "f32",
        physical_width: u32 => "u32",
        physical_height: u32 => "u32",
        scale_factor: f32 => "f32",
        cursor_position: [f32; 2] => "[f32;2]",
        mode: FfiWindowMode => "FfiWindowMode",
        focused: FfiBool => "FfiBool",
        cursor_visible: FfiBool => "FfiBool",
        has_cursor_position: FfiBool => "FfiBool",
    ]
);

impl Default for FfiWindow {
    #[inline]
    fn default() -> Self {
        Self {
            width: 1280.0,
            height: 720.0,
            physical_width: 1280,
            physical_height: 720,
            scale_factor: 1.0,
            cursor_position: [0.0, 0.0],
            mode: FfiWindowMode::WINDOWED,
            focused: FfiBool::TRUE,
            cursor_visible: FfiBool::TRUE,
            has_cursor_position: FfiBool::FALSE,
        }
    }
}

impl FfiWindow {
    /// Returns the logical cursor position `[x, y]` if inside the window.
    #[inline]
    pub const fn cursor_position(&self) -> Option<[f32; 2]> {
        if self.has_cursor_position.get() {
            Some(self.cursor_position)
        } else {
            None
        }
    }
}

/// Host-opt-in `#[repr(C)]` projection of `bevy_window::WindowResized`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiWindowResized {
    /// Entity ID of the resized window.
    pub window: FfiEntity,
    /// New logical width.
    pub width: f32,
    /// New logical height.
    pub height: f32,
}

impl_wire_abi_pod!(
    FfiWindowResized,
    size = 16,
    align = core::mem::align_of::<u64>(),
    schema = [
        window: FfiEntity => "FfiEntity",
        width: f32 => "f32",
        height: f32 => "f32",
    ]
);

/// Host-opt-in `#[repr(C)]` projection of `bevy_window::WindowCloseRequested`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfiWindowCloseRequested {
    /// Entity ID of the window requested to close.
    pub window: FfiEntity,
}

impl_wire_abi_pod!(
    FfiWindowCloseRequested,
    size = 8,
    align = core::mem::align_of::<u64>(),
    schema = [window: FfiEntity => "FfiEntity"]
);

/// Host-opt-in `#[repr(C)]` projection of `bevy_app::AppExit`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfiAppExit {
    /// `0` = `AppExit::Success`; non-zero = `AppExit::Error(NonZeroU8)`.
    pub code: u8,
}

impl_wire_abi_pod!(
    FfiAppExit,
    size = 1,
    align = 1,
    schema = [code: u8 => "u8"]
);

impl FfiAppExit {
    /// Successful exit (`code = 0`).
    pub const SUCCESS: Self = Self { code: 0 };

    /// Constructs a non-zero error exit (`0` is clamped to `1`).
    #[inline]
    pub const fn error(code: u8) -> Self {
        Self {
            code: if code == 0 { 1 } else { code },
        }
    }

    /// Returns `true` if this exit represents `AppExit::Success`.
    #[inline]
    pub const fn is_success(self) -> bool {
        self.code == 0
    }
}

/// Fixed-layout `#[repr(transparent)]` kind tag matching `bevy_asset::AssetEvent<A>`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfiAssetEventKind(pub u8);

impl_wire_abi_pod!(FfiAssetEventKind, size = 1, align = 1, fields = [u8]);

#[allow(missing_docs)]
impl FfiAssetEventKind {
    pub const ADDED: Self = Self(0);
    pub const MODIFIED: Self = Self(1);
    pub const REMOVED: Self = Self(2);
    pub const UNUSED: Self = Self(3);
    pub const LOADED_WITH_DEPENDENCIES: Self = Self(4);
}

/// Host-opt-in `#[repr(C)]` projection of `bevy_asset::AssetEvent<A>`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfiAssetEvent {
    /// Affected asset handle.
    pub handle: FfiAssetHandle,
    /// Lifecycle event kind ([`FfiAssetEventKind`]).
    pub kind: FfiAssetEventKind,
    /// Explicit zero-reserved alignment bytes.
    pub reserved0: [u8; 3],
}

impl_wire_abi_pod!(
    FfiAssetEvent,
    size = 36,
    align = 4,
    schema = [
        handle: FfiAssetHandle => "FfiAssetHandle",
        kind: FfiAssetEventKind => "FfiAssetEventKind",
        reserved0: [u8; 3] => "[u8;3]",
    ]
);

/// Host-opt-in `#[repr(C)]` projection of Bevy `0.20`'s `StateTransitionEvent<S>`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfiStateTransitionEvent {
    /// Exposed state slot index.
    pub state_slot: u32,
    /// Previous state discriminant (valid when `has_exited.get()`).
    pub exited: u32,
    /// New state discriminant (valid when `has_entered.get()`).
    pub entered: u32,
    /// Whether `exited` is `Some(...)`.
    pub has_exited: FfiBool,
    /// Whether `entered` is `Some(...)`.
    pub has_entered: FfiBool,
    /// Bevy `0.20` `allow_same_state_transitions` flag.
    pub allow_same_state_transitions: FfiBool,
    /// Explicit zero-reserved alignment byte.
    pub reserved0: u8,
}

impl_wire_abi_pod!(
    FfiStateTransitionEvent,
    size = 16,
    align = 4,
    schema = [
        state_slot: u32 => "u32",
        exited: u32 => "u32",
        entered: u32 => "u32",
        has_exited: FfiBool => "FfiBool",
        has_entered: FfiBool => "FfiBool",
        allow_same_state_transitions: FfiBool => "FfiBool",
        reserved0: u8 => "u8",
    ]
);

impl FfiStateTransitionEvent {
    /// Returns the exited state discriminant, if any.
    #[inline]
    pub const fn exited(&self) -> Option<u32> {
        if self.has_exited.get() {
            Some(self.exited)
        } else {
            None
        }
    }

    /// Returns the entered state discriminant, if any.
    #[inline]
    pub const fn entered(&self) -> Option<u32> {
        if self.has_entered.get() {
            Some(self.entered)
        } else {
            None
        }
    }
}

/// Host-opt-in `#[repr(C)]` gizmo line segment request (`bevy_gizmos`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiGizmoLine {
    /// Line start point `[x, y, z]`.
    pub start: [f32; 3],
    /// Line end point `[x, y, z]`.
    pub end: [f32; 3],
    /// Line color (sRGBA).
    pub color: FfiColor,
}

impl_wire_abi_pod!(
    FfiGizmoLine,
    size = 40,
    align = 4,
    schema = [
        start: [f32; 3] => "[f32;3]",
        end: [f32; 3] => "[f32;3]",
        color: FfiColor => "FfiColor",
    ]
);

/// Host-opt-in `#[repr(C)]` gizmo ray request (`bevy_gizmos`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiGizmoRay {
    /// Ray origin `[x, y, z]`.
    pub start: [f32; 3],
    /// Ray displacement vector `[dx, dy, dz]`.
    pub vector: [f32; 3],
    /// Ray color (sRGBA).
    pub color: FfiColor,
}

impl_wire_abi_pod!(
    FfiGizmoRay,
    size = 40,
    align = 4,
    schema = [
        start: [f32; 3] => "[f32;3]",
        vector: [f32; 3] => "[f32;3]",
        color: FfiColor => "FfiColor",
    ]
);

/// Host-opt-in `#[repr(C)]` gizmo circle request (`bevy_gizmos`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiGizmoCircle {
    /// Circle center `[x, y, z]`.
    pub center: [f32; 3],
    /// Circle plane normal `[nx, ny, nz]` (`[0, 0, 1]` for 2D XY plane).
    pub normal: [f32; 3],
    /// Circle radius.
    pub radius: f32,
    /// Circle color (sRGBA).
    pub color: FfiColor,
}

impl_wire_abi_pod!(
    FfiGizmoCircle,
    size = 44,
    align = 4,
    schema = [
        center: [f32; 3] => "[f32;3]",
        normal: [f32; 3] => "[f32;3]",
        radius: f32 => "f32",
        color: FfiColor => "FfiColor",
    ]
);

/// Host-opt-in `#[repr(C)]` gizmo wireframe sphere request (`bevy_gizmos`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiGizmoSphere {
    /// Sphere center `[x, y, z]`.
    pub center: [f32; 3],
    /// Sphere radius.
    pub radius: f32,
    /// Sphere color (sRGBA).
    pub color: FfiColor,
}

impl_wire_abi_pod!(
    FfiGizmoSphere,
    size = 32,
    align = 4,
    schema = [
        center: [f32; 3] => "[f32;3]",
        radius: f32 => "f32",
        color: FfiColor => "FfiColor",
    ]
);

/// Host-opt-in `#[repr(C)]` gizmo oriented rectangle request (`bevy_gizmos`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiGizmoRect {
    /// Rectangle center `[x, y, z]`.
    pub center: [f32; 3],
    /// Orientation quaternion `[x, y, z, w]`.
    pub rotation: [f32; 4],
    /// Rectangle extents `[width, height]`.
    pub size: [f32; 2],
    /// Rectangle color (sRGBA).
    pub color: FfiColor,
}

impl_wire_abi_pod!(
    FfiGizmoRect,
    size = 52,
    align = 4,
    schema = [
        center: [f32; 3] => "[f32;3]",
        rotation: [f32; 4] => "[f32;4]",
        size: [f32; 2] => "[f32;2]",
        color: FfiColor => "FfiColor",
    ]
);

/// Host-opt-in `#[repr(C)]` gizmo oriented wireframe cuboid request (`bevy_gizmos`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiGizmoCuboid {
    /// Cuboid center `[x, y, z]`.
    pub center: [f32; 3],
    /// Orientation quaternion `[x, y, z, w]`.
    pub rotation: [f32; 4],
    /// Full box extents `[width, height, depth]`.
    pub size: [f32; 3],
    /// Cuboid color (sRGBA).
    pub color: FfiColor,
}

impl_wire_abi_pod!(
    FfiGizmoCuboid,
    size = 56,
    align = 4,
    schema = [
        center: [f32; 3] => "[f32;3]",
        rotation: [f32; 4] => "[f32;4]",
        size: [f32; 3] => "[f32;3]",
        color: FfiColor => "FfiColor",
    ]
);

/// Host-opt-in `#[repr(C)]` gizmo arrow request (`bevy_gizmos`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiGizmoArrow {
    /// Arrow tail `[x, y, z]`.
    pub start: [f32; 3],
    /// Arrow tip `[x, y, z]`.
    pub end: [f32; 3],
    /// Optional arrowhead tip length (`0.0` = host default).
    pub tip_length: f32,
    /// Arrow color (sRGBA).
    pub color: FfiColor,
}

impl_wire_abi_pod!(
    FfiGizmoArrow,
    size = 44,
    align = 4,
    schema = [
        start: [f32; 3] => "[f32;3]",
        end: [f32; 3] => "[f32;3]",
        tip_length: f32 => "f32",
        color: FfiColor => "FfiColor",
    ]
);
