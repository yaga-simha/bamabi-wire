//! Bevy `0.20` camera, tonemapping, lighting, `PanOrbitCamera`, and `SolariLighting` wire mirror
//! types matching `bamabi_bridges::camera_light` byte-for-byte.

use bamabi_sys::FfiBool;

/// `ClearColorConfig::Default` discriminant (`0`).
pub const CLEAR_COLOR_DEFAULT: u8 = 0;
/// `ClearColorConfig::Custom(Color)` discriminant (`1`).
pub const CLEAR_COLOR_CUSTOM: u8 = 1;
/// `ClearColorConfig::None` discriminant (`2`).
pub const CLEAR_COLOR_NONE: u8 = 2;

/// `Camera3dDepthLoadOp::Clear(f32)` discriminant (`0`).
pub const DEPTH_LOAD_OP_CLEAR: u8 = 0;
/// `Camera3dDepthLoadOp::Load` discriminant (`1`).
pub const DEPTH_LOAD_OP_LOAD: u8 = 1;

/// `Tonemapping::None` discriminant (`0`).
pub const TONEMAPPING_NONE: u8 = 0;
/// `Tonemapping::Linear` discriminant (`1`).
pub const TONEMAPPING_LINEAR: u8 = 1;
/// `Tonemapping::Reinhard` discriminant (`2`).
pub const TONEMAPPING_REINHARD: u8 = 2;
/// `Tonemapping::ReinhardLuminance` discriminant (`3`).
pub const TONEMAPPING_REINHARD_LUMINANCE: u8 = 3;
/// `Tonemapping::AcesFitted` discriminant (`4`).
pub const TONEMAPPING_ACES_FITTED: u8 = 4;
/// `Tonemapping::AgX` discriminant (`5`).
pub const TONEMAPPING_AGX: u8 = 5;
/// `Tonemapping::SomewhatBoringDisplayTransform` discriminant (`6`).
pub const TONEMAPPING_SOMEWHAT_BORING_DISPLAY_TRANSFORM: u8 = 6;
/// `Tonemapping::TonyMcMapface` discriminant (`7`, Bevy default).
pub const TONEMAPPING_TONY_MC_MAPFACE: u8 = 7;
/// `Tonemapping::BlenderFilmic` discriminant (`8`).
pub const TONEMAPPING_BLENDER_FILMIC: u8 = 8;
/// `Tonemapping::KhronosPbrNeutral` discriminant (`9`).
pub const TONEMAPPING_KHRONOS_PBR_NEUTRAL: u8 = 9;

/// `ScreenSpaceTransmissionQuality::Low` discriminant (`0`).
pub const TRANSMISSION_QUALITY_LOW: u8 = 0;
/// `ScreenSpaceTransmissionQuality::Medium` discriminant (`1`).
pub const TRANSMISSION_QUALITY_MEDIUM: u8 = 1;
/// `ScreenSpaceTransmissionQuality::High` discriminant (`2`).
pub const TRANSMISSION_QUALITY_HIGH: u8 = 2;
/// `ScreenSpaceTransmissionQuality::Ultra` discriminant (`3`).
pub const TRANSMISSION_QUALITY_ULTRA: u8 = 3;

/// `OrbitConstraint::Fixed` discriminant (`0`).
pub const ORBIT_CONSTRAINT_FIXED: u8 = 0;
/// `OrbitConstraint::Free` discriminant (`1`).
pub const ORBIT_CONSTRAINT_FREE: u8 = 1;

/// Zero-padding `#[repr(C)]` wire mirror of Bevy `Camera2d` + `Camera` + `Hdr`
/// (`"bamabi::camera_light::FfiCamera2d"`, 24 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiCamera2d {
    /// Camera render order (`Camera::order`; higher orders render later / on top).
    pub order: i32,
    /// Clear color linear RGBA `[r, g, b, a]` when `clear_color_mode == CLEAR_COLOR_CUSTOM`.
    pub clear_color: [f32; 4],
    /// Whether this camera is active (`Camera::is_active`).
    pub is_active: FfiBool,
    /// Clear color mode discriminant (`CLEAR_COLOR_DEFAULT`, `CLEAR_COLOR_CUSTOM`, `CLEAR_COLOR_NONE`).
    pub clear_color_mode: u8,
    /// Whether face culling is inverted (`Camera::invert_culling`).
    pub invert_culling: FfiBool,
    /// Whether Bevy's `Hdr` marker component is attached to this camera entity.
    pub hdr: FfiBool,
}

