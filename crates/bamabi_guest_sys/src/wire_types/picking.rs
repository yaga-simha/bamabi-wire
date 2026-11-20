//! Bevy `0.20` `bevy_picking` flattened observer event wire types and `EntityCursor` mirror
//! matching `bamabi_bridges::picking` byte-for-byte.

use bamabi_sys::{FfiBool, FfiEntity};

/// Wire discriminant identifying the kind of Bevy `PointerId`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfiPointerKind(pub u8);

impl_wire_abi_pod!(FfiPointerKind, size = 1, align = 1, fields = [u8]);

impl FfiPointerKind {
    /// Mouse pointer (`PointerId::Mouse` = `0`).
    pub const MOUSE: Self = Self(0);
    /// Touch pointer (`PointerId::Touch(u64)` = `1`).
    pub const TOUCH: Self = Self(1);
    /// Custom UUID-identified pointer (`PointerId::Custom(Uuid)` = `2`).
    pub const CUSTOM: Self = Self(2);

    /// Raw `u8` tag for [`Self::MOUSE`].
    pub const MOUSE_RAW: u8 = 0;
    /// Raw `u8` tag for [`Self::TOUCH`].
    pub const TOUCH_RAW: u8 = 1;
    /// Raw `u8` tag for [`Self::CUSTOM`].
    pub const CUSTOM_RAW: u8 = 2;

    /// Returns `true` if this kind represents a mouse pointer.
    #[inline]
    pub const fn is_mouse(self) -> bool {
        self.0 == Self::MOUSE_RAW
    }

    /// Returns `true` if this kind represents a touch pointer.
    #[inline]
    pub const fn is_touch(self) -> bool {
        self.0 == Self::TOUCH_RAW
    }

    /// Returns `true` if this kind represents a custom UUID pointer.
    #[inline]
    pub const fn is_custom(self) -> bool {
        self.0 == Self::CUSTOM_RAW
    }
}

/// Wire discriminant identifying a Bevy `PointerButton`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfiPointerButton(pub u8);

impl_wire_abi_pod!(FfiPointerButton, size = 1, align = 1, fields = [u8]);

impl FfiPointerButton {
    /// No pointer button (`0`).
    pub const NONE: Self = Self(0);
    /// Primary pointer button (`PointerButton::Primary` = `1`).
    pub const PRIMARY: Self = Self(1);
    /// Secondary pointer button (`PointerButton::Secondary` = `2`).
    pub const SECONDARY: Self = Self(2);
    /// Middle/tertiary pointer button (`PointerButton::Middle` = `3`).
    pub const MIDDLE: Self = Self(3);

    /// Raw `u8` tag for [`Self::NONE`].
    pub const NONE_RAW: u8 = 0;
    /// Raw `u8` tag for [`Self::PRIMARY`].
    pub const PRIMARY_RAW: u8 = 1;
    /// Raw `u8` tag for [`Self::SECONDARY`].
    pub const SECONDARY_RAW: u8 = 2;
    /// Raw `u8` tag for [`Self::MIDDLE`].
    pub const MIDDLE_RAW: u8 = 3;
}

#[inline]
const fn decode_uuid_bytes(lo: u64, hi: u64) -> [u8; 16] {
    let a = lo.to_le_bytes();
    let b = hi.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7], b[0], b[1], b[2], b[3], b[4], b[5], b[6],
        b[7],
    ]
}

/// Zero-padding `#[repr(C)]` wire projection of Bevy `0.20`'s flattened `PointerPress` event
/// (`"bevy_picking::events::PointerPress"`, 96 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiPointerPress {
    /// Target entity receiving this observer dispatch (updated along `PointerTraversal`).
    pub target: FfiEntity,
    /// Original entity hit by the picking backend prior to bubbling propagation.
    pub hit_entity: FfiEntity,
    /// Camera entity that produced the picking hit (or [`FfiEntity::NULL`]).
    pub hit_camera: FfiEntity,
    /// Window entity associated with the pointer render target (or [`FfiEntity::NULL`]).
    pub window: FfiEntity,
    /// Pointer identifier: `0` for mouse, touch ID for touch, or lower 64 bits of custom UUID.
    pub pointer_id: u64,
    /// Upper 64 bits of custom pointer UUID (`0` for mouse/touch).
    pub pointer_id_hi: u64,
    /// Pointer logical position `[x, y]` in the render target.
    pub position: [f32; 2],
    /// Picking intersection depth reported by `HitData::depth`.
    pub hit_depth: f32,
    /// Picking intersection position `[x, y, z]` when [`Self::has_hit_position`] is true.
    pub hit_position: [f32; 3],
    /// Picking intersection surface normal `[x, y, z]` when [`Self::has_hit_normal`] is true.
    pub hit_normal: [f32; 3],
    /// Pointer kind tag (`0 = Mouse`, `1 = Touch`, `2 = Custom`; see [`FfiPointerKind`]).
    pub pointer_kind: u8,
    /// Pressed pointer button (`1 = Primary`, `2 = Secondary`, `3 = Middle`; see [`FfiPointerButton`]).
    pub button: u8,
    /// Number of consecutive presses, starting at `1`.
    pub count: u8,
    /// Whether [`Self::hit_position`] contains a valid backend-reported position.
    pub has_hit_position: FfiBool,
    /// Whether [`Self::hit_normal`] contains a valid backend-reported normal.
    pub has_hit_normal: FfiBool,
    /// Explicit zero-reserved alignment bytes.
    pub reserved0: [u8; 7],
}

