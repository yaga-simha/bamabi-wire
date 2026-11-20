//! Host-opt-in `#![forbid(unsafe_code)]`, zero-Bevy guest bridge conventions for Bevy `0.20`
//! `bevy_gizmos` immediate debug drawing requests (`line`, `ray`, `circle`, `sphere`, `rect`,
//! `cuboid`, `arrow`).
//!
//! **Host-opt-in convention:** Requires the host game to register the corresponding
//! `bevy_gizmos::FfiGizmo*` `ModComponent` event types and forward them to `Gizmos`.

use bamabi_sys::FfiResult;
use core::ffi::CStr;

pub use bamabi_guest_sys::wire_types::{
    FfiColor, FfiGizmoArrow, FfiGizmoCircle, FfiGizmoCuboid, FfiGizmoLine, FfiGizmoRay,
    FfiGizmoRect, FfiGizmoSphere,
};

use crate::{commands::GuestCommands, component::GuestComponent};

/// Canonical ABI name for debug gizmo line events.
pub const CANONICAL_GIZMO_LINE_NAME: &str = "bevy_gizmos::FfiGizmoLine";
/// Canonical ABI name for debug gizmo ray events.
pub const CANONICAL_GIZMO_RAY_NAME: &str = "bevy_gizmos::FfiGizmoRay";
/// Canonical ABI name for debug gizmo circle events.
pub const CANONICAL_GIZMO_CIRCLE_NAME: &str = "bevy_gizmos::FfiGizmoCircle";
/// Canonical ABI name for debug gizmo sphere events.
pub const CANONICAL_GIZMO_SPHERE_NAME: &str = "bevy_gizmos::FfiGizmoSphere";
/// Canonical ABI name for debug gizmo rectangle events.
pub const CANONICAL_GIZMO_RECT_NAME: &str = "bevy_gizmos::FfiGizmoRect";
/// Canonical ABI name for debug gizmo cuboid events.
pub const CANONICAL_GIZMO_CUBOID_NAME: &str = "bevy_gizmos::FfiGizmoCuboid";
/// Canonical ABI name for debug gizmo arrow events.
pub const CANONICAL_GIZMO_ARROW_NAME: &str = "bevy_gizmos::FfiGizmoArrow";

impl GuestComponent for FfiGizmoLine {
    const NAME: &'static CStr = c"bevy_gizmos::FfiGizmoLine";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiGizmoRay {
    const NAME: &'static CStr = c"bevy_gizmos::FfiGizmoRay";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiGizmoCircle {
    const NAME: &'static CStr = c"bevy_gizmos::FfiGizmoCircle";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiGizmoSphere {
    const NAME: &'static CStr = c"bevy_gizmos::FfiGizmoSphere";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiGizmoRect {
    const NAME: &'static CStr = c"bevy_gizmos::FfiGizmoRect";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiGizmoCuboid {
    const NAME: &'static CStr = c"bevy_gizmos::FfiGizmoCuboid";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiGizmoArrow {
    const NAME: &'static CStr = c"bevy_gizmos::FfiGizmoArrow";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}

/// Extension trait on [`GuestCommands`] emitting immediate Bevy `0.20` debug gizmo draw events.
pub trait GuestCommandsGizmosExt {
    /// Queues a 3D/2D line segment gizmo from `start` to `end`.
    fn gizmo_line(
        &mut self,
        start: [f32; 3],
        end: [f32; 3],
        color: FfiColor,
    ) -> Result<(), FfiResult>;
    /// Queues a ray gizmo from `start` along `vector`.
    fn gizmo_ray(
        &mut self,
        start: [f32; 3],
        vector: [f32; 3],
        color: FfiColor,
    ) -> Result<(), FfiResult>;
    /// Queues a circle gizmo centered at `center` with plane `normal` and `radius`.
    fn gizmo_circle(
        &mut self,
        center: [f32; 3],
        normal: [f32; 3],
        radius: f32,
        color: FfiColor,
    ) -> Result<(), FfiResult>;
    /// Queues a wireframe sphere gizmo at `center` with `radius`.
    fn gizmo_sphere(
        &mut self,
        center: [f32; 3],
        radius: f32,
        color: FfiColor,
    ) -> Result<(), FfiResult>;
    /// Queues an oriented rectangle outline gizmo.
    fn gizmo_rect(
        &mut self,
        center: [f32; 3],
        rotation: [f32; 4],
        size: [f32; 2],
        color: FfiColor,
    ) -> Result<(), FfiResult>;
    /// Queues an oriented wireframe cuboid gizmo.
    fn gizmo_cuboid(
        &mut self,
        center: [f32; 3],
        rotation: [f32; 4],
        size: [f32; 3],
        color: FfiColor,
    ) -> Result<(), FfiResult>;
    /// Queues an arrow gizmo from `start` to `end`.
    fn gizmo_arrow(
        &mut self,
        start: [f32; 3],
        end: [f32; 3],
        color: FfiColor,
    ) -> Result<(), FfiResult>;
}

impl GuestCommandsGizmosExt for GuestCommands<'_> {
    #[inline]
    fn gizmo_line(
        &mut self,
        start: [f32; 3],
        end: [f32; 3],
        color: FfiColor,
    ) -> Result<(), FfiResult> {
        self.trigger(&FfiGizmoLine { start, end, color })
    }

    #[inline]
    fn gizmo_ray(
        &mut self,
        start: [f32; 3],
        vector: [f32; 3],
        color: FfiColor,
    ) -> Result<(), FfiResult> {
        self.trigger(&FfiGizmoRay {
            start,
            vector,
            color,
        })
    }

    #[inline]
    fn gizmo_circle(
        &mut self,
        center: [f32; 3],
        normal: [f32; 3],
        radius: f32,
        color: FfiColor,
    ) -> Result<(), FfiResult> {
        self.trigger(&FfiGizmoCircle {
            center,
            normal,
            radius,
            color,
        })
    }

    #[inline]
    fn gizmo_sphere(
        &mut self,
        center: [f32; 3],
        radius: f32,
        color: FfiColor,
    ) -> Result<(), FfiResult> {
        self.trigger(&FfiGizmoSphere {
            center,
            radius,
            color,
        })
    }

    #[inline]
    fn gizmo_rect(
        &mut self,
        center: [f32; 3],
        rotation: [f32; 4],
        size: [f32; 2],
        color: FfiColor,
    ) -> Result<(), FfiResult> {
        self.trigger(&FfiGizmoRect {
            center,
            rotation,
            size,
            color,
        })
    }

    #[inline]
    fn gizmo_cuboid(
        &mut self,
        center: [f32; 3],
        rotation: [f32; 4],
        size: [f32; 3],
        color: FfiColor,
    ) -> Result<(), FfiResult> {
        self.trigger(&FfiGizmoCuboid {
            center,
            rotation,
            size,
            color,
        })
    }

    #[inline]
    fn gizmo_arrow(
        &mut self,
        start: [f32; 3],
        end: [f32; 3],
        color: FfiColor,
    ) -> Result<(), FfiResult> {
        self.trigger(&FfiGizmoArrow {
            start,
            end,
            tip_length: 0.0,
            color,
        })
    }
}