impl_wire_abi_pod!(
    FfiCamera2d,
    size = 24,
    align = 4,
    schema = [
        order: i32 => "i32",
        clear_color: [f32; 4] => "[f32;4]",
        is_active: FfiBool => "FfiBool",
        clear_color_mode: u8 => "u8",
        invert_culling: FfiBool => "FfiBool",
        hdr: FfiBool => "FfiBool",
    ]
);

impl Default for FfiCamera2d {
    #[inline]
    fn default() -> Self {
        Self {
            order: 0,
            clear_color: [0.0, 0.0, 0.0, 1.0],
            is_active: FfiBool::TRUE,
            clear_color_mode: CLEAR_COLOR_DEFAULT,
            invert_culling: FfiBool::FALSE,
            hdr: FfiBool::FALSE,
        }
    }
}

/// Zero-padding `#[repr(C)]` wire mirror of Bevy `Camera3d` + `Camera` + `Tonemapping` +
/// `DebandDither` + `ScreenSpaceTransmission` + `Exposure` + `Hdr`
/// (`"bamabi::camera_light::FfiCamera3d"`, 48 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiCamera3d {
    /// Camera render order (`Camera::order`).
    pub order: i32,
    /// Clear color linear RGBA `[r, g, b, a]` when `clear_color_mode == CLEAR_COLOR_CUSTOM`.
    pub clear_color: [f32; 4],
    /// Reverse-Z depth clear value when `depth_load_op == DEPTH_LOAD_OP_CLEAR` (default `0.0`).
    pub depth_clear_value: f32,
    /// `Camera3d::depth_texture_usages` bitflags (`wgpu_types::TextureUsages::bits()`).
    pub depth_texture_usages: u32,
    /// `Exposure::ev100` (default `Exposure::EV100_BLENDER` = `9.7`).
    pub exposure_ev100: f32,
    /// `ScreenSpaceTransmission::steps` when `transmission_enabled` is true (default `1`).
    pub transmission_steps: u32,
    /// Whether this camera is active (`Camera::is_active`).
    pub is_active: FfiBool,
    /// Clear color mode discriminant (`CLEAR_COLOR_DEFAULT`, `CLEAR_COLOR_CUSTOM`, `CLEAR_COLOR_NONE`).
    pub clear_color_mode: u8,
    /// Whether face culling is inverted (`Camera::invert_culling`).
    pub invert_culling: FfiBool,
    /// Whether Bevy's `Hdr` marker component is attached to this camera entity.
    pub hdr: FfiBool,
    /// Depth load op discriminant (`DEPTH_LOAD_OP_CLEAR` or `DEPTH_LOAD_OP_LOAD`).
    pub depth_load_op: u8,
    /// `Tonemapping` discriminant (`TONEMAPPING_NONE..=TONEMAPPING_KHRONOS_PBR_NEUTRAL`).
    pub tonemapping: u8,
    /// `DebandDither` flag (`FfiBool::FALSE` = `Disabled`, `FfiBool::TRUE` = `Enabled`).
    pub deband_dither: FfiBool,
    /// Whether `ScreenSpaceTransmission` is attached to this camera entity.
    pub transmission_enabled: FfiBool,
    /// `ScreenSpaceTransmissionQuality` discriminant (`TRANSMISSION_QUALITY_LOW..=ULTRA`).
    pub transmission_quality: u8,
    /// Explicit zero-reserved alignment padding bytes.
    pub reserved0: [u8; 3],
}