impl_wire_abi_pod!(
    FfiPointerPress,
    size = 96,
    align = core::mem::align_of::<FfiEntity>(),
    schema = [
        target: FfiEntity => "TargetEntity",
        hit_entity: FfiEntity => "FfiEntity",
        hit_camera: FfiEntity => "FfiEntity",
        window: FfiEntity => "FfiEntity",
        pointer_id: u64 => "u64",
        pointer_id_hi: u64 => "u64",
        position: [f32; 2] => "[f32;2]",
        hit_depth: f32 => "f32",
        hit_position: [f32; 3] => "[f32;3]",
        hit_normal: [f32; 3] => "[f32;3]",
        pointer_kind: u8 => "u8",
        button: u8 => "u8",
        count: u8 => "u8",
        has_hit_position: FfiBool => "FfiBool",
        has_hit_normal: FfiBool => "FfiBool",
        reserved0: [u8; 7] => "[u8;7]",
    ]
);

impl FfiPointerPress {
    /// Returns the typed [`FfiPointerKind`].
    #[inline]
    pub const fn pointer_kind_typed(&self) -> FfiPointerKind {
        FfiPointerKind(self.pointer_kind)
    }

    /// Returns the typed [`FfiPointerButton`].
    #[inline]
    pub const fn button_typed(&self) -> FfiPointerButton {
        FfiPointerButton(self.button)
    }

    /// Returns the 16-byte custom pointer UUID in little-endian wire order.
    #[inline]
    pub const fn pointer_uuid_bytes(&self) -> [u8; 16] {
        decode_uuid_bytes(self.pointer_id, self.pointer_id_hi)
    }

    /// Returns the picking hit position `[x, y, z]` when present.
    #[inline]
    pub const fn hit_position_opt(&self) -> Option<[f32; 3]> {
        if self.has_hit_position.get() {
            Some(self.hit_position)
        } else {
            None
        }
    }

    /// Returns the picking hit surface normal `[x, y, z]` when present.
    #[inline]
    pub const fn hit_normal_opt(&self) -> Option<[f32; 3]> {
        if self.has_hit_normal.get() {
            Some(self.hit_normal)
        } else {
            None
        }
    }
}

/// Zero-padding `#[repr(C)]` wire projection of Bevy `0.20`'s flattened `PointerRelease` event
/// (`"bevy_picking::events::PointerRelease"`, 88 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiPointerRelease {
    /// Target entity receiving this observer dispatch (updated along `PointerTraversal`).
    pub target: FfiEntity,
    /// Original entity hit by the picking backend prior to bubbling propagation.
    pub hit_entity: FfiEntity,
    /// Camera entity that produced the picking hit (or [`FfiEntity::NULL`]).
    pub hit_camera: FfiEntity,
    /// Window entity associated with the pointer render target (or [`FfiEntity::NULL`]).
    pub window: FfiEntity,
    /// Pointer identifier: `0` for mouse, touch ID for touch, or lower 64 bits of custom UUID.
    pub pointer_id: u64,
    /// Upper 64 bits of custom pointer UUID (`0` for mouse/touch).
    pub pointer_id_hi: u64,
    /// Pointer logical position `[x, y]` in the render target.
    pub position: [f32; 2],
    /// Picking intersection depth reported by `HitData::depth`.
    pub hit_depth: f32,
    /// Picking intersection position `[x, y, z]` when [`Self::has_hit_position`] is true.
    pub hit_position: [f32; 3],
    /// Picking intersection surface normal `[x, y, z]` when [`Self::has_hit_normal`] is true.
    pub hit_normal: [f32; 3],
    /// Pointer kind tag (`0 = Mouse`, `1 = Touch`, `2 = Custom`; see [`FfiPointerKind`]).
    pub pointer_kind: u8,
    /// Released pointer button (`1 = Primary`, `2 = Secondary`, `3 = Middle`; see [`FfiPointerButton`]).
    pub button: u8,
    /// Whether [`Self::hit_position`] contains a valid backend-reported position.
    pub has_hit_position: FfiBool,
    /// Whether [`Self::hit_normal`] contains a valid backend-reported normal.
    pub has_hit_normal: FfiBool,
}

