//! Unconditional cross-language `#[repr(C)]` / `#[repr(transparent)]` `AbiPod` wire projection
//! types for Bevy `0.20` core and companion subsystems.
//!
//! Defining these wire structs inside `bamabi_guest_sys` with compile-time zero-padding,
//! per-field [`bamabi_sys::AbiPod`] proofs, and deterministic `FIELD_SCHEMA_HASH` constants
//! (matching `#[derive(ModComponent)]` and `#[derive(GuestComponent)]` bit-for-bit) guarantees
//! that every Bevy `0.20` bridge in `bamabi_guest` works unconditionally even with
//! `--no-default-features` (`default = []`), while keeping `bamabi_guest` strictly
//! `#![forbid(unsafe_code)]` and `bamabi_sys` frozen at ABI `0.3.0`.

/// Implements [`bamabi_sys::RawInsertable`] and [`bamabi_sys::AbiPod`] for a `#[repr(C)]` or
/// `#[repr(transparent)]` wire projection struct after asserting at compile time that:
/// 1. `!core::mem::needs_drop::<Self>()`,
/// 2. every field type implements [`bamabi_sys::AbiPod`],
/// 3. `size_of::<Self>()` equals the exact sum of field sizes (zero implicit padding), and
/// 4. `(size_of::<Self>(), align_of::<Self>())` match the declared ABI layout.
///
/// The `schema = [field: Ty => "tag", ...]` arm additionally synthesizes
/// `pub const FIELD_SCHEMA_HASH: u64` using [`bamabi_sys::ComponentUuid::hash_field_schema`] and
/// `core::mem::offset_of!`, producing a bit-identical SipHash-2-4 schema digest to
/// `#[derive(ModComponent)]` and `#[derive(GuestComponent)]` without requiring proc-macros.
macro_rules! impl_wire_abi_pod {
    (
        $ty:ty,
        size = $size:expr,
        align = $align:expr,
        fields = [$($field_ty:ty),* $(,)?]
    ) => {
        const _: () = {
            assert!(!::core::mem::needs_drop::<$ty>());
            $(
                ::bamabi_sys::assert_abi_pod::<$field_ty>();
            )*
            let field_sum: usize = 0usize $( + ::core::mem::size_of::<$field_ty>() )*;
            assert!(::core::mem::size_of::<$ty>() == field_sum);
            assert!(::core::mem::size_of::<$ty>() == $size);
            assert!(::core::mem::align_of::<$ty>() == $align);
        };

        // SAFETY: `$ty` is `#[repr(C)]` or `#[repr(transparent)]`, `Copy + Send + Sync + 'static`,
        // has no drop glue (`!needs_drop`), has zero implicit padding verified above, and every
        // field satisfies `AbiPod` (hence `RawInsertable`).
        unsafe impl ::bamabi_sys::RawInsertable for $ty {}

        // SAFETY: All fields satisfy `AbiPod`, layout is deterministic C-ABI with zero implicit
        // padding, and exact `(size, align)` are asserted at compile time above.
        unsafe impl ::bamabi_sys::AbiPod for $ty {}
    };
    (
        $ty:ty,
        size = $size:expr,
        align = $align:expr,
        schema = [$($field_name:ident : $field_ty:ty => $type_tag:literal),* $(,)?]
    ) => {
        impl_wire_abi_pod!(
            $ty,
            size = $size,
            align = $align,
            fields = [$($field_ty),*]
        );

        impl $ty {
            /// Deterministic SipHash-2-4 field-schema hash matching `#[derive(ModComponent)]` and
            /// `#[derive(GuestComponent)]` on the host.
            pub const FIELD_SCHEMA_HASH: u64 = {
                let mut acc: u64 = 0;
                $(
                    acc = ::bamabi_sys::ComponentUuid::hash_field_schema(
                        acc,
                        stringify!($field_name),
                        $type_tag,
                        ::core::mem::offset_of!($ty, $field_name),
                    );
                )*
                acc
            };
        }
    };
}

pub mod audio;
pub mod camera_light;
pub mod lifecycle;
pub mod picking;
pub mod render;
pub mod std_types;
pub mod ui;