impl_wire_abi_pod!(
    FfiCamera3d,
    size = 48,
    align = 4,
    schema = [
        order: i32 => "i32",
        clear_color: [f32; 4] => "[f32;4]",
        depth_clear_value: f32 => "f32",
        depth_texture_usages: u32 => "u32",
        exposure_ev100: f32 => "f32",
        transmission_steps: u32 => "u32",
        is_active: FfiBool => "FfiBool",
        clear_color_mode: u8 => "u8",
        invert_culling: FfiBool => "FfiBool",
        hdr: FfiBool => "FfiBool",
        depth_load_op: u8 => "u8",
        tonemapping: u8 => "u8",
        deband_dither: FfiBool => "FfiBool",
        transmission_enabled: FfiBool => "FfiBool",
        transmission_quality: u8 => "u8",
        reserved0: [u8; 3] => "[u8;3]",
    ]
);

impl Default for FfiCamera3d {
    #[inline]
    fn default() -> Self {
        Self {
            order: 0,
            clear_color: [0.0, 0.0, 0.0, 1.0],
            depth_clear_value: 0.0,
            depth_texture_usages: 16, // wgpu RENDER_ATTACHMENT
            exposure_ev100: 9.7,
            transmission_steps: 1,
            is_active: FfiBool::TRUE,
            clear_color_mode: CLEAR_COLOR_DEFAULT,
            invert_culling: FfiBool::FALSE,
            hdr: FfiBool::FALSE,
            depth_load_op: DEPTH_LOAD_OP_CLEAR,
            tonemapping: TONEMAPPING_TONY_MC_MAPFACE,
            deband_dither: FfiBool::FALSE,
            transmission_enabled: FfiBool::FALSE,
            transmission_quality: TRANSMISSION_QUALITY_MEDIUM,
            reserved0: [0; 3],
        }
    }
}

/// Zero-padding `#[repr(C)]` wire mirror of Bevy `PointLight`
/// (`"bamabi::camera_light::FfiPointLight"`, 44 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiPointLight {
    /// Linear RGBA color `[r, g, b, a]`.
    pub color: [f32; 4],
    /// Luminous power in lumens.
    pub intensity: f32,
    /// Cut-off range in meters.
    pub range: f32,
    /// Spherical emitter radius in meters.
    pub radius: f32,
    /// Shadow map depth bias.
    pub shadow_depth_bias: f32,
    /// Shadow map normal bias.
    pub shadow_normal_bias: f32,
    /// Shadow map near-Z plane distance.
    pub shadow_map_near_z: f32,
    /// Whether shadow maps are enabled (`PointLight::shadow_maps_enabled`).
    pub shadow_maps_enabled: FfiBool,
    /// Whether contact shadows are enabled (`PointLight::contact_shadows_enabled`).
    pub contact_shadows_enabled: FfiBool,
    /// Whether this light contributes diffuse lighting to lightmapped meshes.
    pub affects_lightmapped_mesh_diffuse: FfiBool,
    /// Explicit zero-reserved alignment byte.
    pub reserved0: u8,
}

impl_wire_abi_pod!(
    FfiPointLight,
    size = 44,
    align = 4,
    schema = [
        color: [f32; 4] => "[f32;4]",
        intensity: f32 => "f32",
        range: f32 => "f32",
        radius: f32 => "f32",
        shadow_depth_bias: f32 => "f32",
        shadow_normal_bias: f32 => "f32",
        shadow_map_near_z: f32 => "f32",
        shadow_maps_enabled: FfiBool => "FfiBool",
        contact_shadows_enabled: FfiBool => "FfiBool",
        affects_lightmapped_mesh_diffuse: FfiBool => "FfiBool",
        reserved0: u8 => "u8",
    ]
);

impl Default for FfiPointLight {
    #[inline]
    fn default() -> Self {
        Self {
            color: [1.0, 1.0, 1.0, 1.0],
            intensity: 1_000_000.0,
            range: 20.0,
            radius: 0.0,
            shadow_depth_bias: 0.08,
            shadow_normal_bias: 0.6,
            shadow_map_near_z: 0.1,
            shadow_maps_enabled: FfiBool::FALSE,
            contact_shadows_enabled: FfiBool::FALSE,
            affects_lightmapped_mesh_diffuse: FfiBool::TRUE,
            reserved0: 0,
        }
    }
}

