//! Host-opt-in Bevy `0.20` 2D sprite, mesh handle, `StandardMaterial`, and WESL shader/sprite
//! material wire projection types.

use bamabi_sys::{ComponentUuid, FfiAssetHandle, FfiBool};

use super::std_types::FfiColor;

/// Host-opt-in `#[repr(C)]` wire projection of `bevy_sprite::Sprite`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiSprite {
    /// Texture `Image` asset handle (`FfiAssetHandle::NONE` for solid-color quad).
    pub image: FfiAssetHandle,
    /// Tint color (sRGBA).
    pub color: FfiColor,
    /// Custom size `[width, height]` in world units (active when `has_custom_size.get()`).
    pub custom_size: [f32; 2],
    /// Sub-rectangle `[min_x, min_y, max_x, max_y]` in texture pixels (active when `has_rect.get()`).
    pub rect: [f32; 4],
    /// Normalized anchor offset (`[0.0, 0.0]` = center).
    pub anchor: [f32; 2],
    /// Flip sprite horizontally (`FfiBool::TRUE`).
    pub flip_x: FfiBool,
    /// Flip sprite vertically (`FfiBool::TRUE`).
    pub flip_y: FfiBool,
    /// Whether `custom_size` overrides the texture dimensions.
    pub has_custom_size: FfiBool,
    /// Whether `rect` crops the source image.
    pub has_rect: FfiBool,
}

impl_wire_abi_pod!(
    FfiSprite,
    size = 84,
    align = 4,
    schema = [
        image: FfiAssetHandle => "FfiAssetHandle",
        color: FfiColor => "FfiColor",
        custom_size: [f32; 2] => "[f32;2]",
        rect: [f32; 4] => "[f32;4]",
        anchor: [f32; 2] => "[f32;2]",
        flip_x: FfiBool => "FfiBool",
        flip_y: FfiBool => "FfiBool",
        has_custom_size: FfiBool => "FfiBool",
        has_rect: FfiBool => "FfiBool",
    ]
);

impl Default for FfiSprite {
    #[inline]
    fn default() -> Self {
        Self {
            image: FfiAssetHandle::NONE,
            color: FfiColor::WHITE,
            custom_size: [0.0, 0.0],
            rect: [0.0, 0.0, 0.0, 0.0],
            anchor: [0.0, 0.0],
            flip_x: FfiBool::FALSE,
            flip_y: FfiBool::FALSE,
            has_custom_size: FfiBool::FALSE,
            has_rect: FfiBool::FALSE,
        }
    }
}

/// Host-opt-in `#[repr(C)]` wire projection of `bevy_pbr::StandardMaterial` core PBR parameters.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiStandardMaterial {
    /// Albedo base color (sRGBA).
    pub base_color: FfiColor,
    /// Emissive color (linear HDR intensity in `[r, g, b, a]`).
    pub emissive: [f32; 4],
    /// Base color texture `Image` asset handle (`NONE` if untextured).
    pub base_color_texture: FfiAssetHandle,
    /// Normal map texture `Image` asset handle (`NONE` if absent).
    pub normal_map_texture: FfiAssetHandle,
    /// Metallic-roughness texture `Image` asset handle (`NONE` if absent).
    pub metallic_roughness_texture: FfiAssetHandle,
    /// Perceptual roughness in `[0.089, 1.0]`.
    pub perceptual_roughness: f32,
    /// Metallic factor in `[0.0, 1.0]`.
    pub metallic: f32,
    /// Dielectric reflectance (`0.5` default).
    pub reflectance: f32,
    /// Specular transmission in `[0.0, 1.0]`.
    pub specular_transmission: f32,
    /// Index of refraction (`1.5` default).
    pub ior: f32,
    /// Alpha cutoff threshold when `alpha_mode == Self::ALPHA_MASK`.
    pub alpha_cutoff: f32,
    /// Alpha blending mode (`0..=6`).
    pub alpha_mode: u8,
    /// Disable lighting (`FfiBool::TRUE`).
    pub unlit: FfiBool,
    /// Render both front and back faces (`FfiBool::TRUE`).
    pub double_sided: FfiBool,
    /// Explicit zero-reserved alignment byte.
    pub reserved0: u8,
}

impl_wire_abi_pod!(
    FfiStandardMaterial,
    size = 156,
    align = 4,
    schema = [
        base_color: FfiColor => "FfiColor",
        emissive: [f32; 4] => "[f32;4]",
        base_color_texture: FfiAssetHandle => "FfiAssetHandle",
        normal_map_texture: FfiAssetHandle => "FfiAssetHandle",
        metallic_roughness_texture: FfiAssetHandle => "FfiAssetHandle",
        perceptual_roughness: f32 => "f32",
        metallic: f32 => "f32",
        reflectance: f32 => "f32",
        specular_transmission: f32 => "f32",
        ior: f32 => "f32",
        alpha_cutoff: f32 => "f32",
        alpha_mode: u8 => "u8",
        unlit: FfiBool => "FfiBool",
        double_sided: FfiBool => "FfiBool",
        reserved0: u8 => "u8",
    ]
);