impl_wire_abi_pod!(
    FfiPointerRelease,
    size = 88,
    align = core::mem::align_of::<FfiEntity>(),
    schema = [
        target: FfiEntity => "TargetEntity",
        hit_entity: FfiEntity => "FfiEntity",
        hit_camera: FfiEntity => "FfiEntity",
        window: FfiEntity => "FfiEntity",
        pointer_id: u64 => "u64",
        pointer_id_hi: u64 => "u64",
        position: [f32; 2] => "[f32;2]",
        hit_depth: f32 => "f32",
        hit_position: [f32; 3] => "[f32;3]",
        hit_normal: [f32; 3] => "[f32;3]",
        pointer_kind: u8 => "u8",
        button: u8 => "u8",
        has_hit_position: FfiBool => "FfiBool",
        has_hit_normal: FfiBool => "FfiBool",
    ]
);

impl FfiPointerRelease {
    /// Returns the typed [`FfiPointerKind`].
    #[inline]
    pub const fn pointer_kind_typed(&self) -> FfiPointerKind {
        FfiPointerKind(self.pointer_kind)
    }

    /// Returns the typed [`FfiPointerButton`].
    #[inline]
    pub const fn button_typed(&self) -> FfiPointerButton {
        FfiPointerButton(self.button)
    }

    /// Returns the 16-byte custom pointer UUID in little-endian wire order.
    #[inline]
    pub const fn pointer_uuid_bytes(&self) -> [u8; 16] {
        decode_uuid_bytes(self.pointer_id, self.pointer_id_hi)
    }

    /// Returns the picking hit position `[x, y, z]` when present.
    #[inline]
    pub const fn hit_position_opt(&self) -> Option<[f32; 3]> {
        if self.has_hit_position.get() {
            Some(self.hit_position)
        } else {
            None
        }
    }

    /// Returns the picking hit surface normal `[x, y, z]` when present.
    #[inline]
    pub const fn hit_normal_opt(&self) -> Option<[f32; 3]> {
        if self.has_hit_normal.get() {
            Some(self.hit_normal)
        } else {
            None
        }
    }
}

/// Zero-padding `#[repr(C)]` wire projection of Bevy `0.20`'s flattened `PointerMove` event
/// (`"bevy_picking::events::PointerMove"`, 96 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiPointerMove {
    /// Target entity receiving this observer dispatch (updated along `PointerTraversal`).
    pub target: FfiEntity,
    /// Original entity hit by the picking backend prior to bubbling propagation.
    pub hit_entity: FfiEntity,
    /// Camera entity that produced the picking hit (or [`FfiEntity::NULL`]).
    pub hit_camera: FfiEntity,
    /// Window entity associated with the pointer render target (or [`FfiEntity::NULL`]).
    pub window: FfiEntity,
    /// Pointer identifier: `0` for mouse, touch ID for touch, or lower 64 bits of custom UUID.
    pub pointer_id: u64,
    /// Upper 64 bits of custom pointer UUID (`0` for mouse/touch).
    pub pointer_id_hi: u64,
    /// Pointer logical position `[x, y]` in the render target.
    pub position: [f32; 2],
    /// Screen-pixel movement delta `[dx, dy]` since the previous move event.
    pub delta: [f32; 2],
    /// Picking intersection depth reported by `HitData::depth`.
    pub hit_depth: f32,
    /// Picking intersection position `[x, y, z]` when [`Self::has_hit_position`] is true.
    pub hit_position: [f32; 3],
    /// Picking intersection surface normal `[x, y, z]` when [`Self::has_hit_normal`] is true.
    pub hit_normal: [f32; 3],
    /// Pointer kind tag (`0 = Mouse`, `1 = Touch`, `2 = Custom`; see [`FfiPointerKind`]).
    pub pointer_kind: u8,
    /// Whether [`Self::hit_position`] contains a valid backend-reported position.
    pub has_hit_position: FfiBool,
    /// Whether [`Self::hit_normal`] contains a valid backend-reported normal.
    pub has_hit_normal: FfiBool,
    /// Explicit zero-reserved alignment byte.
    pub reserved0: u8,
}

