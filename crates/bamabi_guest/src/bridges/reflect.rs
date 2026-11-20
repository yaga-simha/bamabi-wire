//! Safe (`#![forbid(unsafe_code)]`), zero-Bevy guest helpers for dynamic reflection, BRP
//! (`bamabi/*`, `bam/*`, and standard Bevy `0.20` `world.*` / `registry.schema` / `rpc.discover`),
//! settings bridges, and 64-byte `bql_lsp` telemetry frames (`SystemTelemetryDigest64`).

use bamabi_guest_sys::{Host, resolve_component};
use bamabi_sys::{BamComponent, ComponentUuid, FfiResult};
use core::ffi::CStr;
use std::ffi::CString;

pub use bamabi_guest_sys::wire_types::{
    SYSTEM_TELEMETRY_DIGEST_SIZE, SYSTEM_TELEMETRY_DIGEST_VERSION, SYSTEM_TELEMETRY_MAGIC,
    SystemTelemetryDigest64, TELEMETRY_FLAG_BUDGET_BREACHED, TELEMETRY_FLAG_FAULTED,
    TELEMETRY_FLAG_QUARANTINED, TELEMETRY_FLAG_SUMMARY_TICK_ACTIVE, TelemetryFrameError,
    fnv1a_64_hash_system,
};

use crate::component::GuestComponent;

/// BRP method path listing mod-registered components, resources, and witness schemas.
pub const BRP_METHOD_LIST_COMPONENTS: &str = "bamabi/list_components";
/// BRP method path inspecting mod-registered dynamic or reflected components on an entity.
pub const BRP_METHOD_GET_COMPONENTS: &str = "bamabi/get_components";
/// BRP method path mutating mod-registered dynamic or reflected components on an entity.
pub const BRP_METHOD_MUTATE_COMPONENTS: &str = "bamabi/mutate_components";
/// BRP method path inserting or updating a witness-backed or reflected component on an entity.
pub const BRP_METHOD_INSERT_COMPONENTS: &str = "bamabi/insert_components";
/// BRP method path inspecting a mod-registered singleton resource.
pub const BRP_METHOD_GET_RESOURCES: &str = "bamabi/get_resources";
/// BRP method path mutating a mod-registered singleton resource.
pub const BRP_METHOD_MUTATE_RESOURCES: &str = "bamabi/mutate_resources";

/// Telemetry BRP method listing loaded mods, versions, and health states (`bamabi_reflect::telemetry`).
pub const BRP_METHOD_BAM_LIST_MODS: &str = "bam/list_mods";
/// Telemetry BRP method returning per-system latency/fault digests (`bamabi_reflect::telemetry`).
pub const BRP_METHOD_BAM_SYSTEM_METRICS: &str = "bam/system_metrics";
/// Telemetry BRP method listing registered ABI `0.3` component schemas and fingerprints.
pub const BRP_METHOD_BAM_COMPONENT_SCHEMAS: &str = "bam/component_schemas";
/// Control BRP method triggering live hot-reload of a loaded mod.
pub const BRP_METHOD_BAM_HOT_RELOAD: &str = "bam/hot_reload";

