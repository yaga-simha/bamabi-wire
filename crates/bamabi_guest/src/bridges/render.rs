//! Host-opt-in `#![forbid(unsafe_code)]`, zero-Bevy guest bridge conventions for Bevy `0.20`
//! 2D sprites (`Sprite`, `SpriteMaterial` / WESL 2D sprite shaders), 2D/3D mesh handles (`Mesh2d`,
//! `Mesh3d`), `StandardMaterial`, and custom WESL shader materials.
//!
//! **Host-opt-in convention:** Unlike `std_types`, `state`, `picking`, `camera_light`, `audio`,
//! and `ui` (which are installed automatically by `bamabi_bridges`'s preset plugins), these wire
//! types require the host game to opt in to exposing the corresponding `HostWireBridge` VTables
//! (`"bevy_sprite::Sprite"`, `"bevy_pbr::StandardMaterial"`) or direct POD mirror components
//! (`"bamabi::render::Ffi*"`).

use bamabi_guest_sys::Host;
use bamabi_sys::{AccessMode, ComponentUuid, FfiEntity, FfiResult};
use core::ffi::CStr;

pub use bamabi_guest_sys::wire_types::{
    FfiMesh2d, FfiMesh3d, FfiSprite, FfiSpriteMaterialSpec, FfiStandardMaterial,
    FfiWeslMaterialSpec,
};

use crate::{
    app::QueryBuilder, bridges::std_types::GuestWireBridge, commands::GuestCommands,
    component::GuestComponent,
};

/// Canonical ABI name for host-bridged native `bevy_sprite::Sprite` (`GuestSpriteBridge`).
pub const CANONICAL_SPRITE_NAME: &str = "bevy_sprite::Sprite";
/// Canonical ABI name for direct POD mirror [`FfiSprite`].
pub const CANONICAL_FFI_SPRITE_NAME: &str = "bamabi::render::FfiSprite";
/// Canonical ABI name for host-bridged native `bevy_pbr::StandardMaterial`.
pub const CANONICAL_STANDARD_MATERIAL_NAME: &str = "bevy_pbr::StandardMaterial";
/// Canonical ABI name for direct POD mirror [`FfiStandardMaterial`].
pub const CANONICAL_FFI_STANDARD_MATERIAL_NAME: &str = "bamabi::render::FfiStandardMaterial";
/// Canonical ABI name for Bevy `0.20` custom WESL shader material spec.
pub const CANONICAL_WESL_MATERIAL_SPEC_NAME: &str = "bamabi::render::FfiWeslMaterialSpec";
/// Canonical ABI name for Bevy `0.20` 2D sprite WESL material spec.
pub const CANONICAL_SPRITE_MATERIAL_SPEC_NAME: &str = "bamabi::render::FfiSpriteMaterialSpec";
/// Canonical ABI name for direct POD mirror [`FfiMesh2d`].
pub const CANONICAL_MESH_2D_NAME: &str = "bamabi::render::FfiMesh2d";
/// Canonical ABI name for direct POD mirror [`FfiMesh3d`].
pub const CANONICAL_MESH_3D_NAME: &str = "bamabi::render::FfiMesh3d";