impl_wire_abi_pod!(
    FfiPointerMove,
    size = 96,
    align = core::mem::align_of::<FfiEntity>(),
    schema = [
        target: FfiEntity => "TargetEntity",
        hit_entity: FfiEntity => "FfiEntity",
        hit_camera: FfiEntity => "FfiEntity",
        window: FfiEntity => "FfiEntity",
        pointer_id: u64 => "u64",
        pointer_id_hi: u64 => "u64",
        position: [f32; 2] => "[f32;2]",
        delta: [f32; 2] => "[f32;2]",
        hit_depth: f32 => "f32",
        hit_position: [f32; 3] => "[f32;3]",
        hit_normal: [f32; 3] => "[f32;3]",
        pointer_kind: u8 => "u8",
        has_hit_position: FfiBool => "FfiBool",
        has_hit_normal: FfiBool => "FfiBool",
        reserved0: u8 => "u8",
    ]
);

impl FfiPointerMove {
    /// Returns the typed [`FfiPointerKind`].
    #[inline]
    pub const fn pointer_kind_typed(&self) -> FfiPointerKind {
        FfiPointerKind(self.pointer_kind)
    }

    /// Returns the 16-byte custom pointer UUID in little-endian wire order.
    #[inline]
    pub const fn pointer_uuid_bytes(&self) -> [u8; 16] {
        decode_uuid_bytes(self.pointer_id, self.pointer_id_hi)
    }

    /// Returns the picking hit position `[x, y, z]` when present.
    #[inline]
    pub const fn hit_position_opt(&self) -> Option<[f32; 3]> {
        if self.has_hit_position.get() {
            Some(self.hit_position)
        } else {
            None
        }
    }

    /// Returns the picking hit surface normal `[x, y, z]` when present.
    #[inline]
    pub const fn hit_normal_opt(&self) -> Option<[f32; 3]> {
        if self.has_hit_normal.get() {
            Some(self.hit_normal)
        } else {
            None
        }
    }
}

/// Zero-padding `#[repr(C)]` wire projection of Bevy `0.20`'s flattened `PointerClick` event
/// (`"bevy_picking::events::PointerClick"`, 104 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiPointerClick {
    /// Target entity receiving this observer dispatch (updated along `PointerTraversal`).
    pub target: FfiEntity,
    /// Original entity hit by the picking backend prior to bubbling propagation.
    pub hit_entity: FfiEntity,
    /// Camera entity that produced the picking hit (or [`FfiEntity::NULL`]).
    pub hit_camera: FfiEntity,
    /// Window entity associated with the pointer render target (or [`FfiEntity::NULL`]).
    pub window: FfiEntity,
    /// Pointer identifier: `0` for mouse, touch ID for touch, or lower 64 bits of custom UUID.
    pub pointer_id: u64,
    /// Upper 64 bits of custom pointer UUID (`0` for mouse/touch).
    pub pointer_id_hi: u64,
    /// Duration between pointer press and release in seconds (`f64`).
    pub duration_secs: f64,
    /// Pointer logical position `[x, y]` in the render target.
    pub position: [f32; 2],
    /// Picking intersection depth reported by `HitData::depth`.
    pub hit_depth: f32,
    /// Picking intersection position `[x, y, z]` when [`Self::has_hit_position`] is true.
    pub hit_position: [f32; 3],
    /// Picking intersection surface normal `[x, y, z]` when [`Self::has_hit_normal`] is true.
    pub hit_normal: [f32; 3],
    /// Pointer kind tag (`0 = Mouse`, `1 = Touch`, `2 = Custom`; see [`FfiPointerKind`]).
    pub pointer_kind: u8,
    /// Clicked pointer button (`1 = Primary`, `2 = Secondary`, `3 = Middle`; see [`FfiPointerButton`]).
    pub button: u8,
    /// Number of consecutive clicks, starting at `1`.
    pub count: u8,
    /// Whether [`Self::hit_position`] contains a valid backend-reported position.
    pub has_hit_position: FfiBool,
    /// Whether [`Self::hit_normal`] contains a valid backend-reported normal.
    pub has_hit_normal: FfiBool,
    /// Explicit zero-reserved alignment bytes.
    pub reserved0: [u8; 7],
}

