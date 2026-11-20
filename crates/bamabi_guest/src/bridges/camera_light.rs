//! Guest-side `#![forbid(unsafe_code)]`, zero-Bevy bridge for Bevy `0.20` cameras, tonemapping,
//! lights, `PanOrbitCamera`, and `SolariLighting` matching `bamabi_bridges::camera_light`
//! (`ModCameraLightBridgePlugin`) bit-for-bit.

use bamabi_sys::{AccessMode, FfiEntity, FfiResult};
use core::ffi::CStr;

pub use bamabi_guest_sys::wire_types::{
    CLEAR_COLOR_CUSTOM, CLEAR_COLOR_DEFAULT, CLEAR_COLOR_NONE, DEPTH_LOAD_OP_CLEAR,
    DEPTH_LOAD_OP_LOAD, FfiCamera2d, FfiCamera3d, FfiDirectionalLight, FfiPanOrbitCamera,
    FfiPointLight, FfiSolariLighting, FfiSpotLight, ORBIT_CONSTRAINT_FIXED, ORBIT_CONSTRAINT_FREE,
    TONEMAPPING_ACES_FITTED, TONEMAPPING_AGX, TONEMAPPING_BLENDER_FILMIC,
    TONEMAPPING_KHRONOS_PBR_NEUTRAL, TONEMAPPING_LINEAR, TONEMAPPING_NONE, TONEMAPPING_REINHARD,
    TONEMAPPING_REINHARD_LUMINANCE, TONEMAPPING_SOMEWHAT_BORING_DISPLAY_TRANSFORM,
    TONEMAPPING_TONY_MC_MAPFACE, TRANSMISSION_QUALITY_HIGH, TRANSMISSION_QUALITY_LOW,
    TRANSMISSION_QUALITY_MEDIUM, TRANSMISSION_QUALITY_ULTRA,
};

use crate::{app::QueryBuilder, commands::GuestCommands, component::GuestComponent};

/// Canonical ABI name for [`FfiCamera2d`].
pub const CANONICAL_CAMERA_2D_NAME: &str = "bamabi::camera_light::FfiCamera2d";
/// Canonical ABI name for [`FfiCamera3d`].
pub const CANONICAL_CAMERA_3D_NAME: &str = "bamabi::camera_light::FfiCamera3d";
/// Canonical ABI name for [`FfiPointLight`].
pub const CANONICAL_POINT_LIGHT_NAME: &str = "bamabi::camera_light::FfiPointLight";
/// Canonical ABI name for [`FfiSpotLight`].
pub const CANONICAL_SPOT_LIGHT_NAME: &str = "bamabi::camera_light::FfiSpotLight";
/// Canonical ABI name for [`FfiDirectionalLight`].
pub const CANONICAL_DIRECTIONAL_LIGHT_NAME: &str = "bamabi::camera_light::FfiDirectionalLight";
/// Canonical ABI name for [`FfiPanOrbitCamera`].
pub const CANONICAL_PAN_ORBIT_CAMERA_NAME: &str = "bamabi::camera_light::FfiPanOrbitCamera";
/// Canonical ABI name for [`FfiSolariLighting`].
pub const CANONICAL_SOLARI_LIGHTING_NAME: &str = "bamabi::camera_light::FfiSolariLighting";