impl GuestComponent for FfiSprite {
    const NAME: &'static CStr = c"bamabi::render::FfiSprite";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiStandardMaterial {
    const NAME: &'static CStr = c"bamabi::render::FfiStandardMaterial";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiWeslMaterialSpec {
    const NAME: &'static CStr = c"bamabi::render::FfiWeslMaterialSpec";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiSpriteMaterialSpec {
    const NAME: &'static CStr = c"bamabi::render::FfiSpriteMaterialSpec";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiMesh2d {
    const NAME: &'static CStr = c"bamabi::render::FfiMesh2d";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}
impl GuestComponent for FfiMesh3d {
    const NAME: &'static CStr = c"bamabi::render::FfiMesh3d";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}

/// Guest wire VTable bridge for host-opt-in `bevy_sprite::Sprite`.
pub type GuestSpriteBridge = GuestWireBridge<FfiSprite>;
/// Guest wire VTable bridge for host-opt-in `bevy_pbr::StandardMaterial`.
pub type GuestStandardMaterialBridge = GuestWireBridge<FfiStandardMaterial>;

impl GuestSpriteBridge {
    /// Resolves a host-opt-in `Sprite` VTable (`"bam.v0.Sprite.vtable"`).
    pub fn resolve_sprite(host: &Host) -> Result<Self, FfiResult> {
        Self::resolve(
            host,
            CANONICAL_SPRITE_NAME,
            c"bam.v0.bevy_sprite::Sprite.vtable",
            Some(c"bam.v0.Sprite.vtable"),
        )
    }
}

impl GuestStandardMaterialBridge {
    /// Resolves a host-opt-in `StandardMaterial` VTable (`"bam.v0.StandardMaterial.vtable"`).
    pub fn resolve_standard_material(host: &Host) -> Result<Self, FfiResult> {
        Self::resolve(
            host,
            CANONICAL_STANDARD_MATERIAL_NAME,
            c"bam.v0.bevy_pbr::StandardMaterial.vtable",
            Some(c"bam.v0.StandardMaterial.vtable"),
        )
    }
}

/// Extension trait on [`QueryBuilder`] adding host-opt-in bridged Bevy `0.20` sprite query terms.
pub trait GuestRenderQueryBuilderExt {
    /// Appends a shared read (`&Sprite` wire-bridged) term.
    fn read_sprite(self) -> Self;
    /// Appends an exclusive write (`&mut Sprite` wire-bridged) term.
    fn write_sprite(self) -> Self;
    /// Appends a shared read (`&FfiSprite` direct POD mirror) term.
    fn read_ffi_sprite(self) -> Self;
    /// Appends an exclusive write (`&mut FfiSprite` direct POD mirror) term.
    fn write_ffi_sprite(self) -> Self;
}

impl GuestRenderQueryBuilderExt for QueryBuilder<'_> {
    #[inline]
    fn read_sprite(self) -> Self {
        self.term_named(
            c"bevy_sprite::Sprite",
            Some(ComponentUuid::from_canonical_name(CANONICAL_SPRITE_NAME)),
            AccessMode::READ,
        )
    }

    #[inline]
    fn write_sprite(self) -> Self {
        self.term_named(
            c"bevy_sprite::Sprite",
            Some(ComponentUuid::from_canonical_name(CANONICAL_SPRITE_NAME)),
            AccessMode::WRITE,
        )
    }

    #[inline]
    fn read_ffi_sprite(self) -> Self {
        self.term_named(FfiSprite::NAME, Some(FfiSprite::UUID), AccessMode::READ)
    }

    #[inline]
    fn write_ffi_sprite(self) -> Self {
        self.term_named(FfiSprite::NAME, Some(FfiSprite::UUID), AccessMode::WRITE)
    }
}

/// Extension trait on [`GuestCommands`] for inserting host-opt-in direct POD Bevy `0.20` sprites,
/// meshes, and WESL shader materials.
pub trait GuestCommandsRenderExt {
    /// Inserts [`FfiSprite`] onto `entity`.
    fn insert_sprite(&mut self, entity: FfiEntity, sprite: &FfiSprite) -> FfiResult;
    /// Inserts [`FfiMesh2d`] onto `entity`.
    fn insert_mesh_2d(&mut self, entity: FfiEntity, mesh: FfiMesh2d) -> FfiResult;
    /// Inserts [`FfiMesh3d`] onto `entity`.
    fn insert_mesh_3d(&mut self, entity: FfiEntity, mesh: FfiMesh3d) -> FfiResult;
    /// Inserts [`FfiWeslMaterialSpec`] onto `entity`.
    fn insert_wesl_material(&mut self, entity: FfiEntity, spec: &FfiWeslMaterialSpec) -> FfiResult;
    /// Inserts [`FfiSpriteMaterialSpec`] onto `entity`.
    fn insert_sprite_material(
        &mut self,
        entity: FfiEntity,
        spec: &FfiSpriteMaterialSpec,
    ) -> FfiResult;
}

impl GuestCommandsRenderExt for GuestCommands<'_> {
    #[inline]
    fn insert_sprite(&mut self, entity: FfiEntity, sprite: &FfiSprite) -> FfiResult {
        self.insert_component(entity, sprite)
    }

    #[inline]
    fn insert_mesh_2d(&mut self, entity: FfiEntity, mesh: FfiMesh2d) -> FfiResult {
        self.insert_component(entity, &mesh)
    }

    #[inline]
    fn insert_mesh_3d(&mut self, entity: FfiEntity, mesh: FfiMesh3d) -> FfiResult {
        self.insert_component(entity, &mesh)
    }

    #[inline]
    fn insert_wesl_material(&mut self, entity: FfiEntity, spec: &FfiWeslMaterialSpec) -> FfiResult {
        self.insert_component(entity, spec)
    }

    #[inline]
    fn insert_sprite_material(
        &mut self,
        entity: FfiEntity,
        spec: &FfiSpriteMaterialSpec,
    ) -> FfiResult {
        self.insert_component(entity, spec)
    }
}