impl_wire_abi_pod!(
    FfiPointerClick,
    size = 104,
    align = core::mem::align_of::<FfiEntity>(),
    schema = [
        target: FfiEntity => "TargetEntity",
        hit_entity: FfiEntity => "FfiEntity",
        hit_camera: FfiEntity => "FfiEntity",
        window: FfiEntity => "FfiEntity",
        pointer_id: u64 => "u64",
        pointer_id_hi: u64 => "u64",
        duration_secs: f64 => "f64",
        position: [f32; 2] => "[f32;2]",
        hit_depth: f32 => "f32",
        hit_position: [f32; 3] => "[f32;3]",
        hit_normal: [f32; 3] => "[f32;3]",
        pointer_kind: u8 => "u8",
        button: u8 => "u8",
        count: u8 => "u8",
        has_hit_position: FfiBool => "FfiBool",
        has_hit_normal: FfiBool => "FfiBool",
        reserved0: [u8; 7] => "[u8;7]",
    ]
);

impl FfiPointerClick {
    /// Returns the typed [`FfiPointerKind`].
    #[inline]
    pub const fn pointer_kind_typed(&self) -> FfiPointerKind {
        FfiPointerKind(self.pointer_kind)
    }

    /// Returns the typed [`FfiPointerButton`].
    #[inline]
    pub const fn button_typed(&self) -> FfiPointerButton {
        FfiPointerButton(self.button)
    }

    /// Returns the 16-byte custom pointer UUID in little-endian wire order.
    #[inline]
    pub const fn pointer_uuid_bytes(&self) -> [u8; 16] {
        decode_uuid_bytes(self.pointer_id, self.pointer_id_hi)
    }

    /// Returns the picking hit position `[x, y, z]` when present.
    #[inline]
    pub const fn hit_position_opt(&self) -> Option<[f32; 3]> {
        if self.has_hit_position.get() {
            Some(self.hit_position)
        } else {
            None
        }
    }

    /// Returns the picking hit surface normal `[x, y, z]` when present.
    #[inline]
    pub const fn hit_normal_opt(&self) -> Option<[f32; 3]> {
        if self.has_hit_normal.get() {
            Some(self.hit_normal)
        } else {
            None
        }
    }
}

/// Zero-padding `#[repr(C)]` wire projection of Bevy `0.20`'s flattened `PointerOver` event
/// (`"bevy_picking::events::PointerOver"`, 88 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiPointerOver {
    /// Target entity receiving this observer dispatch (updated along `PointerTraversal`).
    pub target: FfiEntity,
    /// Original entity hit by the picking backend prior to bubbling propagation.
    pub hit_entity: FfiEntity,
    /// Camera entity that produced the picking hit (or [`FfiEntity::NULL`]).
    pub hit_camera: FfiEntity,
    /// Window entity associated with the pointer render target (or [`FfiEntity::NULL`]).
    pub window: FfiEntity,
    /// Pointer identifier: `0` for mouse, touch ID for touch, or lower 64 bits of custom UUID.
    pub pointer_id: u64,
    /// Upper 64 bits of custom pointer UUID (`0` for mouse/touch).
    pub pointer_id_hi: u64,
    /// Pointer logical position `[x, y]` in the render target.
    pub position: [f32; 2],
    /// Picking intersection depth reported by `HitData::depth`.
    pub hit_depth: f32,
    /// Picking intersection position `[x, y, z]` when [`Self::has_hit_position`] is true.
    pub hit_position: [f32; 3],
    /// Picking intersection surface normal `[x, y, z]` when [`Self::has_hit_normal`] is true.
    pub hit_normal: [f32; 3],
    /// Pointer kind tag (`0 = Mouse`, `1 = Touch`, `2 = Custom`; see [`FfiPointerKind`]).
    pub pointer_kind: u8,
    /// Whether [`Self::hit_position`] contains a valid backend-reported position.
    pub has_hit_position: FfiBool,
    /// Whether [`Self::hit_normal`] contains a valid backend-reported normal.
    pub has_hit_normal: FfiBool,
    /// Explicit zero-reserved alignment byte.
    pub reserved0: u8,
}