/// Zero-padding `#[repr(C)]` wire mirror of Bevy `SpotLight`
/// (`"bamabi::camera_light::FfiSpotLight"`, 52 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiSpotLight {
    /// Linear RGBA color `[r, g, b, a]`.
    pub color: [f32; 4],
    /// Luminous power in lumens.
    pub intensity: f32,
    /// Illumination range in meters.
    pub range: f32,
    /// Spherical emitter radius in meters.
    pub radius: f32,
    /// Shadow map depth bias.
    pub shadow_depth_bias: f32,
    /// Shadow map normal bias.
    pub shadow_normal_bias: f32,
    /// Shadow map near-Z plane distance.
    pub shadow_map_near_z: f32,
    /// Outer cone half-angle in radians (`< PI / 2`).
    pub outer_angle: f32,
    /// Inner cone half-angle in radians (`<= outer_angle`).
    pub inner_angle: f32,
    /// Whether shadow maps are enabled (`SpotLight::shadow_maps_enabled`).
    pub shadow_maps_enabled: FfiBool,
    /// Whether contact shadows are enabled (`SpotLight::contact_shadows_enabled`).
    pub contact_shadows_enabled: FfiBool,
    /// Whether this light contributes diffuse lighting to lightmapped meshes.
    pub affects_lightmapped_mesh_diffuse: FfiBool,
    /// Explicit zero-reserved alignment byte.
    pub reserved0: u8,
}

impl_wire_abi_pod!(
    FfiSpotLight,
    size = 52,
    align = 4,
    schema = [
        color: [f32; 4] => "[f32;4]",
        intensity: f32 => "f32",
        range: f32 => "f32",
        radius: f32 => "f32",
        shadow_depth_bias: f32 => "f32",
        shadow_normal_bias: f32 => "f32",
        shadow_map_near_z: f32 => "f32",
        outer_angle: f32 => "f32",
        inner_angle: f32 => "f32",
        shadow_maps_enabled: FfiBool => "FfiBool",
        contact_shadows_enabled: FfiBool => "FfiBool",
        affects_lightmapped_mesh_diffuse: FfiBool => "FfiBool",
        reserved0: u8 => "u8",
    ]
);

impl Default for FfiSpotLight {
    #[inline]
    fn default() -> Self {
        Self {
            color: [1.0, 1.0, 1.0, 1.0],
            intensity: 1_000_000.0,
            range: 20.0,
            radius: 0.0,
            shadow_depth_bias: 0.02,
            shadow_normal_bias: 1.8,
            shadow_map_near_z: 0.1,
            outer_angle: core::f32::consts::FRAC_PI_4,
            inner_angle: 0.0,
            shadow_maps_enabled: FfiBool::FALSE,
            contact_shadows_enabled: FfiBool::FALSE,
            affects_lightmapped_mesh_diffuse: FfiBool::TRUE,
            reserved0: 0,
        }
    }
}

/// Zero-padding `#[repr(C)]` wire mirror of Bevy `DirectionalLight`
/// (`"bamabi::camera_light::FfiDirectionalLight"`, 32 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiDirectionalLight {
    /// Linear RGBA color `[r, g, b, a]`.
    pub color: [f32; 4],
    /// Illuminance in lux.
    pub illuminance: f32,
    /// Cascaded shadow map depth bias.
    pub shadow_depth_bias: f32,
    /// Cascaded shadow map normal bias.
    pub shadow_normal_bias: f32,
    /// Whether shadow maps are enabled (`DirectionalLight::shadow_maps_enabled`).
    pub shadow_maps_enabled: FfiBool,
    /// Whether contact shadows are enabled (`DirectionalLight::contact_shadows_enabled`).
    pub contact_shadows_enabled: FfiBool,
    /// Whether this light contributes diffuse lighting to lightmapped meshes.
    pub affects_lightmapped_mesh_diffuse: FfiBool,
    /// Explicit zero-reserved alignment byte.
    pub reserved0: u8,
}

impl_wire_abi_pod!(
    FfiDirectionalLight,
    size = 32,
    align = 4,
    schema = [
        color: [f32; 4] => "[f32;4]",
        illuminance: f32 => "f32",
        shadow_depth_bias: f32 => "f32",
        shadow_normal_bias: f32 => "f32",
        shadow_maps_enabled: FfiBool => "FfiBool",
        contact_shadows_enabled: FfiBool => "FfiBool",
        affects_lightmapped_mesh_diffuse: FfiBool => "FfiBool",
        reserved0: u8 => "u8",
    ]
);

