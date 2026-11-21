//! Generated C99 component & VTable header emitter (`cargo xtask gen-component-headers`).
//!
//! Emits `include/generated/testbed_components.h` from the canonical `#[derive(ModComponent)]`
//! testbed definitions using [`bamabi_sdk_gen::render_component_header`].

use std::fs;
use std::path::Path;

use bamabi_macros::ModComponent;
use bamabi_sdk_gen::{render_component_header, section_for};

#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, ModComponent)]
#[mod_component(name = "testbed::Position")]
struct Position {
    x: f32,
    y: f32,
    z: f32,
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, ModComponent)]
#[mod_component(name = "testbed::Velocity")]
struct Velocity {
    x: f32,
    y: f32,
    z: f32,
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, ModComponent)]
#[mod_component(name = "testbed::CombatHealth")]
struct CombatHealth {
    current: i32,
    max: i32,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Default, ModComponent)]
#[mod_component(name = "testbed::NameTag")]
struct NameTag {
    label: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Default, ModComponent)]
#[mod_component(name = "testbed::MetadataTag")]
struct MetadataTag {
    label: String,
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, ModComponent)]
#[mod_component(name = "testbed::Mass")]
struct Mass {
    value: f32,
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, ModComponent)]
#[mod_component(name = "testbed::Armor")]
struct Armor {
    defense: i32,
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, ModComponent)]
#[mod_component(name = "testbed::FilteredTransform")]
struct FilteredTransform {
    x: f32,
    y: f32,
    z: f32,
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, ModComponent)]
#[mod_component(name = "testbed::CFilteredTransform")]
struct CFilteredTransform {
    x: f32,
    y: f32,
    z: f32,
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, ModComponent)]
#[mod_component(name = "testbed::ChangeSensor")]
struct ChangeSensor {
    val: u32,
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, ModComponent)]
#[mod_component(name = "testbed::CChangeSensor")]
struct CChangeSensor {
    val: u32,
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, ModComponent)]
#[mod_component(name = "testbed::FreshSpawnTag")]
struct FreshSpawnTag {
    code: u32,
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, ModComponent)]
#[mod_component(name = "testbed::CFreshSpawnTag")]
struct CFreshSpawnTag {
    code: u32,
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, ModComponent)]
#[mod_component(name = "testbed::TransientMarker")]
struct TransientMarker {
    dummy: u32,
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, ModComponent)]
#[mod_component(name = "testbed::CTransientMarker")]
struct CTransientMarker {
    dummy: u32,
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, ModComponent)]
#[mod_component(name = "testbed::RustFilterTelemetry")]
struct RustFilterTelemetry {
    scale: f32,
    matched_rows: u32,
    optional_vel_hits: u32,
    _pad0: u32,
    spawned_default_entity: u64,
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, ModComponent)]
#[mod_component(name = "testbed::CFilterTelemetry")]
struct CFilterTelemetry {
    scale: f32,
    matched_rows: u32,
    optional_vel_hits: u32,
    _pad0: u32,
    spawned_default_entity: u64,
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, ModComponent)]
#[mod_component(name = "c_interop::Vec3")]
struct Vec3 {
    x: f32,
    y: f32,
    z: f32,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Default, ModComponent)]
#[mod_component(name = "c_interop::CInteropComponent")]
struct CInteropComponent {
    health: i32,
    pos: Vec3,
    label: String,
}

/// Renders the deterministic C99/C++17 header `include/generated/testbed_components.h`.
pub(crate) fn generate_testbed_components_header() -> String {
    render_component_header(
        "BAMABI_GENERATED_TESTBED_COMPONENTS_H",
        "../bamabi.h",
        &[
            section_for::<Position>(),
            section_for::<Velocity>(),
            section_for::<CombatHealth>(),
            section_for::<NameTag>(),
            section_for::<MetadataTag>(),
            section_for::<Mass>(),
            section_for::<Armor>(),
            section_for::<FilteredTransform>(),
            section_for::<CFilteredTransform>(),
            section_for::<ChangeSensor>(),
            section_for::<CChangeSensor>(),
            section_for::<FreshSpawnTag>(),
            section_for::<CFreshSpawnTag>(),
            section_for::<TransientMarker>(),
            section_for::<CTransientMarker>(),
            section_for::<RustFilterTelemetry>(),
            section_for::<CFilterTelemetry>(),
            section_for::<Vec3>(),
            section_for::<CInteropComponent>(),
        ],
    )
}

/// Writes `include/generated/testbed_components.h` or verifies byte-for-byte parity.
pub(crate) fn write_or_check_component_headers(
    root: &Path,
    check_only: bool,
) -> Result<(), String> {
    let gen_dir = root.join("include").join("generated");
    let header_path = gen_dir.join("testbed_components.h");
    let expected = generate_testbed_components_header();

    if check_only {
        let existing_raw = fs::read_to_string(&header_path).map_err(|e| {
            format!(
                "missing {} ({e}); run `cargo xtask gen-component-headers`",
                header_path.display()
            )
        })?;
        let existing = existing_raw.replace("\r\n", "\n");
        if existing != expected {
            let diff = first_line_diff(&expected, &existing);
            return Err(format!(
                "{} is out of date with `ModComponent::C_DECL` ({diff}); run `cargo xtask gen-component-headers`",
                header_path.display()
            ));
        }
        println!(
            "==> Verified {} matches ModComponent::C_DECL byte-for-byte",
            header_path.display()
        );
        return Ok(());
    }

    fs::create_dir_all(&gen_dir)
        .map_err(|e| format!("failed to create {}: {e}", gen_dir.display()))?;
    let up_to_date = fs::read_to_string(&header_path)
        .map(|existing| existing.replace("\r\n", "\n") == expected)
        .unwrap_or(false);
    if up_to_date {
        println!("==> {} is up to date", header_path.display());
    } else {
        fs::write(&header_path, expected)
            .map_err(|e| format!("failed to write {}: {e}", header_path.display()))?;
        println!("==> Generated {}", header_path.display());
    }
    Ok(())
}

/// Formats the first differing line or length mismatch between rendered and on-disk text.
pub(crate) fn first_line_diff(expected: &str, actual: &str) -> String {
    expected
        .lines()
        .zip(actual.lines())
        .enumerate()
        .find(|(_, (a, b))| a != b)
        .map(|(idx, (a, b))| format!("first diff at line {}: expected {a:?}, got {b:?}", idx + 1))
        .unwrap_or_else(|| {
            format!(
                "length mismatch: expected {} bytes, got {} bytes",
                expected.len(),
                actual.len()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Proves that the checked-in `include/generated/testbed_components.h` matches the
    /// deterministic output of `ModComponent::C_DECL` byte-for-byte (modulo CRLF normalization).
    #[test]
    fn generated_testbed_header_matches_checked_in_file() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask sits directly under workspace root");
        write_or_check_component_headers(repo_root, true)
            .expect("include/generated/testbed_components.h must match ModComponent::C_DECL");
    }
}