impl_wire_abi_pod!(
    FfiPointerOver,
    size = 88,
    align = core::mem::align_of::<FfiEntity>(),
    schema = [
        target: FfiEntity => "TargetEntity",
        hit_entity: FfiEntity => "FfiEntity",
        hit_camera: FfiEntity => "FfiEntity",
        window: FfiEntity => "FfiEntity",
        pointer_id: u64 => "u64",
        pointer_id_hi: u64 => "u64",
        position: [f32; 2] => "[f32;2]",
        hit_depth: f32 => "f32",
        hit_position: [f32; 3] => "[f32;3]",
        hit_normal: [f32; 3] => "[f32;3]",
        pointer_kind: u8 => "u8",
        has_hit_position: FfiBool => "FfiBool",
        has_hit_normal: FfiBool => "FfiBool",
        reserved0: u8 => "u8",
    ]
);

impl FfiPointerOver {
    /// Returns the typed [`FfiPointerKind`].
    #[inline]
    pub const fn pointer_kind_typed(&self) -> FfiPointerKind {
        FfiPointerKind(self.pointer_kind)
    }

    /// Returns the 16-byte custom pointer UUID in little-endian wire order.
    #[inline]
    pub const fn pointer_uuid_bytes(&self) -> [u8; 16] {
        decode_uuid_bytes(self.pointer_id, self.pointer_id_hi)
    }

    /// Returns the picking hit position `[x, y, z]` when present.
    #[inline]
    pub const fn hit_position_opt(&self) -> Option<[f32; 3]> {
        if self.has_hit_position.get() {
            Some(self.hit_position)
        } else {
            None
        }
    }

    /// Returns the picking hit surface normal `[x, y, z]` when present.
    #[inline]
    pub const fn hit_normal_opt(&self) -> Option<[f32; 3]> {
        if self.has_hit_normal.get() {
            Some(self.hit_normal)
        } else {
            None
        }
    }
}

/// Zero-padding `#[repr(C)]` wire projection of Bevy `0.20`'s flattened `PointerOut` event
/// (`"bevy_picking::events::PointerOut"`, 88 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiPointerOut {
    /// Target entity receiving this observer dispatch (updated along `PointerTraversal`).
    pub target: FfiEntity,
    /// Original entity exited by the pointer prior to bubbling propagation.
    pub hit_entity: FfiEntity,
    /// Camera entity that produced the prior picking hit (or [`FfiEntity::NULL`]).
    pub hit_camera: FfiEntity,
    /// Window entity associated with the pointer render target (or [`FfiEntity::NULL`]).
    pub window: FfiEntity,
    /// Pointer identifier: `0` for mouse, touch ID for touch, or lower 64 bits of custom UUID.
    pub pointer_id: u64,
    /// Upper 64 bits of custom pointer UUID (`0` for mouse/touch).
    pub pointer_id_hi: u64,
    /// Pointer logical position `[x, y]` in the render target.
    pub position: [f32; 2],
    /// Prior picking intersection depth reported by `HitData::depth`.
    pub hit_depth: f32,
    /// Prior picking intersection position `[x, y, z]` when [`Self::has_hit_position`] is true.
    pub hit_position: [f32; 3],
    /// Prior picking intersection surface normal `[x, y, z]` when [`Self::has_hit_normal`] is true.
    pub hit_normal: [f32; 3],
    /// Pointer kind tag (`0 = Mouse`, `1 = Touch`, `2 = Custom`; see [`FfiPointerKind`]).
    pub pointer_kind: u8,
    /// Whether [`Self::hit_position`] contains a valid backend-reported position.
    pub has_hit_position: FfiBool,
    /// Whether [`Self::hit_normal`] contains a valid backend-reported normal.
    pub has_hit_normal: FfiBool,
    /// Explicit zero-reserved alignment byte.
    pub reserved0: u8,
}

impl_wire_abi_pod!(
    FfiPointerOut,
    size = 88,
    align = core::mem::align_of::<FfiEntity>(),
    schema = [
        target: FfiEntity => "TargetEntity",
        hit_entity: FfiEntity => "FfiEntity",
        hit_camera: FfiEntity => "FfiEntity",
        window: FfiEntity => "FfiEntity",
        pointer_id: u64 => "u64",
        pointer_id_hi: u64 => "u64",
        position: [f32; 2] => "[f32;2]",
        hit_depth: f32 => "f32",
        hit_position: [f32; 3] => "[f32;3]",
        hit_normal: [f32; 3] => "[f32;3]",
        pointer_kind: u8 => "u8",
        has_hit_position: FfiBool => "FfiBool",
        has_hit_normal: FfiBool => "FfiBool",
        reserved0: u8 => "u8",
    ]
);