impl Default for FfiDirectionalLight {
    #[inline]
    fn default() -> Self {
        Self {
            color: [1.0, 1.0, 1.0, 1.0],
            illuminance: 10_000.0,
            shadow_depth_bias: 0.02,
            shadow_normal_bias: 1.8,
            shadow_maps_enabled: FfiBool::FALSE,
            contact_shadows_enabled: FfiBool::FALSE,
            affects_lightmapped_mesh_diffuse: FfiBool::TRUE,
            reserved0: 0,
        }
    }
}

/// Zero-padding `#[repr(C)]` wire mirror of Bevy `0.20`'s `PanOrbitCamera`
/// (`"bamabi::camera_light::FfiPanOrbitCamera"`, 136 bytes, align 8 on 64-bit / 4 on i686).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiPanOrbitCamera {
    /// Fallback anchor depth along camera view space Z (`PanOrbitCamera::last_anchor_depth`).
    pub last_anchor_depth: f64,
    /// Minimum world units per pixel when zooming in (`ZoomLimits::min_size_per_pixel`).
    pub zoom_min_size_per_pixel: f64,
    /// Maximum world units per pixel when zooming out (`ZoomLimits::max_size_per_pixel`).
    pub zoom_max_size_per_pixel: f64,
    /// Fixed up-axis unit vector `[x, y, z]` when `orbit_constraint_mode == ORBIT_CONSTRAINT_FIXED`.
    pub orbit_up: [f64; 3],
    /// Constant pan momentum friction (`Momentum::pan_friction`).
    pub momentum_pan_friction: f64,
    /// Constant orbit momentum friction (`Momentum::orbit_friction`).
    pub momentum_orbit_friction: f64,
    /// Pan input smoothing window in seconds (`Smoothing::pan`).
    pub smoothing_pan_secs: f32,
    /// Orbit input smoothing window in seconds (`Smoothing::orbit`).
    pub smoothing_orbit_secs: f32,
    /// Zoom input smoothing window in seconds (`Smoothing::zoom`).
    pub smoothing_zoom_secs: f32,
    /// Orbit input sensitivity `[x, y]` (`Sensitivity::orbit`).
    pub sensitivity_orbit: [f32; 2],
    /// Zoom input sensitivity (`Sensitivity::zoom`).
    pub sensitivity_zoom: f32,
    /// Pan momentum initial sampling window in seconds (`Momentum::init_pan`).
    pub momentum_init_pan_secs: f32,
    /// Orbit momentum initial sampling window in seconds (`Momentum::init_orbit`).
    pub momentum_init_orbit_secs: f32,
    /// Debounce window in seconds before starting a new motion (`PanOrbitCamera::input_debounce`).
    pub input_debounce_secs: f32,
    /// Lower bound of perspective near-clip range (`PerspectiveSettings::near_clip_limits.start`).
    pub perspective_near_clip_min: f32,
    /// Upper bound of perspective near-clip range (`PerspectiveSettings::near_clip_limits.end`).
    pub perspective_near_clip_max: f32,
    /// Perspective near-clip multiplier (`PerspectiveSettings::near_clip_multiplier`).
    pub perspective_near_clip_multiplier: f32,
    /// Orthographic scale-to-near-clip factor (`OrthographicSettings::scale_to_near_clip`).
    pub ortho_scale_to_near_clip: f32,
    /// Lower bound of orthographic near-clip range (`OrthographicSettings::near_clip_limits.start`).
    pub ortho_near_clip_min: f32,
    /// Upper bound of orthographic near-clip range (`OrthographicSettings::near_clip_limits.end`).
    pub ortho_near_clip_max: f32,
    /// Orthographic far-clip multiplier (`OrthographicSettings::far_clip_multiplier`).
    pub ortho_far_clip_multiplier: f32,
    /// Whether panning is enabled (`EnabledMotion::pan`).
    pub enabled_pan: FfiBool,
    /// Whether orbiting is enabled (`EnabledMotion::orbit`).
    pub enabled_orbit: FfiBool,
    /// Whether zooming is enabled (`EnabledMotion::zoom`).
    pub enabled_zoom: FfiBool,
    /// Orbit constraint discriminant (`ORBIT_CONSTRAINT_FIXED` or `ORBIT_CONSTRAINT_FREE`).
    pub orbit_constraint_mode: u8,
    /// Whether fixed-up orbit may pass over top dead center (`OrbitConstraint::Fixed::can_pass_tdc`).
    pub orbit_can_pass_tdc: FfiBool,
    /// Whether zooming in may pass through objects (`ZoomLimits::zoom_through_objects`).
    pub zoom_through_objects: FfiBool,
    /// Velocity-proportional pan momentum damping (`Momentum::pan_damping`).
    pub momentum_pan_damping: u8,
    /// Velocity-proportional orbit momentum damping (`Momentum::orbit_damping`).
    pub momentum_orbit_damping: u8,
}