impl FfiStandardMaterial {
    /// Fully opaque alpha mode (`AlphaMode::Opaque`).
    pub const ALPHA_OPAQUE: u8 = 0;
    /// Alpha cutoff mask mode (`AlphaMode::Mask`).
    pub const ALPHA_MASK: u8 = 1;
    /// Straight alpha blending (`AlphaMode::Blend`).
    pub const ALPHA_BLEND: u8 = 2;
    /// Premultiplied alpha blending (`AlphaMode::Premultiplied`).
    pub const ALPHA_PREMULTIPLIED: u8 = 3;
    /// Additive blending (`AlphaMode::Add`).
    pub const ALPHA_ADD: u8 = 4;
    /// Multiplicative blending (`AlphaMode::Multiply`).
    pub const ALPHA_MULTIPLY: u8 = 5;
    /// MSAA alpha-to-coverage (`AlphaMode::AlphaToCoverage`).
    pub const ALPHA_TO_COVERAGE: u8 = 6;
}

impl Default for FfiStandardMaterial {
    #[inline]
    fn default() -> Self {
        Self {
            base_color: FfiColor::WHITE,
            emissive: [0.0, 0.0, 0.0, 1.0],
            base_color_texture: FfiAssetHandle::NONE,
            normal_map_texture: FfiAssetHandle::NONE,
            metallic_roughness_texture: FfiAssetHandle::NONE,
            perceptual_roughness: 0.5,
            metallic: 0.0,
            reflectance: 0.5,
            specular_transmission: 0.0,
            ior: 1.5,
            alpha_cutoff: 0.5,
            alpha_mode: Self::ALPHA_OPAQUE,
            unlit: FfiBool::FALSE,
            double_sided: FfiBool::FALSE,
            reserved0: 0,
        }
    }
}

/// Host-opt-in `#[repr(C)]` specification for a Bevy `0.20` WESL / WGSL custom shader material
/// (`Shader` asset + up to 2 bound `Image` textures + 4 `vec4<f32>` uniform parameters).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiWeslMaterialSpec {
    /// Compiled/loaded `.wesl` or `.wgsl` `Shader` asset handle.
    pub shader: FfiAssetHandle,
    /// Optional primary texture `Image` asset handle.
    pub texture_0: FfiAssetHandle,
    /// Optional secondary texture `Image` asset handle.
    pub texture_1: FfiAssetHandle,
    /// Four `vec4<f32>` uniform slots passed to the material shader.
    pub params: [[f32; 4]; 4],
    /// Alpha blending mode (same constants as [`FfiStandardMaterial`]).
    pub alpha_mode: u8,
    /// Render double-sided (`FfiBool::TRUE`).
    pub double_sided: FfiBool,
    /// Explicit zero-reserved alignment bytes.
    pub reserved0: [u8; 2],
}

impl_wire_abi_pod!(
    FfiWeslMaterialSpec,
    size = 164,
    align = 4,
    schema = [
        shader: FfiAssetHandle => "FfiAssetHandle",
        texture_0: FfiAssetHandle => "FfiAssetHandle",
        texture_1: FfiAssetHandle => "FfiAssetHandle",
        params: [[f32; 4]; 4] => "[[f32;4];4]",
        alpha_mode: u8 => "u8",
        double_sided: FfiBool => "FfiBool",
        reserved0: [u8; 2] => "[u8;2]",
    ]
);

/// Host-opt-in `#[repr(C)]` specification for a Bevy `0.20` 2D Sprite custom material
/// (`SpriteMaterial` / `Material2d` with WESL shader + tint + uniform parameters).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiSpriteMaterialSpec {
    /// `.wesl` or `.wgsl` fragment/vertex `Shader` asset handle.
    pub shader: FfiAssetHandle,
    /// Sprite texture `Image` asset handle.
    pub texture: FfiAssetHandle,
    /// Base tint color (sRGBA).
    pub color: FfiColor,
    /// Two `vec4<f32>` uniform slots passed to the 2D sprite shader.
    pub params: [[f32; 4]; 2],
}

impl_wire_abi_pod!(
    FfiSpriteMaterialSpec,
    size = 112,
    align = 4,
    schema = [
        shader: FfiAssetHandle => "FfiAssetHandle",
        texture: FfiAssetHandle => "FfiAssetHandle",
        color: FfiColor => "FfiColor",
        params: [[f32; 4]; 2] => "[[f32;4];2]",
    ]
);

impl Default for FfiSpriteMaterialSpec {
    #[inline]
    fn default() -> Self {
        Self {
            shader: FfiAssetHandle::NONE,
            texture: FfiAssetHandle::NONE,
            color: FfiColor::WHITE,
            params: [[0.0; 4]; 2],
        }
    }
}

/// Host-opt-in `#[repr(transparent)]` wire projection of `bevy_mesh::Mesh2d(Handle<Mesh>)`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfiMesh2d(pub FfiAssetHandle);

impl_wire_abi_pod!(FfiMesh2d, size = 32, align = 4, fields = [FfiAssetHandle]);

impl FfiMesh2d {
    /// Deterministic SipHash-2-4 field-schema hash matching `#[derive(ModComponent)]`.
    pub const FIELD_SCHEMA_HASH: u64 =
        ComponentUuid::hash_field_schema(0, "0", "FfiAssetHandle", 0);
}

/// Host-opt-in `#[repr(transparent)]` wire projection of `bevy_mesh::Mesh3d(Handle<Mesh>)`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfiMesh3d(pub FfiAssetHandle);

impl_wire_abi_pod!(FfiMesh3d, size = 32, align = 4, fields = [FfiAssetHandle]);

impl FfiMesh3d {
    /// Deterministic SipHash-2-4 field-schema hash matching `#[derive(ModComponent)]`.
    pub const FIELD_SCHEMA_HASH: u64 =
        ComponentUuid::hash_field_schema(0, "0", "FfiAssetHandle", 0);
}