impl FfiPointerOut {
    /// Returns the typed [`FfiPointerKind`].
    #[inline]
    pub const fn pointer_kind_typed(&self) -> FfiPointerKind {
        FfiPointerKind(self.pointer_kind)
    }

    /// Returns the 16-byte custom pointer UUID in little-endian wire order.
    #[inline]
    pub const fn pointer_uuid_bytes(&self) -> [u8; 16] {
        decode_uuid_bytes(self.pointer_id, self.pointer_id_hi)
    }

    /// Returns the prior picking hit position `[x, y, z]` when present.
    #[inline]
    pub const fn hit_position_opt(&self) -> Option<[f32; 3]> {
        if self.has_hit_position.get() {
            Some(self.hit_position)
        } else {
            None
        }
    }

    /// Returns the prior picking hit surface normal `[x, y, z]` when present.
    #[inline]
    pub const fn hit_normal_opt(&self) -> Option<[f32; 3]> {
        if self.has_hit_normal.get() {
            Some(self.hit_normal)
        } else {
            None
        }
    }
}

/// Zero-padding `#[repr(C)]` wire projection of Bevy `0.20`'s flattened `PointerDrag` event
/// (`"bevy_picking::events::PointerDrag"`, 72 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiPointerDrag {
    /// Target entity receiving this observer dispatch (updated along `PointerTraversal`).
    pub target: FfiEntity,
    /// Original entity being dragged prior to bubbling propagation.
    pub hit_entity: FfiEntity,
    /// Window entity associated with the pointer render target (or [`FfiEntity::NULL`]).
    pub window: FfiEntity,
    /// Pointer identifier: `0` for mouse, touch ID for touch, or lower 64 bits of custom UUID.
    pub pointer_id: u64,
    /// Upper 64 bits of custom pointer UUID (`0` for mouse/touch).
    pub pointer_id_hi: u64,
    /// Pointer logical position `[x, y]` in the render target.
    pub position: [f32; 2],
    /// Total drag vector `[dx, dy]` in screen pixels from drag start to the current position.
    pub distance: [f32; 2],
    /// Incremental drag delta `[dx, dy]` in screen pixels since the previous drag event.
    pub delta: [f32; 2],
    /// Pointer kind tag (`0 = Mouse`, `1 = Touch`, `2 = Custom`; see [`FfiPointerKind`]).
    pub pointer_kind: u8,
    /// Held pointer button (`1 = Primary`, `2 = Secondary`, `3 = Middle`; see [`FfiPointerButton`]).
    pub button: u8,
    /// Explicit zero-reserved alignment bytes.
    pub reserved0: [u8; 6],
}

impl_wire_abi_pod!(
    FfiPointerDrag,
    size = 72,
    align = core::mem::align_of::<FfiEntity>(),
    schema = [
        target: FfiEntity => "TargetEntity",
        hit_entity: FfiEntity => "FfiEntity",
        window: FfiEntity => "FfiEntity",
        pointer_id: u64 => "u64",
        pointer_id_hi: u64 => "u64",
        position: [f32; 2] => "[f32;2]",
        distance: [f32; 2] => "[f32;2]",
        delta: [f32; 2] => "[f32;2]",
        pointer_kind: u8 => "u8",
        button: u8 => "u8",
        reserved0: [u8; 6] => "[u8;6]",
    ]
);

impl FfiPointerDrag {
    /// Returns the typed [`FfiPointerKind`].
    #[inline]
    pub const fn pointer_kind_typed(&self) -> FfiPointerKind {
        FfiPointerKind(self.pointer_kind)
    }

    /// Returns the typed [`FfiPointerButton`].
    #[inline]
    pub const fn button_typed(&self) -> FfiPointerButton {
        FfiPointerButton(self.button)
    }

    /// Returns the 16-byte custom pointer UUID in little-endian wire order.
    #[inline]
    pub const fn pointer_uuid_bytes(&self) -> [u8; 16] {
        decode_uuid_bytes(self.pointer_id, self.pointer_id_hi)
    }
}

/// Zero-padding `#[repr(C)]` POD component mirroring Bevy's `EntityCursor` (`SystemCursorIcon`,
/// `"bevy_picking::cursor::EntityCursor"`, 1 byte).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfiEntityCursor {
    /// Deterministic discriminant matching `SystemCursorIcon` (`0 = Default` .. `33 = ZoomOut`).
    pub icon: u8,
}