impl_wire_abi_pod!(
    FfiPanOrbitCamera,
    size = 136,
    align = core::mem::align_of::<f64>(),
    schema = [
        last_anchor_depth: f64 => "f64",
        zoom_min_size_per_pixel: f64 => "f64",
        zoom_max_size_per_pixel: f64 => "f64",
        orbit_up: [f64; 3] => "[f64;3]",
        momentum_pan_friction: f64 => "f64",
        momentum_orbit_friction: f64 => "f64",
        smoothing_pan_secs: f32 => "f32",
        smoothing_orbit_secs: f32 => "f32",
        smoothing_zoom_secs: f32 => "f32",
        sensitivity_orbit: [f32; 2] => "[f32;2]",
        sensitivity_zoom: f32 => "f32",
        momentum_init_pan_secs: f32 => "f32",
        momentum_init_orbit_secs: f32 => "f32",
        input_debounce_secs: f32 => "f32",
        perspective_near_clip_min: f32 => "f32",
        perspective_near_clip_max: f32 => "f32",
        perspective_near_clip_multiplier: f32 => "f32",
        ortho_scale_to_near_clip: f32 => "f32",
        ortho_near_clip_min: f32 => "f32",
        ortho_near_clip_max: f32 => "f32",
        ortho_far_clip_multiplier: f32 => "f32",
        enabled_pan: FfiBool => "FfiBool",
        enabled_orbit: FfiBool => "FfiBool",
        enabled_zoom: FfiBool => "FfiBool",
        orbit_constraint_mode: u8 => "u8",
        orbit_can_pass_tdc: FfiBool => "FfiBool",
        zoom_through_objects: FfiBool => "FfiBool",
        momentum_pan_damping: u8 => "u8",
        momentum_orbit_damping: u8 => "u8",
    ]
);

impl Default for FfiPanOrbitCamera {
    #[inline]
    fn default() -> Self {
        Self {
            last_anchor_depth: -2.0,
            zoom_min_size_per_pixel: 1e-6,
            zoom_max_size_per_pixel: 1e27,
            orbit_up: [0.0, 1.0, 0.0],
            momentum_pan_friction: 0.2,
            momentum_orbit_friction: 0.2,
            smoothing_pan_secs: core::time::Duration::from_millis(10).as_secs_f32(),
            smoothing_orbit_secs: core::time::Duration::from_millis(30).as_secs_f32(),
            smoothing_zoom_secs: core::time::Duration::from_millis(60).as_secs_f32(),
            sensitivity_orbit: [1.0, 1.0],
            sensitivity_zoom: 1.0,
            momentum_init_pan_secs: core::time::Duration::from_millis(40).as_secs_f32(),
            momentum_init_orbit_secs: core::time::Duration::from_millis(60).as_secs_f32(),
            input_debounce_secs: core::time::Duration::from_millis(80).as_secs_f32(),
            perspective_near_clip_min: 1e-9,
            perspective_near_clip_max: f32::INFINITY,
            perspective_near_clip_multiplier: 0.05,
            ortho_scale_to_near_clip: 1_000_000.0,
            ortho_near_clip_min: 1.0,
            ortho_near_clip_max: 1_000_000.0,
            ortho_far_clip_multiplier: 1.0,
            enabled_pan: FfiBool::TRUE,
            enabled_orbit: FfiBool::TRUE,
            enabled_zoom: FfiBool::TRUE,
            orbit_constraint_mode: ORBIT_CONSTRAINT_FIXED,
            orbit_can_pass_tdc: FfiBool::FALSE,
            zoom_through_objects: FfiBool::FALSE,
            momentum_pan_damping: 160,
            momentum_orbit_damping: 160,
        }
    }
}