impl GuestComponent for FfiCamera2d {
    const NAME: &'static CStr = c"bamabi::camera_light::FfiCamera2d";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiCamera3d {
    const NAME: &'static CStr = c"bamabi::camera_light::FfiCamera3d";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiPointLight {
    const NAME: &'static CStr = c"bamabi::camera_light::FfiPointLight";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiSpotLight {
    const NAME: &'static CStr = c"bamabi::camera_light::FfiSpotLight";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiDirectionalLight {
    const NAME: &'static CStr = c"bamabi::camera_light::FfiDirectionalLight";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiPanOrbitCamera {
    const NAME: &'static CStr = c"bamabi::camera_light::FfiPanOrbitCamera";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiSolariLighting {
    const NAME: &'static CStr = c"bamabi::camera_light::FfiSolariLighting";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}

/// Extension trait on [`QueryBuilder`] adding direct POD Bevy `0.20` camera and light query terms.
pub trait GuestCameraLightQueryBuilderExt {
    /// Appends a shared read (`&FfiCamera2d`) term.
    fn read_camera_2d(self) -> Self;
    /// Appends an exclusive write (`&mut FfiCamera2d`) term.
    fn write_camera_2d(self) -> Self;
    /// Appends a shared read (`&FfiCamera3d`) term.
    fn read_camera_3d(self) -> Self;
    /// Appends an exclusive write (`&mut FfiCamera3d`) term.
    fn write_camera_3d(self) -> Self;
    /// Appends a shared read (`&FfiPointLight`) term.
    fn read_point_light(self) -> Self;
    /// Appends an exclusive write (`&mut FfiPointLight`) term.
    fn write_point_light(self) -> Self;
    /// Appends a shared read (`&FfiSpotLight`) term.
    fn read_spot_light(self) -> Self;
    /// Appends an exclusive write (`&mut FfiSpotLight`) term.
    fn write_spot_light(self) -> Self;
    /// Appends a shared read (`&FfiDirectionalLight`) term.
    fn read_directional_light(self) -> Self;
    /// Appends an exclusive write (`&mut FfiDirectionalLight`) term.
    fn write_directional_light(self) -> Self;
    /// Appends a shared read (`&FfiPanOrbitCamera`) term.
    fn read_pan_orbit_camera(self) -> Self;
    /// Appends an exclusive write (`&mut FfiPanOrbitCamera`) term.
    fn write_pan_orbit_camera(self) -> Self;
    /// Appends a shared read (`&FfiSolariLighting`) term.
    fn read_solari_lighting(self) -> Self;
    /// Appends an exclusive write (`&mut FfiSolariLighting`) term.
    fn write_solari_lighting(self) -> Self;
}

impl GuestCameraLightQueryBuilderExt for QueryBuilder<'_> {
    #[inline]
    fn read_camera_2d(self) -> Self {
        self.term_named(FfiCamera2d::NAME, Some(FfiCamera2d::UUID), AccessMode::READ)
    }

    #[inline]
    fn write_camera_2d(self) -> Self {
        self.term_named(
            FfiCamera2d::NAME,
            Some(FfiCamera2d::UUID),
            AccessMode::WRITE,
        )
    }

    #[inline]
    fn read_camera_3d(self) -> Self {
        self.term_named(FfiCamera3d::NAME, Some(FfiCamera3d::UUID), AccessMode::READ)
    }

    #[inline]
    fn write_camera_3d(self) -> Self {
        self.term_named(
            FfiCamera3d::NAME,
            Some(FfiCamera3d::UUID),
            AccessMode::WRITE,
        )
    }

    #[inline]
    fn read_point_light(self) -> Self {
        self.term_named(
            FfiPointLight::NAME,
            Some(FfiPointLight::UUID),
            AccessMode::READ,
        )
    }

    #[inline]
    fn write_point_light(self) -> Self {
        self.term_named(
            FfiPointLight::NAME,
            Some(FfiPointLight::UUID),
            AccessMode::WRITE,
        )
    }

    #[inline]
    fn read_spot_light(self) -> Self {
        self.term_named(
            FfiSpotLight::NAME,
            Some(FfiSpotLight::UUID),
            AccessMode::READ,
        )
    }

    #[inline]
    fn write_spot_light(self) -> Self {
        self.term_named(
            FfiSpotLight::NAME,
            Some(FfiSpotLight::UUID),
            AccessMode::WRITE,
        )
    }

    #[inline]
    fn read_directional_light(self) -> Self {
        self.term_named(
            FfiDirectionalLight::NAME,
            Some(FfiDirectionalLight::UUID),
            AccessMode::READ,
        )
    }

    #[inline]
    fn write_directional_light(self) -> Self {
        self.term_named(
            FfiDirectionalLight::NAME,
            Some(FfiDirectionalLight::UUID),
            AccessMode::WRITE,
        )
    }