impl_wire_abi_pod!(
    FfiEntityCursor,
    size = 1,
    align = 1,
    schema = [icon: u8 => "u8"]
);

impl FfiEntityCursor {
    /// `SystemCursorIcon::Default` (`0`).
    pub const DEFAULT: Self = Self { icon: 0 };
    /// `SystemCursorIcon::ContextMenu` (`1`).
    pub const CONTEXT_MENU: Self = Self { icon: 1 };
    /// `SystemCursorIcon::Help` (`2`).
    pub const HELP: Self = Self { icon: 2 };
    /// `SystemCursorIcon::Pointer` (`3`).
    pub const POINTER: Self = Self { icon: 3 };
    /// `SystemCursorIcon::Progress` (`4`).
    pub const PROGRESS: Self = Self { icon: 4 };
    /// `SystemCursorIcon::Wait` (`5`).
    pub const WAIT: Self = Self { icon: 5 };
    /// `SystemCursorIcon::Cell` (`6`).
    pub const CELL: Self = Self { icon: 6 };
    /// `SystemCursorIcon::Crosshair` (`7`).
    pub const CROSSHAIR: Self = Self { icon: 7 };
    /// `SystemCursorIcon::Text` (`8`).
    pub const TEXT: Self = Self { icon: 8 };
    /// `SystemCursorIcon::VerticalText` (`9`).
    pub const VERTICAL_TEXT: Self = Self { icon: 9 };
    /// `SystemCursorIcon::Alias` (`10`).
    pub const ALIAS: Self = Self { icon: 10 };
    /// `SystemCursorIcon::Copy` (`11`).
    pub const COPY: Self = Self { icon: 11 };
    /// `SystemCursorIcon::Move` (`12`).
    pub const MOVE: Self = Self { icon: 12 };
    /// `SystemCursorIcon::NoDrop` (`13`).
    pub const NO_DROP: Self = Self { icon: 13 };
    /// `SystemCursorIcon::NotAllowed` (`14`).
    pub const NOT_ALLOWED: Self = Self { icon: 14 };
    /// `SystemCursorIcon::Grab` (`15`).
    pub const GRAB: Self = Self { icon: 15 };
    /// `SystemCursorIcon::Grabbing` (`16`).
    pub const GRABBING: Self = Self { icon: 16 };
    /// `SystemCursorIcon::EResize` (`17`).
    pub const E_RESIZE: Self = Self { icon: 17 };
    /// `SystemCursorIcon::NResize` (`18`).
    pub const N_RESIZE: Self = Self { icon: 18 };
    /// `SystemCursorIcon::NeResize` (`19`).
    pub const NE_RESIZE: Self = Self { icon: 19 };
    /// `SystemCursorIcon::NwResize` (`20`).
    pub const NW_RESIZE: Self = Self { icon: 20 };
    /// `SystemCursorIcon::SResize` (`21`).
    pub const S_RESIZE: Self = Self { icon: 21 };
    /// `SystemCursorIcon::SeResize` (`22`).
    pub const SE_RESIZE: Self = Self { icon: 22 };
    /// `SystemCursorIcon::SwResize` (`23`).
    pub const SW_RESIZE: Self = Self { icon: 23 };
    /// `SystemCursorIcon::WResize` (`24`).
    pub const W_RESIZE: Self = Self { icon: 24 };
    /// `SystemCursorIcon::EwResize` (`25`).
    pub const EW_RESIZE: Self = Self { icon: 25 };
    /// `SystemCursorIcon::NsResize` (`26`).
    pub const NS_RESIZE: Self = Self { icon: 26 };
    /// `SystemCursorIcon::NeswResize` (`27`).
    pub const NESW_RESIZE: Self = Self { icon: 27 };
    /// `SystemCursorIcon::NwseResize` (`28`).
    pub const NWSE_RESIZE: Self = Self { icon: 28 };
    /// `SystemCursorIcon::ColResize` (`29`).
    pub const COL_RESIZE: Self = Self { icon: 29 };
    /// `SystemCursorIcon::RowResize` (`30`).
    pub const ROW_RESIZE: Self = Self { icon: 30 };
    /// `SystemCursorIcon::AllScroll` (`31`).
    pub const ALL_SCROLL: Self = Self { icon: 31 };
    /// `SystemCursorIcon::ZoomIn` (`32`).
    pub const ZOOM_IN: Self = Self { icon: 32 };
    /// `SystemCursorIcon::ZoomOut` (`33`).
    pub const ZOOM_OUT: Self = Self { icon: 33 };
}