/// Zero-padding `#[repr(C)]` wire mirror of Bevy `0.20`'s `SolariLighting` realtime global
/// illumination component (`"bamabi::camera_light::FfiSolariLighting"`, 44 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiSolariLighting {
    /// Maximum ReSTIR confidence weight (`SolariLighting::confidence_weight_cap`).
    pub confidence_weight_cap: f32,
    /// Primary hit direct light sample count (`SolariLighting::primary_di_samples`).
    pub primary_di_samples: u32,
    /// Indirect bounce direct light sample count (`SolariLighting::secondary_di_samples`).
    pub secondary_di_samples: u32,
    /// Maximum path bounce count (`SolariLighting::max_bounces`).
    pub max_bounces: u32,
    /// World cache temporal responsiveness (`SolariLighting::world_cache_max_temporal_samples`).
    pub world_cache_max_temporal_samples: f32,
    /// Direct light samples per world cache cell (`SolariLighting::world_cache_direct_light_sample_count`).
    pub world_cache_direct_light_sample_count: u32,
    /// Maximum GI ray distance between world cache cells (`SolariLighting::world_cache_max_gi_ray_distance`).
    pub world_cache_max_gi_ray_distance: f32,
    /// Stochastic soft target for world cache cell updates per frame (`SolariLighting::world_cache_cell_updates_soft_target`).
    pub world_cache_cell_updates_soft_target: u32,
    /// Base world cache cell size in meters (`SolariLighting::world_cache_position_base_cell_size`).
    pub world_cache_position_base_cell_size: f32,
    /// World cache LOD distance transition scale (`SolariLighting::world_cache_position_lod_scale`).
    pub world_cache_position_lod_scale: f32,
    /// Whether spatiotemporal ReSTIR resampling is enabled (`SolariLighting::restir`).
    pub restir: FfiBool,
    /// Set to `true` to discard temporal history on camera cuts (`SolariLighting::reset`).
    pub reset: FfiBool,
    /// Explicit zero-reserved alignment padding bytes.
    pub reserved0: [u8; 2],
}

impl_wire_abi_pod!(
    FfiSolariLighting,
    size = 44,
    align = 4,
    schema = [
        confidence_weight_cap: f32 => "f32",
        primary_di_samples: u32 => "u32",
        secondary_di_samples: u32 => "u32",
        max_bounces: u32 => "u32",
        world_cache_max_temporal_samples: f32 => "f32",
        world_cache_direct_light_sample_count: u32 => "u32",
        world_cache_max_gi_ray_distance: f32 => "f32",
        world_cache_cell_updates_soft_target: u32 => "u32",
        world_cache_position_base_cell_size: f32 => "f32",
        world_cache_position_lod_scale: f32 => "f32",
        restir: FfiBool => "FfiBool",
        reset: FfiBool => "FfiBool",
        reserved0: [u8; 2] => "[u8;2]",
    ]
);

impl Default for FfiSolariLighting {
    #[inline]
    fn default() -> Self {
        Self {
            confidence_weight_cap: 8.0,
            primary_di_samples: 8,
            secondary_di_samples: 4,
            max_bounces: 3,
            world_cache_max_temporal_samples: 32.0,
            world_cache_direct_light_sample_count: 32,
            world_cache_max_gi_ray_distance: 50.0,
            world_cache_cell_updates_soft_target: 40_000,
            world_cache_position_base_cell_size: 0.15,
            world_cache_position_lod_scale: 15.0,
            restir: FfiBool::FALSE,
            reset: FfiBool::TRUE,
            reserved0: [0; 2],
        }
    }
}