    #[inline]
    fn read_pan_orbit_camera(self) -> Self {
        self.term_named(
            FfiPanOrbitCamera::NAME,
            Some(FfiPanOrbitCamera::UUID),
            AccessMode::READ,
        )
    }

    #[inline]
    fn write_pan_orbit_camera(self) -> Self {
        self.term_named(
            FfiPanOrbitCamera::NAME,
            Some(FfiPanOrbitCamera::UUID),
            AccessMode::WRITE,
        )
    }

    #[inline]
    fn read_solari_lighting(self) -> Self {
        self.term_named(
            FfiSolariLighting::NAME,
            Some(FfiSolariLighting::UUID),
            AccessMode::READ,
        )
    }

    #[inline]
    fn write_solari_lighting(self) -> Self {
        self.term_named(
            FfiSolariLighting::NAME,
            Some(FfiSolariLighting::UUID),
            AccessMode::WRITE,
        )
    }
}

/// Extension trait on [`GuestCommands`] for inserting bridged Bevy `0.20` cameras and lights.
pub trait GuestCommandsCameraLightExt {
    /// Inserts [`FfiCamera2d`] onto `entity`.
    fn insert_camera_2d(&mut self, entity: FfiEntity, camera: &FfiCamera2d) -> FfiResult;
    /// Inserts [`FfiCamera3d`] onto `entity`.
    fn insert_camera_3d(&mut self, entity: FfiEntity, camera: &FfiCamera3d) -> FfiResult;
    /// Inserts [`FfiPointLight`] onto `entity`.
    fn insert_point_light(&mut self, entity: FfiEntity, light: &FfiPointLight) -> FfiResult;
    /// Inserts [`FfiSpotLight`] onto `entity`.
    fn insert_spot_light(&mut self, entity: FfiEntity, light: &FfiSpotLight) -> FfiResult;
    /// Inserts [`FfiDirectionalLight`] onto `entity`.
    fn insert_directional_light(
        &mut self,
        entity: FfiEntity,
        light: &FfiDirectionalLight,
    ) -> FfiResult;
    /// Inserts [`FfiPanOrbitCamera`] onto `entity`.
    fn insert_pan_orbit_camera(
        &mut self,
        entity: FfiEntity,
        controller: &FfiPanOrbitCamera,
    ) -> FfiResult;
    /// Inserts [`FfiSolariLighting`] onto `entity`.
    fn insert_solari_lighting(
        &mut self,
        entity: FfiEntity,
        solari: &FfiSolariLighting,
    ) -> FfiResult;
}

impl GuestCommandsCameraLightExt for GuestCommands<'_> {
    #[inline]
    fn insert_camera_2d(&mut self, entity: FfiEntity, camera: &FfiCamera2d) -> FfiResult {
        self.insert_component(entity, camera)
    }

    #[inline]
    fn insert_camera_3d(&mut self, entity: FfiEntity, camera: &FfiCamera3d) -> FfiResult {
        self.insert_component(entity, camera)
    }

    #[inline]
    fn insert_point_light(&mut self, entity: FfiEntity, light: &FfiPointLight) -> FfiResult {
        self.insert_component(entity, light)
    }

    #[inline]
    fn insert_spot_light(&mut self, entity: FfiEntity, light: &FfiSpotLight) -> FfiResult {
        self.insert_component(entity, light)
    }

    #[inline]
    fn insert_directional_light(
        &mut self,
        entity: FfiEntity,
        light: &FfiDirectionalLight,
    ) -> FfiResult {
        self.insert_component(entity, light)
    }

    #[inline]
    fn insert_pan_orbit_camera(
        &mut self,
        entity: FfiEntity,
        controller: &FfiPanOrbitCamera,
    ) -> FfiResult {
        self.insert_component(entity, controller)
    }

    #[inline]
    fn insert_solari_lighting(
        &mut self,
        entity: FfiEntity,
        solari: &FfiSolariLighting,
    ) -> FfiResult {
        self.insert_component(entity, solari)
    }
}