/// Standard Bevy `0.20` BRP method fetching reflected components on an entity.
pub const BRP_METHOD_WORLD_GET_COMPONENTS: &str = "world.get_components";
/// Standard Bevy `0.20` BRP method executing an ad-hoc ECS query.
pub const BRP_METHOD_WORLD_QUERY: &str = "world.query";
/// Standard Bevy `0.20` BRP method spawning a new entity with reflected components.
pub const BRP_METHOD_WORLD_SPAWN_ENTITY: &str = "world.spawn_entity";
/// Standard Bevy `0.20` BRP method despawning an entity.
pub const BRP_METHOD_WORLD_DESPAWN_ENTITY: &str = "world.despawn_entity";
/// Standard Bevy `0.20` BRP method inserting reflected components onto an entity.
pub const BRP_METHOD_WORLD_INSERT_COMPONENTS: &str = "world.insert_components";
/// Standard Bevy `0.20` BRP method removing reflected components from an entity.
pub const BRP_METHOD_WORLD_REMOVE_COMPONENTS: &str = "world.remove_components";
/// Standard Bevy `0.20` BRP method mutating a reflected component field on an entity.
pub const BRP_METHOD_WORLD_MUTATE_COMPONENTS: &str = "world.mutate_components";
/// Standard Bevy `0.20` BRP method listing reflected component types on an entity or world.
pub const BRP_METHOD_WORLD_LIST_COMPONENTS: &str = "world.list_components";
/// Standard Bevy `0.20` BRP method fetching a reflected resource value.
pub const BRP_METHOD_WORLD_GET_RESOURCES: &str = "world.get_resources";
/// Standard Bevy `0.20` BRP method inserting a reflected resource.
pub const BRP_METHOD_WORLD_INSERT_RESOURCES: &str = "world.insert_resources";
/// Standard Bevy `0.20` BRP method removing a reflected resource.
pub const BRP_METHOD_WORLD_REMOVE_RESOURCES: &str = "world.remove_resources";
/// Standard Bevy `0.20` BRP method mutating a reflected resource field.
pub const BRP_METHOD_WORLD_MUTATE_RESOURCES: &str = "world.mutate_resources";
/// Standard Bevy `0.20` BRP method listing reflected resource types.
pub const BRP_METHOD_WORLD_LIST_RESOURCES: &str = "world.list_resources";
/// Standard Bevy `0.20` BRP method exporting the JSON type registry schema.
pub const BRP_METHOD_REGISTRY_SCHEMA: &str = "registry.schema";
/// Standard Bevy `0.20` BRP JSON-RPC method discovery endpoint.
pub const BRP_METHOD_RPC_DISCOVER: &str = "rpc.discover";

impl GuestComponent for SystemTelemetryDigest64 {
    const NAME: &'static CStr = c"bamabi::telemetry::SystemTelemetryDigest64";
    const FIELD_SCHEMA_HASH: u64 = Self::FIELD_SCHEMA_HASH;
}

/// Resolves a host-registered reflected or witness-backed component/resource handle by name.
pub fn resolve_reflected_handle(
    host: &Host,
    canonical_name: &str,
) -> Result<BamComponent, FfiResult> {
    let c_name = CString::new(canonical_name).map_err(|_| FfiResult::INVALID_ARGUMENT)?;
    resolve_component(
        host,
        Some(c_name.as_c_str()),
        Some(ComponentUuid::from_canonical_name(canonical_name)),
    )
    .or_else(|_| resolve_component(host, Some(c_name.as_c_str()), None))
}

/// Resolves a host-exposed `SettingsGroup` resource handle by canonical name or short alias.
#[inline]
pub fn resolve_settings_handle(host: &Host, name: &CStr) -> Result<BamComponent, FfiResult> {
    resolve_component(host, Some(name), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: Verifies that all custom Bamabi BRP JSON-RPC method constants use the `bamabi/`
    /// namespace prefix and `bam/` telemetry namespace without colliding with Bevy `world.*`.
    #[test]
    fn brp_method_constants_use_bamabi_namespace() {
        for method in [
            BRP_METHOD_LIST_COMPONENTS,
            BRP_METHOD_GET_COMPONENTS,
            BRP_METHOD_MUTATE_COMPONENTS,
            BRP_METHOD_INSERT_COMPONENTS,
            BRP_METHOD_GET_RESOURCES,
            BRP_METHOD_MUTATE_RESOURCES,
        ] {
            assert!(method.starts_with("bamabi/"));
        }
        for method in [
            BRP_METHOD_BAM_LIST_MODS,
            BRP_METHOD_BAM_SYSTEM_METRICS,
            BRP_METHOD_BAM_COMPONENT_SCHEMAS,
            BRP_METHOD_BAM_HOT_RELOAD,
        ] {
            assert!(method.starts_with("bam/"));
        }
    }
}