pub use audio::{
    APPLY_EAR_POSITIONS, APPLY_EMITTER_POS, APPLY_SPEED, APPLY_VOLUME, FfiAudioPlaybackSettings,
    FfiAudioPlayerSpec, FfiSpatialAudioSinkControl, MUTE_CMD_MUTE, MUTE_CMD_NONE, MUTE_CMD_TOGGLE,
    MUTE_CMD_UNMUTE, PLAYBACK_MODE_DESPAWN, PLAYBACK_MODE_LOOP, PLAYBACK_MODE_ONCE,
    PLAYBACK_MODE_REMOVE, SINK_CMD_NONE, SINK_CMD_PAUSE, SINK_CMD_PLAY, SINK_CMD_STOP,
    SINK_CMD_TOGGLE_PLAYBACK, VOLUME_DECIBELS, VOLUME_LINEAR,
};
pub use camera_light::{
    CLEAR_COLOR_CUSTOM, CLEAR_COLOR_DEFAULT, CLEAR_COLOR_NONE, DEPTH_LOAD_OP_CLEAR,
    DEPTH_LOAD_OP_LOAD, FfiCamera2d, FfiCamera3d, FfiDirectionalLight, FfiPanOrbitCamera,
    FfiPointLight, FfiSolariLighting, FfiSpotLight, ORBIT_CONSTRAINT_FIXED, ORBIT_CONSTRAINT_FREE,
    TONEMAPPING_ACES_FITTED, TONEMAPPING_AGX, TONEMAPPING_BLENDER_FILMIC,
    TONEMAPPING_KHRONOS_PBR_NEUTRAL, TONEMAPPING_LINEAR, TONEMAPPING_NONE, TONEMAPPING_REINHARD,
    TONEMAPPING_REINHARD_LUMINANCE, TONEMAPPING_SOMEWHAT_BORING_DISPLAY_TRANSFORM,
    TONEMAPPING_TONY_MC_MAPFACE, TRANSMISSION_QUALITY_HIGH, TRANSMISSION_QUALITY_LOW,
    TRANSMISSION_QUALITY_MEDIUM, TRANSMISSION_QUALITY_ULTRA,
};
pub use lifecycle::{
    FfiAppExit, FfiAssetEvent, FfiAssetEventKind, FfiGizmoArrow, FfiGizmoCircle, FfiGizmoCuboid,
    FfiGizmoLine, FfiGizmoRay, FfiGizmoRect, FfiGizmoSphere, FfiStateTransitionEvent, FfiWindow,
    FfiWindowCloseRequested, FfiWindowMode, FfiWindowResized, MOD_STATE_FLAG_ACTIVE,
    MOD_STATE_FLAG_COMPUTED, MOD_STATE_FLAG_MUTABLE, MOD_STATE_FLAG_SUB_STATE, ModDespawnOnEnter,
    ModDespawnOnExit, ModStateEntry, ModStateHandle, ModStateTransitionRequest,
    STATE_ABSENT_DISCRIMINANT, SYSTEM_TELEMETRY_DIGEST_SIZE, SYSTEM_TELEMETRY_DIGEST_VERSION,
    SYSTEM_TELEMETRY_MAGIC, SystemTelemetryDigest64, TELEMETRY_FLAG_BUDGET_BREACHED,
    TELEMETRY_FLAG_FAULTED, TELEMETRY_FLAG_QUARANTINED, TELEMETRY_FLAG_SUMMARY_TICK_ACTIVE,
    TelemetryFrameError, fnv1a_64_hash_system,
};
pub use picking::{
    FfiEntityCursor, FfiPointerButton, FfiPointerClick, FfiPointerDrag, FfiPointerKind,
    FfiPointerMove, FfiPointerOut, FfiPointerOver, FfiPointerPress, FfiPointerRelease,
};
pub use render::{
    FfiMesh2d, FfiMesh3d, FfiSprite, FfiSpriteMaterialSpec, FfiStandardMaterial,
    FfiWeslMaterialSpec,
};
pub use std_types::{
    FfiColor, FfiGamepadButton, FfiGamepadRumble, FfiGamepadState, FfiGlobalTransform,
    FfiInheritedVisibility, FfiKeyCode, FfiKeyboardInput, FfiMouseButton, FfiMouseInput,
    FfiMouseMotion, FfiMouseScroll, FfiTimeFixed, FfiTimeReal, FfiTimeVirtual, FfiTransform,
    FfiViewVisibility, FfiVisibility,
};
pub use ui::{
    FfiBorderColor, FfiBorderRadius, FfiCornerRadius, FfiEditableTextConfig, FfiEmSize,
    FfiFeathersColorInput, FfiFeathersDropdown, FfiFeathersListRow, FfiFeathersListView,
    FfiFeathersNumberInput, FfiFeathersScrollableList, FfiFeathersSelect, FfiFontSize,
    FfiInlineBox, FfiInlineImage, FfiRemSize, FfiRgba, FfiSelectedTab, FfiTab, FfiTabList,
    FfiThemeContext, FfiUiNode, FfiUiRect, FfiUiTextStyle, FfiVal, FfiValueChange,
};
